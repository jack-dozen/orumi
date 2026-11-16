#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    f32::consts::PI,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::GUID,
    Win32::{
        Media::Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
        },
    },
};

const FFT_SIZE: usize = 2048;
const NUM_BANDS: usize = 24;
const TICK_MS: u64 = 22; // ~45 fps capture/FFT emit rate
const STOP_GRACE_MS: u64 = 180; // brief grace window for tab switches

const SUBFORMAT_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBFORMAT_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

#[derive(Serialize, Clone)]
pub struct AudioVizFrame {
    pub bands: Vec<f32>,
    pub beat: f32,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);

fn wakeup() -> &'static (Mutex<bool>, Condvar) {
    static WAKEUP: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
    WAKEUP.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

#[tauri::command]
pub fn set_visualizer_active(active: bool) {
    let (lock, cvar) = wakeup();
    if let Ok(mut guard) = lock.lock() {
        ACTIVE.store(active, Ordering::SeqCst);
        *guard = active;
        cvar.notify_all();
    }
}

pub fn start(app: AppHandle) {
    if WORKER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || run_worker(app));
}

#[derive(Clone, Copy)]
enum SampleFmt {
    Float32,
    Pcm16,
    Pcm24,
    Pcm32,
}

struct MixFormat {
    sample_rate: u32,
    channels: usize,
    block_align: usize,
    fmt: SampleFmt,
}

struct CoTaskMemGuard<T>(*mut T);
impl<T> Drop for CoTaskMemGuard<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
        }
    }
}

struct CaptureSession {
    device_id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    mix: MixFormat,
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

unsafe fn default_device_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let id = device.GetId().ok()?;
    let _guard = CoTaskMemGuard(id.0);
    id.to_string().ok()
}

unsafe fn parse_mix_format(fmt_ptr: *const WAVEFORMATEX) -> Option<MixFormat> {
    let wf = *fmt_ptr;
    let tag = wf.wFormatTag;
    let bits = wf.wBitsPerSample;
    let channels = wf.nChannels as usize;
    let sample_rate = wf.nSamplesPerSec;
    let block_align = wf.nBlockAlign as usize;

    if channels == 0 || sample_rate == 0 || block_align == 0 {
        return None;
    }

    let fmt = if tag == 3 && bits == 32 {
        SampleFmt::Float32
    } else if tag == 1 {
        match bits {
            16 => SampleFmt::Pcm16,
            24 => SampleFmt::Pcm24,
            32 => SampleFmt::Pcm32,
            _ => return None,
        }
    } else if tag == 0xFFFE && wf.cbSize >= 22 {
        // WAVEFORMATEXTENSIBLE: WAVEFORMATEX (18 bytes) + Samples (2 bytes) + dwChannelMask (4 bytes) + SubFormat (16 bytes)
        let subformat = ptr::read_unaligned((fmt_ptr as *const u8).add(24) as *const GUID);
        if subformat == SUBFORMAT_IEEE_FLOAT && bits == 32 {
            SampleFmt::Float32
        } else if subformat == SUBFORMAT_PCM {
            match bits {
                16 => SampleFmt::Pcm16,
                24 => SampleFmt::Pcm24,
                32 => SampleFmt::Pcm32,
                _ => return None,
            }
        } else {
            return None;
        }
    } else {
        return None;
    };

    Some(MixFormat {
        sample_rate,
        channels,
        block_align,
        fmt,
    })
}

unsafe fn open_loopback() -> Option<CaptureSession> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let id_pwstr = device.GetId().ok()?;
    let device_id = {
        let _id_guard = CoTaskMemGuard(id_pwstr.0);
        id_pwstr.to_string().ok()?
    };

    let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;
    let fmt_ptr = client.GetMixFormat().ok()?;
    let _fmt_guard = CoTaskMemGuard(fmt_ptr);
    let mix = parse_mix_format(fmt_ptr)?;

    // 40ms shared-mode loopback buffer
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            400_000,
            0,
            fmt_ptr,
            None,
        )
        .ok()?;
    let capture: IAudioCaptureClient = client.GetService().ok()?;
    client.Start().ok()?;

    Some(CaptureSession {
        device_id,
        client,
        capture,
        mix,
    })
}

