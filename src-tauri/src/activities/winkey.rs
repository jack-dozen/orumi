//! Pemicu "Win key sendirian".
//!
//! Sumber utamanya BUKAN hook, melainkan pemantau (`poller`) yang membaca status tombol dari
//! Windows (GetAsyncKeyState) tiap ~8 ms. Hook keyboard tingkat rendah ternyata bisa tidak
//! menerima tombol selama beberapa ratus ms sampai detik setelah Activities dibuka/ditutup,
//! dan itu membuat penyamaran Start menu terlewat saat Win ditekan beruntun.
//!
//! - Semua event Win dan kombinasinya SELALU diteruskan apa adanya, jadi Win+E, Win+Panah,
//!   dst. tetap bekerja dan tetap terlihat oleh program lain (mis. window manager).
//! - Start menu dicegah dengan "penyamaran": begitu Win mulai ditekan, satu tombol netral
//!   (VK 0xE8) disuntikkan SELAGI Win masih ditahan. Windows lalu menganggap Win dipakai
//!   dalam kombinasi dan tidak membuka Start saat Win dilepas.
//! - Kalau Activities sedang terbuka saat Win ditekan, Activities langsung disembunyikan di
//!   Win-down. Win dilepas tanpa tombol/klik lain di antaranya = ketukan: toggle Activities.
//! - Hook tetap dipasang sebagai pelengkap: ia menyuntik penyamaran seketika (kalau sedang
//!   hidup) dan menangkap tombol lain yang ditekan sangat singkat saat Win ditahan.
//!
//! Batasan: kalau window yang fokus berjalan sebagai administrator, Windows bisa tidak
//! memberi tahu status tombolnya ke program non-admin; jalankan ORUMI sebagai administrator
//! kalau perlu.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Sender},
        OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
            GetWindowTextW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
            KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
        },
    },
};

const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
const LLKHF_INJECTED: u32 = 0x10;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// Tombol netral untuk penyamaran (tidak dipakai Windows; sama dengan altswitch.rs).
const VK_NEUTRAL: u32 = 0xE8;
/// Interval pemantau.
const POLL_MS: u64 = 8;
/// Penyamaran dari hook dan pemantau untuk Win-down yang sama dijadikan satu.
const MASK_DEDUPE_MS: u32 = 60;

/// true = cetak log diagnostik ke stderr (lihat konsol `npm run tauri dev`).
const DEBUG: bool = true;

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Hook melihat tombol lain (atau yang lain) ditekan selagi Win ditahan. Dihapus pemantau
/// tiap kali sebuah tekanan Win selesai.
static COMBO: AtomicBool = AtomicBool::new(false);
/// Waktu (ms sejak start) penyamaran terakhir.
static LAST_MASK: AtomicU32 = AtomicU32::new(0);
static T0: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u32 {
    T0.get_or_init(Instant::now).elapsed().as_millis() as u32
}

