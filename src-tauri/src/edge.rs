use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, PhysicalPosition, PhysicalSize, State, WebviewWindow};

/// Seberapa tipis zona pemicu di tepi layar (DIP).
const EDGE_TRIGGER_DIP: f64 = 2.0;
/// Jeda sebelum panel menghilang setelah kursor menjauh.
const HIDE_DELAY: Duration = Duration::from_millis(400);
/// Kursor harus diam di tepi selama ini sebelum panel muncul (mencegah muncul tidak sengaja).
const EDGE_DWELL: Duration = Duration::from_millis(200);
/// Panel yang dibuka lewat klik tapi tidak pernah dimasuki kursor ditutup setelah ini.
const PIN_GRACE: Duration = Duration::from_secs(4);
/// Skala seluruh UI: ukuran window dan isinya. 1.0 = ukuran desain asli.
/// Kecilkan kalau terasa terlalu besar, besarkan kalau terlalu kecil.
pub const UI_SCALE: f64 = 0.8;
/// Jarak bar dari tepi atas dan bawah layar (dalam satuan desain, sebelum UI_SCALE).
const BAR_INSET_DIP: f64 = 12.0;
const POLL_INTERVAL: Duration = Duration::from_millis(30);
/// Seberapa sering ukuran monitor dibaca ulang.
const METRICS_REFRESH: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub enum Edge {
    Left,
    Top,
    Bottom,
    Right,
}

/// Deskripsi satu panel. Menambah panel baru = menambah satu spec.
#[derive(Clone, Copy)]
pub struct PanelSpec {
    pub label: &'static str,
    pub edge: Edge,
    /// Ukuran window dalam DIP (lebar, tinggi).
    /// Untuk Edge::Left tinggi diabaikan (setinggi layar).
    pub size: (f64, f64),
    /// Panjang zona pemicu di tepi, dalam DIP, di tengah tepi.
    /// None = sepanjang sisi/lebar window.
    pub trigger_len: Option<f64>,
    /// true = muncul saat kursor menyentuh tepi dan hilang saat kursor menjauh.
    /// false = hanya mengikuti flag (hotkey, tombol, atau perintah dari React).
    pub hover: bool,
    /// true = window mengambil fokus keyboard saat muncul.
    pub focus: bool,
}

/// Panel yang dibuka lewat klik di UI (bukan hotkey/tray): menutup sendiri saat kursor menjauh.
static TRANSIENT: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn is_transient(label: &str) -> bool {
    TRANSIENT
        .lock()
        .map(|t| t.iter().any(|l| *l == label))
        .unwrap_or(false)
}

fn set_transient_flag(label: &'static str, on: bool) {
    if let Ok(mut t) = TRANSIENT.lock() {
        t.retain(|l| *l != label);
        if on {
            t.push(label);
        }
    }
}

/// Flag "dibuka paksa" per panel (hotkey, tray, atau klik tombol).
pub struct Panels(pub HashMap<&'static str, Arc<AtomicBool>>);

impl Panels {
    /// Hotkey dan tray: terbuka menetap sampai ditutup lagi.
    pub fn toggle(&self, label: &str) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            set_transient_flag(*key, false);
            flag.fetch_xor(true, Ordering::Relaxed);
        }
    }

    pub fn set(&self, label: &str, open: bool) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            if !open {
                set_transient_flag(*key, false);
            }
            flag.store(open, Ordering::Relaxed);
        }
    }

    /// Klik di UI: terbuka, lalu menutup sendiri saat kursor menjauh.
    pub fn set_transient(&self, label: &str) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            set_transient_flag(*key, true);
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_open(&self, label: &str) -> bool {
        self.0
            .get(label)
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
}

/// Dipanggil dari tombol di bar: buka sementara, atau tutup kalau sudah terbuka.
#[tauri::command]
pub fn toggle_panel(label: String, panels: State<'_, Panels>) {
    if panels.is_open(&label) {
        panels.set(&label, false);
    } else {
        panels.set_transient(&label);
    }
}

/// invoke("set_panel", { label, open }). open = true membuka sementara.
#[tauri::command]
pub fn set_panel(label: String, open: bool, panels: State<'_, Panels>) {
    if open {
        panels.set_transient(&label);
    } else {
        panels.set(&label, false);
    }
}

struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// (x, y, lebar, tinggi, scale) monitor utama dalam pixel fisik.
type Metrics = (f64, f64, f64, f64, f64);

/// Hitung (rect window, rect zona pemicu) dalam pixel fisik.
fn layout(spec: &PanelSpec, m: Metrics) -> (Rect, Rect) {
    let (mx, my, mw, mh, scale) = m;
    // 1 satuan desain = u pixel fisik (skala DPI monitor dikali UI_SCALE).
    let u = scale * UI_SCALE;

    match spec.edge {
        Edge::Left => {
            let inset = BAR_INSET_DIP * u;
            let win = Rect { x: mx, y: my + inset, w: spec.size.0 * u, h: mh - 2.0 * inset };
            let trig = Rect { x: mx, y: my, w: EDGE_TRIGGER_DIP * scale, h: mh };
            (win, trig)
        }
        Edge::Top => {
            let w = spec.size.0 * u;
            let h = spec.size.1 * u;
            let win = Rect { x: mx + (mw - w) / 2.0, y: my, w, h };
            let tw = spec.trigger_len.map(|l| l * u).unwrap_or(w);
            let trig = Rect {
                x: mx + (mw - tw) / 2.0,
                y: my,
                w: tw,
                h: EDGE_TRIGGER_DIP * scale,
            };
            (win, trig)
        }
        Edge::Bottom => {
            let w = spec.size.0 * u;
            let h = spec.size.1 * u;
            let win = Rect { x: mx + (mw - w) / 2.0, y: my + mh - h, w, h };
            let tw = spec.trigger_len.map(|l| l * u).unwrap_or(w);
            let trig = Rect {
                x: mx + (mw - tw) / 2.0,
                y: my + mh - EDGE_TRIGGER_DIP * scale,
                w: tw,
                h: EDGE_TRIGGER_DIP * scale,
            };
            (win, trig)
        }
        Edge::Right => {
            let w = spec.size.0 * u;
            let h = spec.size.1 * u;
            let win = Rect { x: mx + mw - w, y: my + (mh - h) / 2.0, w, h };
            let trig = Rect {
                x: mx + mw - EDGE_TRIGGER_DIP * scale,
                y: win.y,
                w: EDGE_TRIGGER_DIP * scale,
                h,
            };
            (win, trig)
        }
    }
}

fn monitor_metrics(window: &WebviewWindow) -> Option<Metrics> {
    let m = window.primary_monitor().ok()??;
    Some((
        m.position().x as f64,
        m.position().y as f64,
        m.size().width as f64,
        m.size().height as f64,
        m.scale_factor(),
    ))
}

/// Taruh window di posisi dan ukuran yang benar.
pub fn place(window: &WebviewWindow, spec: &PanelSpec) -> tauri::Result<()> {
    if let Some(m) = monitor_metrics(window) {
        let (win, _) = layout(spec, m);
        window.set_size(PhysicalSize::new(win.w.round() as u32, win.h.round() as u32))?;
        window.set_position(PhysicalPosition::new(win.x.round() as i32, win.y.round() as i32))?;
    }
    Ok(())
}

/// Window tidak mengambil fokus saat diklik, jadi mengklik bar atau notch
/// tidak memutus ketikan di aplikasi yang sedang aktif.
pub fn set_no_activate(window: &WebviewWindow) {
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_NOACTIVATE},
    };

    // hwnd() dari Tauri bertipe HWND milik versi crate `windows` yang berbeda dari kita,
    // jadi dikonversi lewat nilai pointer-nya.
    if let Ok(h) = window.hwnd() {
        let hwnd = HWND(h.0 as isize as *mut c_void);
        unsafe {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_NOACTIVATE.0) as i32);
        }
    }
}