#[inline]
unsafe fn decode_mono_sample(frame_ptr: *const u8, mix: &MixFormat) -> f32 {
    let mut sum = 0.0f32;
    match mix.fmt {
        SampleFmt::Float32 => {
            let stride = 4;
            for ch in 0..mix.channels {
                let s = ptr::read_unaligned(frame_ptr.add(ch * stride) as *const f32);
                if s.is_finite() {
                    sum += s;
                }
            }
        }
        SampleFmt::Pcm16 => {
            let stride = 2;
            for ch in 0..mix.channels {
                let s = ptr::read_unaligned(frame_ptr.add(ch * stride) as *const i16);
                sum += s as f32 / 32768.0;
            }
        }
        SampleFmt::Pcm24 => {
            let stride = 3;
            for ch in 0..mix.channels {
                let p = frame_ptr.add(ch * stride);
                let b0 = *p as i32;
                let b1 = (*p.add(1) as i32) << 8;
                let b2 = (*p.add(2) as i8 as i32) << 16;
                let s = b0 | b1 | b2;
                sum += s as f32 / 8_388_608.0;
            }
        }
        SampleFmt::Pcm32 => {
            let stride = 4;
            for ch in 0..mix.channels {
                let s = ptr::read_unaligned(frame_ptr.add(ch * stride) as *const i32);
                sum += s as f32 / 2_147_483_648.0;
            }
        }
    }
    (sum / mix.channels as f32).clamp(-1.0, 1.0)
}

/// In-place iterative radix-2 Cooley-Tukey FFT. `re.len()` must be a power of 2.
fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let half = len >> 1;
        let angle = -2.0 * PI / (len as f32);
        let wlen_re = angle.cos();
        let wlen_im = angle.sin();

        let mut i = 0usize;
        while i < n {
            let mut w_re = 1.0f32;
            let mut w_im = 0.0f32;
            for k in 0..half {
                let u_re = re[i + k];
                let u_im = im[i + k];
                let t_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let t_im = re[i + k + half] * w_im + im[i + k + half] * w_re;

                re[i + k] = u_re + t_re;
                im[i + k] = u_im + t_im;
                re[i + k + half] = u_re - t_re;
                im[i + k + half] = u_im - t_im;

                let next_w_re = w_re * wlen_re - w_im * wlen_im;
                w_im = w_re * wlen_im + w_im * wlen_re;
                w_re = next_w_re;
            }
            i += len;
        }
        len <<= 1;
    }
}

struct Analyzer {
    ring: Vec<f32>,
    write_pos: usize,
    hann: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    mags: Vec<f32>,
    smoothed: [f32; NUM_BANDS],
    agc_peak: f32,
    bass_avg: f32,
    beat: f32,
}