enum Msg {
    /// Ketukan Win sendirian: toggle Activities.
    Tap,
    /// Win baru ditekan: suntikkan tombol netral agar Start menu tidak terbuka.
    Mask,
    /// Win ditekan saat Activities terbuka: tutup sekarang juga.
    Dismiss,
    /// Baris log (dicetak oleh pekerja; I/O di dalam callback hook bisa memicu timeout).
    Log(String),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(m: Msg) -> bool {
    TX.get().map(|tx| tx.send(m).is_ok()).unwrap_or(false)
}

fn log(line: String) {
    if DEBUG {
        send(Msg::Log(line));
    }
}

/// Status fisik tombol menurut Windows. Di dalam callback hook, nilainya adalah keadaan
/// SEBELUM event yang sedang diproses.
unsafe fn async_down(vk: u32) -> bool {
    (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

// ---------- Hook (pelengkap) ----------

unsafe fn handle(msg: usize, kb: &KBDLLHOOKSTRUCT) {
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !(down || up) || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // Event suntikan (termasuk milik kita sendiri dan altswitch) tidak dihitung.
    if kb.flags.0 & LLKHF_INJECTED != 0 {
        return;
    }

    let vk = kb.vkCode;
    if vk == VK_LWIN || vk == VK_RWIN {
        // Tekanan baru (bukan auto-repeat) = Win belum tertahan menurut Windows.
        if down && !async_down(vk) {
            send(Msg::Mask);
            log(format!("t={} hook: win-down vk={vk:#x}", kb.time));
        }
        return;
    }
    if down && (async_down(VK_LWIN) || async_down(VK_RWIN)) && !COMBO.swap(true, Ordering::Relaxed)
    {
        log(format!("t={} hook: tombol lain vk={vk:#x} saat Win ditahan", kb.time));
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        handle(wparam.0, &*(lparam.0 as *const KBDLLHOOKSTRUCT));
    }
    CallNextHookEx(None, code, wparam, lparam) // tidak pernah menelan event
}

// ---------- Pemantau (sumber utama) ----------

fn poller() {
    unsafe {
        // Harus tetap jalan walau CPU sedang sibuk (mis. saat Activities mengambil thumbnail).
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let mut was = false;
    let mut dismissed = false;
    let mut polled_combo = false;
    let mut base = [false; 256];

    loop {
        thread::sleep(Duration::from_millis(POLL_MS));
        if !ENABLED.load(Ordering::Relaxed) {
            was = false;
            continue;
        }
        let win = unsafe { async_down(VK_LWIN) || async_down(VK_RWIN) };

        if win && !was {
            // Tekanan Win baru. Catat tombol/mouse yang sudah tertahan lebih dulu.
            for vk in 1..256u32 {
                base[vk as usize] = unsafe { async_down(vk) };
            }
            polled_combo = false;
            dismissed = super::is_open();
            send(Msg::Mask);
            if dismissed {
                send(Msg::Dismiss);
            }
            log(format!("pemantau: win-down, activities_terbuka={dismissed}"));
        } else if win && !polled_combo {
            // Selagi Win ditahan: tombol keyboard atau klik mouse baru = kombinasi.
            for vk in 1..256u32 {
                if vk == VK_LWIN || vk == VK_RWIN || vk == VK_NEUTRAL {
                    continue;
                }
                let d = unsafe { async_down(vk) };
                if d && !base[vk as usize] {
                    polled_combo = true;
                    log(format!("pemantau: tombol/klik lain vk={vk:#x} saat Win ditahan"));
                    break;
                }
                if !d {
                    base[vk as usize] = false;
                }
            }
        } else if !win && was {
            let combo = COMBO.swap(false, Ordering::Relaxed) || polled_combo;
            let tap = !combo && !dismissed;
            log(format!(
                "pemantau: win-up, kombinasi={combo}, ditutup_saat_down={dismissed} -> {}",
                if tap { "TAP (toggle)" } else { "tidak ada aksi" }
            ));
            if tap {
                send(Msg::Tap);
            }
        }
        was = win;
    }
}

// ---------- Penyamaran, log, pekerja ----------

/// Suntikkan tombol netral (turun lalu naik) selagi Win masih ditahan.
unsafe fn inject_mask() {
    let t = now_ms();
    let last = LAST_MASK.load(Ordering::Relaxed);
    if last != 0 && t.wrapping_sub(last) < MASK_DEDUPE_MS {
        return;
    }
    LAST_MASK.store(t.max(1), Ordering::Relaxed);

    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_NEUTRAL as u16),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    let sent = SendInput(&inputs, size_of::<INPUT>() as i32);
    if DEBUG {
        log(format!("penyamaran: SendInput mengirim {sent} dari 2 event"));
    }
}

unsafe fn foreground_desc() -> String {
    let h = GetForegroundWindow();
    let mut class = [0u16; 128];
    let mut title = [0u16; 128];
    let c = (GetClassNameW(h, &mut class).max(0)) as usize;
    let t = (GetWindowTextW(h, &mut title).max(0)) as usize;
    format!(
        "{} | {}",
        String::from_utf16_lossy(&class[..c]),
        String::from_utf16_lossy(&title[..t])
    )
}

/// Pasang hook, jalankan pemantau dan thread pekerja.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let _ = TX.set(tx);
    let _ = T0.get_or_init(Instant::now);

    thread::spawn(move || {
        for msg in rx {
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match msg {
                Msg::Log(line) => eprintln!("[winkey] {line}"),
                Msg::Mask => unsafe { inject_mask() },
                Msg::Dismiss => super::dismiss(&app),
                Msg::Tap => {
                    super::toggle(&app);
                    if DEBUG {
                        // Kalau Start menu lolos, window depan sesudah ketukan adalah miliknya.
                        thread::spawn(|| {
                            thread::sleep(Duration::from_millis(450));
                            eprintln!("[winkey] window depan 450 ms sesudah ketukan: {}", unsafe {
                                foreground_desc()
                            });
                        });
                    }
                }
            }));
            if done.is_err() {
                eprintln!("[winkey] pekerja panic pada satu pesan; dilanjutkan");
            }
        }
    });

    thread::spawn(poller);

    thread::spawn(|| unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) else {
            eprintln!("[activities] gagal memasang hook (pemantau tetap berjalan)");
            return;
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    });
}