/// Ubah window di main thread: atur ulang posisi, click-through, fokus, dan beri tahu React.
fn set_visible(app: &AppHandle, window: &WebviewWindow, event: &str, show: bool, spec: &PanelSpec) {
    let window = window.clone();
    let event = event.to_string();
    let spec = *spec;
    let _ = app.run_on_main_thread(move || {
        if show {
            // Layar bisa berubah (mati/nyala, resolusi, dock), jadi atur ulang tiap kali muncul.
            let _ = place(&window, &spec);
            let _ = window.set_always_on_top(true);
        }
        let _ = window.set_ignore_cursor_events(!show);
        if show && spec.focus {
            let _ = window.set_focus();
        }
        let _ = window.emit(event.as_str(), show);
    });
}

struct Runtime {
    window: WebviewWindow,
    spec: PanelSpec,
    pinned: Arc<AtomicBool>,
    event: String,
    visible: bool,
    outside_since: Option<Instant>,
    inside_since: Option<Instant>,
    /// Kursor pernah masuk ke window saat panel dibuka lewat klik.
    pin_entered: bool,
}

/// Satu thread untuk semua panel.
/// Memancarkan event "edge-visible:<label>" (payload bool) ke React.
pub fn start_all(app: AppHandle, items: Vec<(WebviewWindow, PanelSpec, Arc<AtomicBool>)>) {
    thread::spawn(move || {
        let mut panels: Vec<Runtime> = items
            .into_iter()
            .map(|(window, spec, pinned)| Runtime {
                event: format!("edge-visible:{}", spec.label),
                window,
                spec,
                pinned,
                visible: false,
                outside_since: None,
                inside_since: None,
                pin_entered: false,
            })
            .collect();
        if panels.is_empty() {
            return;
        }

        // Satu window dipakai untuk membaca kursor dan monitor.
        let probe = panels[0].window.clone();
        let mut metrics = monitor_metrics(&probe);
        let mut last_refresh = Instant::now();

        loop {
            thread::sleep(POLL_INTERVAL);

            if last_refresh.elapsed() >= METRICS_REFRESH {
                metrics = monitor_metrics(&probe).or(metrics);
                last_refresh = Instant::now();
            }
            let Some(m) = metrics else { continue };
            let Ok(cursor) = probe.cursor_position() else { continue };

            for p in panels.iter_mut() {
                let pinned = p.pinned.load(Ordering::Relaxed);

                // Panel tanpa hover: hanya mengikuti flag.
                if !p.spec.hover {
                    if pinned != p.visible {
                        p.visible = pinned;
                        set_visible(&app, &p.window, &p.event, pinned, &p.spec);
                    }
                    continue;
                }

                // Dibuka paksa: abaikan posisi kursor, kecuali panel yang dibuka lewat klik.
                if pinned {
                    if !p.visible {
                        p.visible = true;
                        p.pin_entered = false;
                        p.outside_since = None;
                        set_visible(&app, &p.window, &p.event, true, &p.spec);
                    }
                    if is_transient(p.spec.label) {
                        let (win_rect, _) = layout(&p.spec, m);
                        if win_rect.contains(cursor.x, cursor.y) {
                            p.pin_entered = true;
                            p.outside_since = None;
                        } else {
                            let since = *p.outside_since.get_or_insert_with(Instant::now);
                            let limit = if p.pin_entered { HIDE_DELAY } else { PIN_GRACE };
                            if since.elapsed() >= limit {
                                // Lepas kuncinya; iterasi berikutnya menyembunyikan panel.
                                p.pinned.store(false, Ordering::Relaxed);
                                set_transient_flag(p.spec.label, false);
                            }
                        }
                    } else {
                        p.outside_since = None;
                    }
                    continue;
                }

                let (win_rect, trigger_rect) = layout(&p.spec, m);

                if !p.visible {
                    if trigger_rect.contains(cursor.x, cursor.y) {
                        let since = *p.inside_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= EDGE_DWELL {
                            p.visible = true;
                            p.inside_since = None;
                            p.outside_since = None;
                            set_visible(&app, &p.window, &p.event, true, &p.spec);
                        }
                    } else {
                        p.inside_since = None;
                    }
                } else if win_rect.contains(cursor.x, cursor.y) {
                    p.outside_since = None;
                } else {
                    let since = *p.outside_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= HIDE_DELAY {
                        p.visible = false;
                        p.outside_since = None;
                        set_visible(&app, &p.window, &p.event, false, &p.spec);
                    }
                }
            }
        }
    });
}