impl Analyzer {
    fn new() -> Self {
        let hann = (0..FFT_SIZE)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos()))
            .collect();
        Self {
            ring: vec![0.0; FFT_SIZE],
            write_pos: 0,
            hann,
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            mags: vec![0.0; FFT_SIZE / 2],
            smoothed: [0.0; NUM_BANDS],
            agc_peak: 0.03,
            bass_avg: 0.0,
            beat: 0.0,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write_pos = 0;
        self.smoothed = [0.0; NUM_BANDS];
        self.agc_peak = 0.03;
        self.bass_avg = 0.0;
        self.beat = 0.0;
    }

    #[inline]
    fn push_sample(&mut self, s: f32) {
        self.ring[self.write_pos] = s;
        self.write_pos = (self.write_pos + 1) % FFT_SIZE;
    }

    fn push_silence(&mut self, count: usize) {
        for _ in 0..count.min(FFT_SIZE) {
            self.push_sample(0.0);
        }
    }

    fn process(&mut self, sample_rate: u32) -> ([f32; NUM_BANDS], f32) {
        for i in 0..FFT_SIZE {
            let idx = (self.write_pos + i) % FFT_SIZE;
            self.re[i] = self.ring[idx] * self.hann[i];
            self.im[i] = 0.0;
        }
        fft_in_place(&mut self.re, &mut self.im);

        let scale = 4.0 / FFT_SIZE as f32;
        for k in 0..(FFT_SIZE / 2) {
            self.mags[k] = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * scale;
        }

        let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
        let max_freq = (sample_rate as f32 * 0.45).min(12_500.0);
        let min_freq = 42.0f32;
        let ratio = (max_freq / min_freq).max(2.0);
        let max_bin = (FFT_SIZE / 2 - 1) as f32;

        let mut raw_bands = [0.0f32; NUM_BANDS];
        for b in 0..NUM_BANDS {
            let f_lo = min_freq * ratio.powf(b as f32 / NUM_BANDS as f32);
            let f_hi = min_freq * ratio.powf((b + 1) as f32 / NUM_BANDS as f32);
            let k_lo_f = (f_lo / bin_hz).clamp(1.0, max_bin);
            let k_hi_f = (f_hi / bin_hz).clamp(k_lo_f, max_bin);

            let k_lo = k_lo_f.floor() as usize;
            let k_hi = k_hi_f.ceil() as usize;

            let energy = if k_hi <= k_lo + 1 {
                // Fractional bin interpolation for narrow low-frequency bands
                let k_mid = 0.5 * (k_lo_f + k_hi_f);
                let i0 = (k_mid.floor() as usize).min(FFT_SIZE / 2 - 1);
                let i1 = (i0 + 1).min(FFT_SIZE / 2 - 1);
                let frac = k_mid - i0 as f32;
                self.mags[i0] * (1.0 - frac) + self.mags[i1] * frac
            } else {
                // RMS across bins in band
                let mut sq_sum = 0.0f32;
                let mut count = 0usize;
                for k in k_lo..=k_hi.min(FFT_SIZE / 2 - 1) {
                    sq_sum += self.mags[k] * self.mags[k];
                    count += 1;
                }
                (sq_sum / count.max(1) as f32).sqrt()
            };

            // Pink-noise tilt compensation so mids & highs dance alongside bass
            let f_mid = (f_lo * f_hi).sqrt();
            let tilt = (f_mid / 180.0).max(0.70).powf(0.42);
            raw_bands[b] = energy * tilt;
        }

        // Beat detection on low frequencies (~42..150 Hz: first 5 bands)
        let bass = (raw_bands[0] + raw_bands[1] + raw_bands[2] + raw_bands[3] + raw_bands[4]) / 5.0;
        self.bass_avg = self.bass_avg * 0.91 + bass * 0.09;
        if bass > 0.0025 && bass > self.bass_avg * 1.24 {
            let strength = ((bass / self.bass_avg.max(1e-6)) - 1.12).clamp(0.0, 1.0);
            self.beat = self.beat.max(strength);
        } else {
            self.beat *= 0.80;
            if self.beat < 0.01 {
                self.beat = 0.0;
            }
        }

        let frame_peak = raw_bands.iter().copied().fold(0.0f32, f32::max);
        if frame_peak > self.agc_peak {
            self.agc_peak = self.agc_peak * 0.65 + frame_peak * 0.35;
        } else {
            self.agc_peak = (self.agc_peak * 0.994).max(0.015);
        }

        let silent = frame_peak < 0.0004;
        for b in 0..NUM_BANDS {
            let target = if silent {
                0.0
            } else {
                let rel = (raw_bands[b] / self.agc_peak).max(1e-6);
                let db = 20.0 * rel.log10();
                let norm = ((db + 34.0) / 34.0).clamp(0.0, 1.0);
                // Slight low-end beat punch on bass/low-mid bars while preserving per-band dynamics
                let beat_weight = if b < 6 { 0.14 } else if b < 12 { 0.07 } else { 0.02 };
                (norm.powf(1.22) + self.beat * beat_weight).clamp(0.0, 1.0)
            };

            if target > self.smoothed[b] {
                self.smoothed[b] += (target - self.smoothed[b]) * 0.74;
            } else {
                self.smoothed[b] = (self.smoothed[b] * 0.78).max(target);
            }
            if self.smoothed[b] < 0.005 {
                self.smoothed[b] = 0.0;
            }
        }

        (self.smoothed, self.beat)
    }
}

fn run_worker(app: AppHandle) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut analyzer = Analyzer::new();
    let mut session: Option<CaptureSession> = None;
    let mut last_device_check = Instant::now();
    let mut last_active = Instant::now();
    let mut sent_zero = false;

    loop {
        if !ACTIVE.load(Ordering::SeqCst) {
            if session.is_some() && last_active.elapsed() >= Duration::from_millis(STOP_GRACE_MS) {
                session = None;
                analyzer.reset();
                if !sent_zero {
                    let _ = app.emit_to(
                        "notch",
                        "audio-viz",
                        AudioVizFrame {
                            bands: vec![0.0; NUM_BANDS],
                            beat: 0.0,
                        },
                    );
                    sent_zero = true;
                }
            }

            if session.is_none() {
                let (lock, cvar) = wakeup();
                if let Ok(mut guard) = lock.lock() {
                    while !*guard && !ACTIVE.load(Ordering::SeqCst) {
                        guard = cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                }
                last_active = Instant::now();
                continue;
            }
        } else {
            last_active = Instant::now();
        }

        // Check if the default render device changed (every 2s)
        if session.is_some() && last_device_check.elapsed() >= Duration::from_secs(2) {
            last_device_check = Instant::now();
            unsafe {
                if let Ok(enumerator) =
                    CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                {
                    if let Some(cur_id) = default_device_id(&enumerator) {
                        if session.as_ref().map(|s| s.device_id != cur_id).unwrap_or(false) {
                            session = None;
                            analyzer.reset();
                        }
                    }
                }
            }
        }

        if session.is_none() {
            session = unsafe { open_loopback() };
            last_device_check = Instant::now();
            if session.is_none() {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        }

        let mut invalidate = false;
        let mut got_frames = 0usize;
        let sample_rate = session.as_ref().unwrap().mix.sample_rate;

        if let Some(sess) = session.as_ref() {
            unsafe {
                loop {
                    let Ok(pkt) = sess.capture.GetNextPacketSize() else {
                        invalidate = true;
                        break;
                    };
                    if pkt == 0 {
                        break;
                    }
                    let mut data_ptr: *mut u8 = ptr::null_mut();
                    let mut num_frames: u32 = 0;
                    let mut flags: u32 = 0;
                    if sess
                        .capture
                        .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                        .is_err()
                    {
                        invalidate = true;
                        break;
                    }
                    let frames = num_frames as usize;
                    got_frames += frames;

                    if (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0 || data_ptr.is_null() {
                        analyzer.push_silence(frames);
                    } else {
                        for f in 0..frames {
                            let sample = decode_mono_sample(
                                data_ptr.add(f * sess.mix.block_align),
                                &sess.mix,
                            );
                            analyzer.push_sample(sample);
                        }
                    }

                    if sess.capture.ReleaseBuffer(num_frames).is_err() {
                        invalidate = true;
                        break;
                    }
                }
            }
        }

        if invalidate {
            session = None;
            analyzer.reset();
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        // When Windows is silent, WASAPI loopback produces 0 packets; push silence so the FFT decays cleanly.
        if got_frames == 0 {
            let silent_samples = ((sample_rate as f32) * (TICK_MS as f32 / 1000.0)) as usize;
            analyzer.push_silence(silent_samples);
        }

        let (bands, beat) = analyzer.process(sample_rate);
        let is_zero = beat == 0.0 && bands.iter().all(|&v| v == 0.0);

        if !is_zero || !sent_zero {
            let rounded: Vec<f32> = bands.iter().map(|&v| (v * 100.0).round() / 100.0).collect();
            let _ = app.emit_to(
                "notch",
                "audio-viz",
                AudioVizFrame {
                    bands: rounded,
                    beat: (beat * 100.0).round() / 100.0,
                },
            );
            sent_zero = is_zero;
        }

        thread::sleep(Duration::from_millis(TICK_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_detects_sine_bin() {
        let mut re = vec![0.0f32; FFT_SIZE];
        let mut im = vec![0.0f32; FFT_SIZE];
        let bin = 16usize;
        for i in 0..FFT_SIZE {
            re[i] = (2.0 * PI * bin as f32 * i as f32 / FFT_SIZE as f32).sin();
        }
        fft_in_place(&mut re, &mut im);
        let peak_bin = (0..(FFT_SIZE / 2))
            .max_by(|&a, &b| {
                let ma = re[a] * re[a] + im[a] * im[a];
                let mb = re[b] * re[b] + im[b] * im[b];
                ma.total_cmp(&mb)
            })
            .unwrap();
        assert_eq!(peak_bin, bin);
    }

    #[test]
    fn analyzer_separates_bass_from_treble_and_decays_on_silence() {
        let sr = 48_000u32;
        let mut analyzer = Analyzer::new();

        // Feed 75 Hz bass tone
        for i in 0..FFT_SIZE {
            let s = 0.5 * (2.0 * PI * 75.0 * i as f32 / sr as f32).sin();
            analyzer.push_sample(s);
        }
        let (bass_bands, beat) = analyzer.process(sr);
        let low_peak = bass_bands[0..5].iter().copied().fold(0.0f32, f32::max);
        let high_peak = bass_bands[16..].iter().copied().fold(0.0f32, f32::max);
        assert!(low_peak > 0.5, "expected strong low bands, got {low_peak}");
        assert!(low_peak > high_peak * 2.0, "bass leaked to highs: {low_peak} vs {high_peak}");
        assert!(beat > 0.0, "expected beat impulse on sudden bass onset");

        // Feed 5000 Hz treble tone
        analyzer.reset();
        for i in 0..FFT_SIZE {
            let s = 0.5 * (2.0 * PI * 5000.0 * i as f32 / sr as f32).sin();
            analyzer.push_sample(s);
        }
        let (treble_bands, _) = analyzer.process(sr);
        let low_peak_t = treble_bands[0..5].iter().copied().fold(0.0f32, f32::max);
        let high_peak_t = treble_bands[16..].iter().copied().fold(0.0f32, f32::max);
        assert!(high_peak_t > 0.5, "expected strong high bands, got {high_peak_t}");
        assert!(high_peak_t > low_peak_t * 2.0, "treble leaked to lows: {high_peak_t} vs {low_peak_t}");

        // Feed silence until smoothed decays
        for _ in 0..30 {
            analyzer.push_silence(FFT_SIZE);
            let _ = analyzer.process(sr);
        }
        let (silent_bands, silent_beat) = analyzer.process(sr);
        assert!(silent_bands.iter().all(|&v| v == 0.0), "expected silence to decay to 0");
        assert_eq!(silent_beat, 0.0);
    }
}
