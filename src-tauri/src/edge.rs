use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

/// Thickness of the screen-edge trigger zone (DIP).
const EDGE_TRIGGER_DIP: f64 = 2.0;
/// Delay before a panel hides after the cursor moves away.
const HIDE_DELAY: Duration = Duration::from_millis(400);
/// How long the cursor must remain at the edge before a panel appears (prevents accidental activation).
const EDGE_DWELL: Duration = Duration::from_millis(200);
/// A panel opened by clicking but never entered by the cursor closes after this delay.
const PIN_GRACE: Duration = Duration::from_secs(6);
/// Cursor monitor interval: fast near an edge or while a panel is visible, slow while idle.
const POLL_FAST: Duration = Duration::from_millis(30);
const POLL_IDLE: Duration = Duration::from_millis(150);
/// true = print how long the switcher takes to show on the main thread (turn off when not needed).
const DEBUG_SHOW: bool = false;

/// The edge thread, so opening or closing a panel can wake it from its polling sleep instead of
/// waiting for the interval to end (up to POLL_IDLE).
static EDGE_THREAD: OnceLock<thread::Thread> = OnceLock::new();

fn wake() {
    if let Some(t) = EDGE_THREAD.get() {
        t.unpark();
    }
}

fn left_mouse_down() -> bool {
    unsafe {
        (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x01) as u16) & 0x8000 != 0
    }
}

/// Inject a neutral key (VK 0xE8) while Alt is held so releasing Alt on the newly focused
/// panel does not activate Windows' menu loop (`SC_KEYMENU`) or beep on the next keystroke.
fn mask_alt_if_down() {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    unsafe {
        if (GetAsyncKeyState(0x12) as u16) & 0x8000 == 0 {
            return;
        }
        let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0xE8),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}
/// "Near the edge" = within this distance of the screen edge (design units).
const NEAR_EDGE_DIP: f64 = 96.0;
/// How often to reread monitor dimensions.
const METRICS_REFRESH: Duration = Duration::from_secs(5);
/// Bar distance from the top and bottom screen edges (design units, before UI scaling).
const BAR_INSET_DIP: f64 = 16.0;
/// Inner corner radius of the Caelestia screen border and full-span bar fillets (design units).
pub const BORDER_RADIUS_DIP: f64 = 24.0;
/// Off-screen bleed on each side of the `"border"` window (design units) so any Windows 11 DWM
/// outer-corner rounding sits completely outside the visible monitor area.
const BORDER_BLEED_DIP: f64 = 16.0;

// ---------- Screen border (can be changed at runtime) ----------

static SCREEN_BORDER: AtomicBool = AtomicBool::new(false);
static SCREEN_BORDER_SIZE: AtomicU8 = AtomicU8::new(10);

pub fn is_screen_border() -> bool {
    SCREEN_BORDER.load(Ordering::Relaxed)
}

pub fn screen_border_dip() -> f64 {
    if is_screen_border() {
        SCREEN_BORDER_SIZE.load(Ordering::Relaxed).clamp(6, 16) as f64
    } else {
        0.0
    }
}

pub fn set_screen_border(enabled: bool, size: u8) {
    SCREEN_BORDER.store(enabled, Ordering::Relaxed);
    SCREEN_BORDER_SIZE.store(size.clamp(6, 16), Ordering::Relaxed);
    wake();
}

/// Show/position or hide the click-through fullscreen `"border"` window on the main thread,
/// keeping every ORUMI panel strictly above it in the topmost Z-order band.
pub fn sync_border_window(app: &AppHandle, show: bool) {
    use std::mem::size_of;
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWCP_DONOTROUND, DWMWA_BORDER_COLOR,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE,
        },
        UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_STYLE, HWND_TOPMOST, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, WS_BORDER, WS_CAPTION,
            WS_DLGFRAME, WS_POPUP, WS_THICKFRAME,
        },
    };

    let app_handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(border) = app_handle.get_webview_window("border") else {
            return;
        };
        set_activatable(&border, false);
        let _ = border.set_ignore_cursor_events(true);
        if show {
            let _ = border.set_zoom(ui_scale());
            let border_hwnd = border.hwnd().ok().map(|raw| HWND(raw.0 as isize as *mut c_void));
            let metrics = monitor_metrics(&border);
            if let (Some((mx, my, mw, mh, scale)), Some(hwnd)) = (metrics, border_hwnd) {
                let bleed = (BORDER_BLEED_DIP * scale * ui_scale()).round();
                let x = (mx - bleed).round() as i32;
                let y = (my - bleed).round() as i32;
                let w = (mw + 2.0 * bleed).round() as i32;
                let h = (mh + 2.0 * bleed).round() as i32;
                unsafe {
                    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                    let clean = (style
                        & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_BORDER.0 | WS_DLGFRAME.0))
                        | WS_POPUP.0;
                    if clean != style {
                        SetWindowLongW(hwnd, GWL_STYLE, clean as i32);
                    }
                    let pref = DWMWCP_DONOTROUND;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE,
                        &pref as *const DWM_WINDOW_CORNER_PREFERENCE as *const c_void,
                        size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
                    );
                    let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_BORDER_COLOR,
                        &no_border as *const u32 as *const c_void,
                        size_of::<u32>() as u32,
                    );
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        w,
                        h,
                        SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
            }
            let _ = border.show();
            if let Some(hwnd) = border_hwnd {
                unsafe {
                    let pref = DWMWCP_DONOTROUND;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE,
                        &pref as *const DWM_WINDOW_CORNER_PREFERENCE as *const c_void,
                        size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
                    );
                }
            }
            // Keep all ORUMI panels above the border in Z-order so sliding panels & bar fillets cover border corners.
            for label in [
                "bar",
                "overview",
                "notch",
                "launcher",
                "overlay",
                "switcher",
                "preview",
                "activities",
            ] {
                if let Some(w) = app_handle.get_webview_window(label) {
                    if let Ok(raw) = w.hwnd() {
                        let hwnd = HWND(raw.0 as isize as *mut c_void);
                        unsafe {
                            let _ = SetWindowPos(
                                hwnd,
                                Some(HWND_TOPMOST),
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
            }
            let _ = border.emit("border-visible", true);
        } else {
            let _ = border.emit("border-visible", false);
            let _ = border.hide();
        }
    });
}

// ---------- UI scale (can be changed at runtime) ----------

static UI_SCALE_BITS: AtomicU64 = AtomicU64::new(0);

/// Scale for the entire UI: window dimensions and contents. 1.0 = original design size.
pub fn ui_scale() -> f64 {
    let bits = UI_SCALE_BITS.load(Ordering::Relaxed);
    if bits == 0 {
        0.8
    } else {
        f64::from_bits(bits)
    }
}

pub fn set_ui_scale(v: f64) {
    UI_SCALE_BITS.store(v.to_bits(), Ordering::Relaxed);
}

// ---------- Runtime panel preferences ----------

static PANEL_PREFS: Mutex<Option<crate::config::PanelsConfig>> = Mutex::new(None);

pub fn set_panel_prefs(prefs: crate::config::PanelsConfig) {
    if let Ok(mut p) = PANEL_PREFS.lock() {
        *p = Some(prefs);
    }
    wake();
}

pub fn panel_prefs() -> crate::config::PanelsConfig {
    PANEL_PREFS
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .unwrap_or_default()
}

fn parse_edge(s: &str, fallback: Edge) -> Edge {
    match s {
        "left" => Edge::Left,
        "right" => Edge::Right,
        "top" => Edge::Top,
        "bottom" => Edge::Bottom,
        _ => fallback,
    }
}

fn default_edge_for(label: &str) -> Edge {
    match label {
        "bar" | "overview" => Edge::Left,
        "notch" => Edge::Top,
        "launcher" | "switcher" => Edge::Bottom,
        "overlay" => Edge::Right,
        _ => Edge::Free,
    }
}

pub fn effective_edge(label: &str, fallback: Edge) -> Edge {
    let p = panel_prefs();
    match label {
        "bar" => parse_edge(&p.bar.edge, fallback),
        "notch" => parse_edge(&p.notch.edge, fallback),
        "launcher" => parse_edge(&p.launcher.edge, fallback),
        "overlay" => parse_edge(&p.overlay.edge, fallback),
        _ => fallback,
    }
}

pub fn is_panel_enabled(label: &str) -> bool {
    let p = panel_prefs();
    match label {
        "bar" | "preview" => p.bar.enabled,
        "notch" => p.notch.enabled,
        "launcher" => true,
        "overlay" => p.overlay.enabled,
        "overview" => p.overview,
        "switcher" => p.switcher,
        _ => true,
    }
}

pub fn is_panel_hotedge(label: &str) -> bool {
    let p = panel_prefs();
    match label {
        "bar" => p.bar.enabled && p.bar.hotedge,
        "notch" => p.notch.enabled && p.notch.hotedge,
        "launcher" => p.launcher.hotedge,
        "overlay" => p.overlay.enabled && p.overlay.hotedge,
        _ => false,
    }
}

// ---------- Panels ----------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Bottom,
    Right,
    /// Position and size are set by code through set_free_rect (e.g. thumbnail previews).
    Free,
}

/// Description of one panel. Add a new spec to add a panel.
#[derive(Clone, Copy)]
pub struct PanelSpec {
    pub label: &'static str,
    pub edge: Edge,
    /// Window size in design units (width, height).
    /// For Edge::Left, height is ignored (full screen height).
    pub size: (f64, f64),
    /// Length of the trigger zone centered along the edge. None = the full edge/window width.
    pub trigger_len: Option<f64>,
    /// true = appears when the cursor touches the edge and hides when it moves away.
    /// false = follows the flag only (hotkey, button, or command from React).
    pub hover: bool,
    /// true = the window takes keyboard focus when shown.
    pub focus: bool,
}

fn flag_set(list: &Mutex<Vec<&'static str>>, label: &'static str, on: bool) {
    if let Ok(mut l) = list.lock() {
        l.retain(|x| *x != label);
        if on {
            l.push(label);
        }
    }
}

fn flag_has(list: &Mutex<Vec<&'static str>>, label: &str) -> bool {
    list.lock()
        .map(|l| l.iter().any(|x| *x == label))
        .unwrap_or(false)
}

/// Panel opened by clicking in the UI (not via hotkey/tray): hides when the cursor moves away.
static TRANSIENT: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Panel currently used for typing: do not close automatically.
static HELD: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Hover panel explicitly dismissed via `set_panel(..., false)`: hides even while cursor is inside.
static DISMISSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Temporary window size that replaces PanelSpec.size (e.g. the launcher while the wallpaper picker is open).
static SIZE_OVERRIDE: Mutex<Vec<(&'static str, (f64, f64))>> = Mutex::new(Vec::new());

/// Launcher size (design units) while showing the wallpaper picker: wide and short.
const WALLPAPER_VIEW_SIZE: (f64, f64) = (1080.0, 300.0);
/// Launcher size (design units) while showing the WM keyboard shortcuts cheatsheet.
const CHEATSHEET_VIEW_SIZE: (f64, f64) = (1120.0, 620.0);
static LAUNCHER_CHEATSHEET: AtomicBool = AtomicBool::new(false);
/// true = the launcher was opened by bottom-edge hover (no initial focus, auto-closes on leave).
static LAUNCHER_HOVER: AtomicBool = AtomicBool::new(false);
/// true = the launcher was opened with the full app list forced (e.g. via Win key), bypassing compact mode.
static LAUNCHER_FORCE_LIST: AtomicBool = AtomicBool::new(false);

pub fn launcher_is_cheatsheet() -> bool {
    LAUNCHER_CHEATSHEET.load(Ordering::Relaxed)
}

fn reset_launcher_override(label: &str) {
    if label == "launcher" {
        LAUNCHER_CHEATSHEET.store(false, Ordering::Relaxed);
        LAUNCHER_FORCE_LIST.store(false, Ordering::Relaxed);
        set_size_override("launcher", None);
    }
}

fn size_override(label: &str) -> Option<(f64, f64)> {
    SIZE_OVERRIDE
        .lock()
        .ok()
        .and_then(|l| l.iter().find(|(k, _)| *k == label).map(|(_, s)| *s))
}

pub fn set_size_override(label: &'static str, size: Option<(f64, f64)>) {
    if let Ok(mut l) = SIZE_OVERRIDE.lock() {
        l.retain(|(k, _)| *k != label);
        if let Some(s) = size {
            l.push((label, s));
        }
    }
}

/// Position and size of a "free" window in physical pixels: (x, y, width, height).
static FREE_RECT: Mutex<Vec<(&'static str, (f64, f64, f64, f64))>> = Mutex::new(Vec::new());

fn free_rect(label: &str) -> Option<(f64, f64, f64, f64)> {
    FREE_RECT
        .lock()
        .ok()
        .and_then(|l| l.iter().find(|(k, _)| *k == label).map(|(_, r)| *r))
}

pub fn set_free_rect(label: &'static str, rect: Option<(f64, f64, f64, f64)>) {
    if let Ok(mut l) = FREE_RECT.lock() {
        l.retain(|(k, _)| *k != label);
        if let Some(r) = rect {
            l.push((label, r));
        }
    }
}

/// Logical hit area (physical px: x, y, w, h) of a free window, for windows that are larger than
/// what they show (the preview window is bigger than its card so the card can move inside it).
static HIT_RECT: Mutex<Vec<(&'static str, (f64, f64, f64, f64))>> = Mutex::new(Vec::new());

pub fn hit_rect(label: &str) -> Option<(f64, f64, f64, f64)> {
    HIT_RECT
        .lock()
        .ok()
        .and_then(|l| l.iter().find(|(k, _)| *k == label).map(|(_, r)| *r))
}

pub fn set_hit_rect(label: &'static str, rect: Option<(f64, f64, f64, f64)>) {
    if let Ok(mut l) = HIT_RECT.lock() {
        l.retain(|(k, _)| *k != label);
        if let Some(r) = rect {
            l.push((label, r));
        }
    }
}

/// Launcher view changed: "wallpaper" or "cheatsheet" overrides panel size; otherwise restore default size.
#[tauri::command]
pub fn launcher_view(view: String, app: AppHandle) {
    LAUNCHER_CHEATSHEET.store(view == "cheatsheet", Ordering::Relaxed);
    let next = match view.as_str() {
        "wallpaper" => Some(WALLPAPER_VIEW_SIZE),
        "cheatsheet" => Some(CHEATSHEET_VIEW_SIZE),
        _ => None,
    };
    set_size_override("launcher", next);
    let a = app.clone();
    let _ = app.run_on_main_thread(move || crate::place_panel(&a, "launcher"));
}

/// Active launcher card size (width, height) in design units (used for hover auto-close hit-test).
static LAUNCHER_SIZE: Mutex<Option<(f64, f64)>> = Mutex::new(None);

#[tauri::command]
pub fn launcher_set_size(width: f64, height: f64) {
    if let Ok(mut s) = LAUNCHER_SIZE.lock() {
        *s = (width > 0.0 && height > 0.0).then_some((width, height));
    }
    wake();
}

/// Active notch content size (width, height) in design units (used to size its hover hit area).
static NOTCH_SIZE: Mutex<Option<(Option<f64>, Option<f64>)>> = Mutex::new(None);

#[tauri::command]
pub fn notch_set_height(height: f64, width: Option<f64>) {
    if let Ok(mut s) = NOTCH_SIZE.lock() {
        *s = Some((
            width.filter(|&w| w > 0.0),
            (height > 0.0).then_some(height),
        ));
    }
    wake();
}

/// Active top-notch toast popup size (width, height) in design units while the notch is closed.
static TOAST_RECT: Mutex<Option<(f64, f64)>> = Mutex::new(None);

#[tauri::command]
pub fn notch_set_toast_rect(width: f64, height: f64) {
    if let Ok(mut r) = TOAST_RECT.lock() {
        *r = (width > 0.0 && height > 0.0).then_some((width, height));
    }
    wake();
}

/// Notch only: true while the compact live-lyrics pill is pinned open (stays visible until clicked
/// and keeps areas outside the pill click-through).
static NOTCH_LYRICS_PIN: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn notch_set_lyrics_pin(pinned: bool) {
    NOTCH_LYRICS_PIN.store(pinned, Ordering::Relaxed);
    wake();
}

/// Timestamp of the last close per panel, so a bar button click that causes focus loss (closing
/// the panel on mouse-down) does not immediately re-open it on mouse-up.
static LAST_CLOSE: Mutex<Vec<(&'static str, Instant)>> = Mutex::new(Vec::new());

fn is_locked_by_fullscreen(label: &str) -> bool {
    matches!(label, "bar" | "launcher" | "notch") && crate::winlist::is_foreground_fullscreen()
}

/// Per-panel "forced open" flag (hotkey, tray, or button click).
pub struct Panels(pub HashMap<&'static str, Arc<AtomicBool>>);

impl Panels {
    /// Dismiss any other open panel sharing the same screen edge (except bar + preview).
    fn dismiss_same_edge(&self, opening: &str) {
        let edge = effective_edge(opening, default_edge_for(opening));
        if edge == Edge::Free {
            return;
        }
        for (&other, flag) in &self.0 {
            if other == opening {
                continue;
            }
            if matches!((opening, other), ("bar", "preview") | ("preview", "bar")) {
                continue;
            }
            if effective_edge(other, default_edge_for(other)) == edge
                && flag.load(Ordering::Relaxed)
            {
                flag_set(&TRANSIENT, other, false);
                flag_set(&HELD, other, false);
                flag_set(&DISMISSED, other, true);
                reset_launcher_override(other);
                flag.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Hotkey and tray: stays open until closed again.
    pub fn toggle(&self, label: &str) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            let opening = !flag.load(Ordering::Relaxed);
            if opening && (!is_panel_enabled(key) || is_locked_by_fullscreen(key)) {
                return;
            }
            if opening {
                self.dismiss_same_edge(key);
            }
            if *key == "launcher" {
                LAUNCHER_HOVER.store(false, Ordering::Relaxed);
                LAUNCHER_FORCE_LIST.store(false, Ordering::Relaxed);
            }
            flag_set(&TRANSIENT, key, false);
            flag_set(&HELD, key, false);
            flag_set(&DISMISSED, key, false);
            let was_open = flag.fetch_xor(true, Ordering::Relaxed);
            if was_open {
                reset_launcher_override(key);
            }
            wake();
        }
    }

    /// Win key -> Launcher: toggle launcher and always show the full app list (even over maximized windows).
    pub fn toggle_launcher_full(&self) {
        if let Some((key, flag)) = self.0.get_key_value("launcher") {
            let opening = !flag.load(Ordering::Relaxed);
            if opening && (!is_panel_enabled(key) || is_locked_by_fullscreen(key)) {
                return;
            }
            if opening {
                self.dismiss_same_edge(key);
            }
            LAUNCHER_HOVER.store(false, Ordering::Relaxed);
            LAUNCHER_FORCE_LIST.store(opening, Ordering::Relaxed);
            flag_set(&TRANSIENT, key, false);
            flag_set(&HELD, key, false);
            flag_set(&DISMISSED, key, false);
            let was_open = flag.fetch_xor(true, Ordering::Relaxed);
            if was_open {
                reset_launcher_override(key);
            }
            wake();
        }
    }

    pub fn set(&self, label: &str, open: bool) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            if open && (!is_panel_enabled(key) || is_locked_by_fullscreen(key)) {
                return;
            }
            if open {
                self.dismiss_same_edge(key);
            }
            if *key == "launcher" {
                LAUNCHER_HOVER.store(false, Ordering::Relaxed);
                LAUNCHER_FORCE_LIST.store(false, Ordering::Relaxed);
            }
            if !open {
                flag_set(&TRANSIENT, key, false);
                flag_set(&HELD, key, false);
                flag_set(&DISMISSED, key, true);
                reset_launcher_override(key);
            } else {
                flag_set(&DISMISSED, key, false);
            }
            let was_open = flag.swap(open, Ordering::Relaxed);
            if !open && was_open {
                if let Ok(mut lc) = LAST_CLOSE.lock() {
                    if let Some(entry) = lc.iter_mut().find(|(k, _)| *k == *key) {
                        entry.1 = Instant::now();
                    } else {
                        lc.push((*key, Instant::now()));
                    }
                }
            }
            wake();
        }
    }

    /// True if `label` closed within `within` (used to ignore mouse-up re-open after blur close).
    pub fn recently_closed(&self, label: &str, within: Duration) -> bool {
        LAST_CLOSE
            .lock()
            .ok()
            .and_then(|lc| lc.iter().find(|(k, _)| *k == label).map(|(_, at)| at.elapsed() < within))
            .unwrap_or(false)
    }

    /// UI click: opens, then hides when the cursor moves away.
    pub fn set_transient(&self, label: &str) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            if !is_panel_enabled(key) || is_locked_by_fullscreen(key) {
                return;
            }
            self.dismiss_same_edge(key);
            flag_set(&TRANSIENT, key, true);
            flag_set(&DISMISSED, key, false);
            flag.store(true, Ordering::Relaxed);
            wake();
        }
    }

    pub fn is_open(&self, label: &str) -> bool {
        self.0
            .get(label)
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
}

/// Called from a bar button: open temporarily, or close if already open.
#[tauri::command]
pub fn toggle_panel(label: String, panels: State<'_, Panels>) {
    if panels.is_open(&label) {
        panels.set(&label, false);
    } else if !panels.recently_closed(&label, Duration::from_millis(350)) {
        if label == "launcher" {
            panels.set("launcher", true);
        } else {
            panels.set_transient(&label);
        }
    }
}

/// invoke("set_panel", { label, open }). open = true opens temporarily.
#[tauri::command]
pub fn set_panel(label: String, open: bool, panels: State<'_, Panels>) {
    if open {
        panels.set_transient(&label);
    } else {
        panels.set(&label, false);
    }
}

/// Temporarily allow the bar/notch/launcher to take keyboard focus (for typing inside it).
/// enable = false returns it to "does not take focus" mode.
#[tauri::command]
pub fn panel_activation(
    label: String,
    enable: bool,
    app: AppHandle,
    panels: State<'_, Panels>,
) {
    if label != "bar" && label != "notch" && label != "launcher" {
        return;
    }
    if DEBUG_SHOW && label == "notch" {
        eprintln!("[edge] notch keyboard activation: {enable}");
    }
    if let Some((key, _)) = panels.0.get_key_value(label.as_str()) {
        flag_set(&HELD, key, enable);
        if *key == "launcher" && enable {
            flag_set(&TRANSIENT, key, false);
            LAUNCHER_HOVER.store(false, Ordering::Relaxed);
        }
    }
    if let Some(window) = app.get_webview_window(&label) {
        set_activatable(&window, enable);
        if enable {
            let _ = window.set_focus();
        }
    }
    crate::altswitch::ensure_hook("panel activation");
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

/// (x, y, width, height, scale) of the primary monitor in physical pixels.
type Metrics = (f64, f64, f64, f64, f64);

/// Calculate the window rect and trigger-zone rect in physical pixels.
fn layout(spec: &PanelSpec, m: Metrics) -> (Rect, Rect) {
    let (mx, my, mw, mh, scale) = m;
    // 1 design unit = u physical pixels (monitor DPI scale multiplied by UI scale).
    let u = scale * ui_scale();
    let size = size_override(spec.label).unwrap_or(spec.size);
    let edge = effective_edge(spec.label, spec.edge);
    let full_span = matches!(spec.label, "bar" | "overview");
    let b_dip = screen_border_dip();
    // Floor to whole physical pixels so panels overlap the border by <=1px and never leave a subpixel gap.
    let b = if b_dip > 0.0 { (b_dip * u).floor() } else { 0.0 };
    // In screen-border mode, full-span panels (bar & overview) start at the true screen edge so
    // their total thickness stays identical to non-border mode, and include extra inner depth for
    // their 24px concave corner fillets.
    let span_extra = if full_span && b > 0.0 {
        BORDER_RADIUS_DIP * u
    } else {
        0.0
    };

    match edge {
        Edge::Left => {
            if full_span {
                let win = Rect {
                    x: mx,
                    y: my + b,
                    w: size.0 * u + span_extra,
                    h: (mh - 2.0 * b).max(1.0),
                };
                let trig = Rect { x: mx, y: my, w: EDGE_TRIGGER_DIP * scale, h: mh };
                (win, trig)
            } else {
                let w = size.0 * u;
                let h = (size.1 * u).min((mh - 2.0 * b).max(1.0));
                let win = Rect { x: mx + b, y: my + (mh - h) / 2.0, w, h };
                let trig = Rect {
                    x: mx,
                    y: win.y,
                    w: EDGE_TRIGGER_DIP * scale,
                    h,
                };
                (win, trig)
            }
        }
        Edge::Top => {
            if full_span {
                let h = size.0 * u + span_extra;
                let win = Rect {
                    x: mx + b,
                    y: my,
                    w: (mw - 2.0 * b).max(1.0),
                    h,
                };
                let trig = Rect { x: mx, y: my, w: mw, h: EDGE_TRIGGER_DIP * scale };
                (win, trig)
            } else {
                let w = (size.0 * u).min((mw - 2.0 * b).max(1.0));
                let h = (size.1 * u).min((mh - 2.0 * b).max(1.0));
                let win = Rect { x: mx + (mw - w) / 2.0, y: my + b, w, h };
                let tw = spec.trigger_len.map(|l| l * u).unwrap_or(w);
                let trig = Rect {
                    x: mx + (mw - tw) / 2.0,
                    y: my,
                    w: tw,
                    h: EDGE_TRIGGER_DIP * scale,
                };
                (win, trig)
            }
        }
        Edge::Bottom => {
            if full_span {
                let h = size.0 * u + span_extra;
                let win = Rect {
                    x: mx + b,
                    y: my + mh - h,
                    w: (mw - 2.0 * b).max(1.0),
                    h,
                };
                let trig = Rect {
                    x: mx,
                    y: my + mh - EDGE_TRIGGER_DIP * scale,
                    w: mw,
                    h: EDGE_TRIGGER_DIP * scale,
                };
                (win, trig)
            } else {
                let w = (size.0 * u).min((mw - 2.0 * b).max(1.0));
                let h = (size.1 * u).min((mh - 2.0 * b).max(1.0));
                let win = Rect { x: mx + (mw - w) / 2.0, y: my + mh - b - h, w, h };
                let tw = spec.trigger_len.map(|l| l * u).unwrap_or(w);
                let trig = Rect {
                    x: mx + (mw - tw) / 2.0,
                    y: my + mh - EDGE_TRIGGER_DIP * scale,
                    w: tw,
                    h: EDGE_TRIGGER_DIP * scale,
                };
                (win, trig)
            }
        }
        Edge::Right => {
            if full_span {
                let w = size.0 * u + span_extra;
                let win = Rect {
                    x: mx + mw - w,
                    y: my + b,
                    w,
                    h: (mh - 2.0 * b).max(1.0),
                };
                let trig = Rect {
                    x: mx + mw - EDGE_TRIGGER_DIP * scale,
                    y: my,
                    w: EDGE_TRIGGER_DIP * scale,
                    h: mh,
                };
                (win, trig)
            } else {
                let w = size.0 * u;
                let h = (size.1 * u).min((mh - 2.0 * b).max(1.0));
                let win = Rect { x: mx + mw - b - w, y: my + (mh - h) / 2.0, w, h };
                let trig = Rect {
                    x: mx + mw - EDGE_TRIGGER_DIP * scale,
                    y: win.y,
                    w: EDGE_TRIGGER_DIP * scale,
                    h,
                };
                (win, trig)
            }
        }
        Edge::Free => {
            let (x, y, w, h) = free_rect(spec.label).unwrap_or((mx, my, 300.0 * u, 176.0 * u));
            (Rect { x, y, w, h }, Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 })
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

/// Place the window at the correct position and size in a single atomic Win32 call.
pub fn place(window: &WebviewWindow, spec: &PanelSpec) -> tauri::Result<()> {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER},
    };

    if let Some(m) = monitor_metrics(window) {
        let (win, _) = layout(spec, m);
        let w = win.w.round() as u32;
        let h = win.h.round() as u32;
        let x = win.x.round() as i32;
        let y = win.y.round() as i32;
        let size = PhysicalSize::new(w, h);
        let pos = PhysicalPosition::new(x, y);
        // Every SetWindowPos makes WebView2 resize its surface, so skip what is already right.
        if window.outer_size().ok() != Some(size) || window.outer_position().ok() != Some(pos) {
            if let Ok(raw) = window.hwnd() {
                let hwnd = HWND(raw.0 as isize as *mut c_void);
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        x,
                        y,
                        w as i32,
                        h as i32,
                        SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                    );
                }
            } else {
                window.set_size(size)?;
                window.set_position(pos)?;
            }
        }
    }
    Ok(())
}

/// Whether the window already has the always-on-top style.
fn is_topmost(window: &WebviewWindow) -> bool {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GetWindowLongW, GWL_EXSTYLE, WS_EX_TOPMOST},
    };

    window
        .hwnd()
        .map(|h| unsafe {
            let ex = GetWindowLongW(HWND(h.0 as isize as *mut c_void), GWL_EXSTYLE) as u32;
            (ex & WS_EX_TOPMOST.0) != 0
        })
        .unwrap_or(false)
}

/// activatable = false: clicking the window does not give it focus (bar and notch),
/// so clicking the bar or notch does not interrupt typing in the active app.
pub fn set_activatable(window: &WebviewWindow, activatable: bool) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_NOACTIVATE},
    };

    // Tauri's hwnd() uses an HWND type from a different version of the `windows` crate,
    // so convert it through its pointer value.
    if let Ok(h) = window.hwnd() {
        let hwnd = HWND(h.0 as isize as *mut c_void);
        unsafe {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let new = if activatable {
                ex & !WS_EX_NOACTIVATE.0
            } else {
                ex | WS_EX_NOACTIVATE.0
            };
            SetWindowLongW(hwnd, GWL_EXSTYLE, new as i32);
        }
    }
}

/// Update the window on the main thread: reset position, click-through and focus, then notify React.
fn set_visible(
    app: &AppHandle,
    window: &WebviewWindow,
    event: &str,
    show: bool,
    spec: &PanelSpec,
    notch_compact: bool,
) {
    let app_handle = app.clone();
    let window = window.clone();
    let event = event.to_string();
    let spec = *spec;
    let queued = Instant::now();
    let _ = app.run_on_main_thread(move || {
        let started = Instant::now();
        let waited = queued.elapsed();
        if show {
            // Always open the launcher at its usual size (the wallpaper view expands it itself).
            if spec.label == "launcher" {
                set_size_override("launcher", None);
                let compact = !LAUNCHER_FORCE_LIST.load(Ordering::Relaxed)
                    && crate::winlist::has_visible_window();
                let _ = window.emit("launcher-compact", compact);
            }
            if spec.label == "notch" {
                let _ = window.emit("notch-compact", notch_compact);
            }

            // The display can change (power, resolution, dock), so reset each time it appears.
            let _ = place(&window, &spec);
            if !is_topmost(&window) {
                let _ = window.set_always_on_top(true);
            }
        }
        let launcher_focus = if spec.label == "launcher" {
            let take = show && !LAUNCHER_HOVER.load(Ordering::Relaxed);
            set_activatable(&window, take);
            if show {
                let _ = window.emit("launcher-focus", take);
            }
            take
        } else {
            spec.focus
        };
        let _ = window.set_ignore_cursor_events(!show);
        if show && launcher_focus {
            mask_alt_if_down();
            let _ = window.set_focus();
            mask_alt_if_down();
        }
        let _ = window.emit(event.as_str(), show);
        if spec.label != "switcher" {
            if !show {
                // If an ORUMI panel (launcher, notch, bar) still holds foreground focus when closing,
                // restore focus to the last active external window so the hidden WebView2 window does
                // not keep keyboard focus.
                let panels = app_handle.state::<Panels>();
                if !panels.is_open("launcher")
                    && !panels.is_open("overlay")
                    && !panels.is_open("overview")
                    && !crate::activities::is_open()
                {
                    crate::winlist::restore_external_foreground_if_ours();
                }
            }
            crate::altswitch::ensure_hook(if show { "panel opened" } else { "panel closed" });
            if launcher_focus {
                crate::wm_ipc::ensure_hook();
                if show {
                    // WebView2 focus settles slightly asynchronously after `set_focus()`.
                    thread::spawn(|| {
                        thread::sleep(Duration::from_millis(80));
                        crate::altswitch::ensure_hook("panel focus settled");
                        crate::wm_ipc::ensure_hook();
                    });
                }
            }
        }
        if DEBUG_SHOW && show && spec.label == "switcher" {
            eprintln!(
                "[edge] switcher: waited {} ms for the main thread, show work took {} ms",
                waited.as_millis(),
                started.elapsed().as_millis()
            );
        }
    });
}

fn panel_hit_rect(label: &str, mut win: Rect, scale: f64) -> Rect {
    if let Some((x, y, w, h)) = hit_rect(label) {
        return Rect { x, y, w, h };
    }
    let u = scale * ui_scale();
    let b_dip = screen_border_dip();
    let b = if b_dip > 0.0 { (b_dip * u).floor() } else { 0.0 };

    if matches!(label, "bar" | "overview") && b > 0.0 {
        // Full-span bar & overview in border mode: window already starts at the true screen edge;
        // exclude the inner concave fillet overhang and extend perpendicular ends across the border.
        let fillet = BORDER_RADIUS_DIP * u;
        match effective_edge(label, Edge::Left) {
            Edge::Left => {
                win.w = (win.w - fillet).max(0.0);
                win.y -= b;
                win.h += 2.0 * b;
            }
            Edge::Right => {
                win.x += fillet;
                win.w = (win.w - fillet).max(0.0);
                win.y -= b;
                win.h += 2.0 * b;
            }
            Edge::Top => {
                win.x -= b;
                win.w += 2.0 * b;
                win.h = (win.h - fillet).max(0.0);
            }
            Edge::Bottom => {
                win.x -= b;
                win.w += 2.0 * b;
                win.y += fillet;
                win.h = (win.h - fillet).max(0.0);
            }
            Edge::Free => {}
        }
        return win;
    }

    if matches!(label, "bar" | "overview") {
        let inset = BAR_INSET_DIP * u;
        if matches!(effective_edge(label, Edge::Left), Edge::Top | Edge::Bottom) {
            win.x += inset;
            win.w = (win.w - 2.0 * inset).max(0.0);
        } else {
            win.y += inset;
            win.h = (win.h - 2.0 * inset).max(0.0);
        }
    }
    if label == "launcher" {
        if let Ok(ls) = LAUNCHER_SIZE.lock() {
            if let Some((cw, ch)) = *ls {
                let actual_w = (cw * u).min(win.w);
                let actual_h = (ch * u).min(win.h);
                win.x += (win.w - actual_w) / 2.0;
                if effective_edge("launcher", Edge::Bottom) == Edge::Bottom {
                    win.y += win.h - actual_h;
                }
                win.w = actual_w;
                win.h = actual_h;
            }
        }
    }
    if label == "notch" {
        if let Ok(ns) = NOTCH_SIZE.lock() {
            if let Some((cw, ch)) = *ns {
                if let Some(w) = cw {
                    let actual_w = (w * u).min(win.w);
                    win.x += (win.w - actual_w) / 2.0;
                    win.w = actual_w;
                }
                if let Some(h) = ch {
                    let actual_h = (h * u).min(win.h);
                    if effective_edge("notch", Edge::Top) == Edge::Bottom {
                        win.y += win.h - actual_h;
                    }
                    win.h = actual_h;
                }
            }
        }
    }
    if b > 0.0 {
        // Extend the hit rect across the border strip on the panel's anchored edge so resting
        // the cursor on the screen edge while the panel is open does not trigger auto-hide.
        match effective_edge(label, default_edge_for(label)) {
            Edge::Left => {
                win.x -= b;
                win.w += b;
            }
            Edge::Right => {
                win.w += b;
            }
            Edge::Top => {
                win.y -= b;
                win.h += b;
            }
            Edge::Bottom => {
                win.h += b;
            }
            Edge::Free => {}
        }
    }
    win
}

fn toast_hit_rect(m: Metrics) -> Option<Rect> {
    if !is_panel_enabled("notch") {
        return None;
    }
    let (w_dip, h_dip) = TOAST_RECT.lock().ok().and_then(|r| *r)?;
    let (mx, my, mw, mh, scale) = m;
    let u = scale * ui_scale();
    let b_dip = screen_border_dip();
    let b = if b_dip > 0.0 { (b_dip * u).floor() } else { 0.0 };
    let w = w_dip * u;
    let trig_h = EDGE_TRIGGER_DIP * scale;
    let h = (h_dip * u - trig_h).max(0.0);
    let y = if effective_edge("notch", Edge::Top) == Edge::Bottom {
        my + mh - b - trig_h - h
    } else {
        my + b + trig_h
    };
    Some(Rect {
        x: mx + (mw - w) / 2.0,
        y,
        w,
        h,
    })
}

fn set_toast_passthrough(app: &AppHandle, window: &WebviewWindow, ignore: bool) {
    let window = window.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = window.set_ignore_cursor_events(ignore);
    });
}

struct Runtime {
    window: WebviewWindow,
    spec: PanelSpec,
    pinned: Arc<AtomicBool>,
    event: String,
    visible: bool,
    was_pinned: bool,
    outside_since: Option<Instant>,
    inside_since: Option<Instant>,
    /// Whether the cursor entered the window after the panel was opened by clicking.
    pin_entered: bool,
    /// Hover-opened flag-only panels (launcher, overlay): the cursor must leave the trigger zone
    /// after a close before the panel can open again.
    armed: bool,
    /// Notch only: true while cursor is over the closed-notch notification popup.
    toast_hit: bool,
    /// Notch only: true while the compact live-lyrics pill is pinned open.
    was_lyrics_pin: bool,
    /// Notch only: true while cursor is over the pinned compact live-lyrics pill.
    lyrics_hit: bool,
}

/// One thread for all panels.
/// Emit an "edge-visible:<label>" event (bool payload) to React.
pub fn start_all(app: AppHandle, items: Vec<(WebviewWindow, PanelSpec, Arc<AtomicBool>)>) {
    thread::spawn(move || {
        let _ = EDGE_THREAD.set(thread::current());
        let mut panels: Vec<Runtime> = items
            .into_iter()
            .map(|(window, spec, pinned)| Runtime {
                event: format!("edge-visible:{}", spec.label),
                window,
                spec,
                pinned,
                visible: false,
                was_pinned: false,
                outside_since: None,
                inside_since: None,
                pin_entered: false,
                armed: true,
                toast_hit: false,
                was_lyrics_pin: false,
                lyrics_hit: false,
            })
            .collect();
        if panels.is_empty() {
            return;
        }

        // Use one window to read cursor and monitor state.
        let probe = panels[0].window.clone();
        let mut metrics = monitor_metrics(&probe);
        let mut last_refresh = Instant::now();
        let mut last_fs_check = Instant::now() - Duration::from_secs(1);
        let mut fullscreen = false;
        let mut border_shown = false;
        let mut interval = POLL_FAST;

        loop {
            thread::park_timeout(interval);

            if last_refresh.elapsed() >= METRICS_REFRESH {
                metrics = monitor_metrics(&probe).or(metrics);
                last_refresh = Instant::now();
            }
            if last_fs_check.elapsed() >= Duration::from_millis(200) {
                fullscreen = crate::winlist::is_foreground_fullscreen();
                last_fs_check = Instant::now();
            }
            let want_border = is_screen_border() && !fullscreen;
            if want_border != border_shown {
                border_shown = want_border;
                sync_border_window(&app, want_border);
            }
            let Some(m) = metrics else { continue };
            let Ok(cursor) = probe.cursor_position() else { continue };


            // Choose how often to poll on the next iteration.
            let margin = NEAR_EDGE_DIP * m.4;
            let near_edge = cursor.x < m.0 + margin
                || cursor.x > m.0 + m.2 - margin
                || cursor.y < m.1 + margin
                || cursor.y > m.1 + m.3 - margin;
            let toast_rect = toast_hit_rect(m);
            let lyrics_pin = NOTCH_LYRICS_PIN.load(Ordering::Relaxed);
            let busy = toast_rect.is_some()
                || panels.iter().any(|p| {
                    (p.visible && !(p.spec.label == "notch" && lyrics_pin && !p.was_pinned))
                        || p.pinned.load(Ordering::Relaxed)
                });
            interval = if near_edge || busy { POLL_FAST } else { POLL_IDLE };

            // A thumbnail preview extends the bar: keep the bar open while the cursor is over it.
            let over_preview = panels
                .iter()
                .any(|q| q.spec.label == "preview" && q.visible)
                && hit_rect("preview")
                    .or_else(|| free_rect("preview"))
                    .map(|(x, y, w, h)| Rect { x, y, w, h }.contains(cursor.x, cursor.y))
                    .unwrap_or(false);
            let over_bar = panels
                .iter()
                .find(|q| q.spec.label == "bar" && q.visible)
                .is_some_and(|q| {
                    let (bar_win, _) = layout(&q.spec, m);
                    bar_win.contains(cursor.x, cursor.y)
                });

            let overview_open = panels
                .iter()
                .any(|q| q.spec.label == "overview" && (q.visible || q.pinned.load(Ordering::Relaxed)));

            for p in panels.iter_mut() {
                if !is_panel_enabled(p.spec.label) {
                    p.pinned.store(false, Ordering::Relaxed);
                    flag_set(&TRANSIENT, p.spec.label, false);
                    flag_set(&HELD, p.spec.label, false);
                    flag_set(&DISMISSED, p.spec.label, false);
                    if p.spec.label == "notch" {
                        NOTCH_LYRICS_PIN.store(false, Ordering::Relaxed);
                        p.was_lyrics_pin = false;
                        p.lyrics_hit = false;
                        if p.toast_hit {
                            p.toast_hit = false;
                            set_toast_passthrough(&app, &p.window, true);
                        }
                    }
                    p.armed = false;
                    p.was_pinned = false;
                    p.inside_since = None;
                    p.outside_since = None;
                    if p.visible {
                        p.visible = false;
                        set_visible(&app, &p.window, &p.event, false, &p.spec, false);
                    }
                    continue;
                }
                if fullscreen && matches!(p.spec.label, "bar" | "launcher" | "notch") {
                    p.pinned.store(false, Ordering::Relaxed);
                    flag_set(&TRANSIENT, p.spec.label, false);
                    flag_set(&HELD, p.spec.label, false);
                    if p.spec.label == "launcher" {
                        LAUNCHER_HOVER.store(false, Ordering::Relaxed);
                    }
                    if p.spec.label == "notch" {
                        NOTCH_LYRICS_PIN.store(false, Ordering::Relaxed);
                        p.was_lyrics_pin = false;
                        p.lyrics_hit = false;
                        if p.toast_hit {
                            p.toast_hit = false;
                            set_toast_passthrough(&app, &p.window, true);
                        }
                    }
                    p.armed = false;
                    p.was_pinned = false;
                    p.inside_since = None;
                    p.outside_since = None;
                    if p.visible {
                        p.visible = false;
                        set_visible(&app, &p.window, &p.event, false, &p.spec, false);
                    }
                    continue;
                }
                if p.spec.label == "bar"
                    && overview_open
                    && effective_edge("bar", Edge::Left) == Edge::Left
                {
                    p.pinned.store(false, Ordering::Relaxed);
                    flag_set(&TRANSIENT, "bar", false);
                    flag_set(&HELD, "bar", false);
                    p.armed = false;
                    p.was_pinned = false;
                    p.inside_since = None;
                    p.outside_since = None;
                    if p.visible {
                        p.visible = false;
                        set_visible(&app, &p.window, &p.event, false, &p.spec, false);
                    }
                    continue;
                }
                let mut pinned = p.pinned.load(Ordering::Relaxed);

                // Panel without hover: follow the flag only. The launcher and overlay may also be
                // opened by dwelling at their screen edge.
                if !p.spec.hover {
                    if matches!(p.spec.label, "overlay" | "launcher") {
                        let (win_rect, trigger_rect) = layout(&p.spec, m);
                        if !pinned && !p.visible && is_panel_hotedge(p.spec.label) {
                            if trigger_rect.contains(cursor.x, cursor.y) {
                                if p.armed {
                                    let since = *p.inside_since.get_or_insert_with(Instant::now);
                                    if since.elapsed() >= EDGE_DWELL {
                                        p.inside_since = None;
                                        p.pin_entered = false;
                                        p.outside_since = None;
                                        if p.spec.label == "overlay" {
                                            crate::overlay::hover_open(&app);
                                        } else {
                                            // Hover-opened launcher starts without focus and auto-closes
                                            // when the cursor leaves unless the user clicks to type.
                                            LAUNCHER_HOVER.store(true, Ordering::Relaxed);
                                            LAUNCHER_FORCE_LIST.store(false, Ordering::Relaxed);
                                            flag_set(&TRANSIENT, "launcher", true);
                                            flag_set(&DISMISSED, "launcher", false);
                                            p.pinned.store(true, Ordering::Relaxed);
                                            pinned = true;
                                        }
                                    }
                                } else {
                                    p.inside_since = None;
                                }
                            } else {
                                p.armed = true;
                                p.inside_since = None;
                            }
                        } else if !is_panel_hotedge(p.spec.label) {
                            p.inside_since = None;
                        }
                        // Hover-opened overlay/launcher hides again once the cursor leaves it (unless held).
                        if pinned
                            && flag_has(&TRANSIENT, p.spec.label)
                            && !flag_has(&HELD, p.spec.label)
                        {
                            let hit = panel_hit_rect(p.spec.label, win_rect, m.4);
                            if hit.contains(cursor.x, cursor.y) {
                                p.pin_entered = true;
                                p.outside_since = None;
                            } else {
                                let since = *p.outside_since.get_or_insert_with(Instant::now);
                                let limit = if p.pin_entered { HIDE_DELAY } else { PIN_GRACE };
                                if since.elapsed() >= limit {
                                    p.pinned.store(false, Ordering::Relaxed);
                                    flag_set(&TRANSIENT, p.spec.label, false);
                                    pinned = false;
                                }
                            }
                        }
                    }
                    if pinned != p.visible {
                        p.visible = pinned;
                        if !pinned {
                            p.armed = false;
                            flag_set(&HELD, p.spec.label, false);
                            flag_set(&TRANSIENT, p.spec.label, false);
                            if p.spec.label == "launcher" {
                                LAUNCHER_HOVER.store(false, Ordering::Relaxed);
                            }
                        }
                        set_visible(&app, &p.window, &p.event, pinned, &p.spec, false);
                    }
                    p.was_pinned = pinned;
                    continue;
                }

                // Forced open: ignore cursor position, except for panels opened by clicking.
                if pinned {
                    if p.spec.label == "notch" {
                        NOTCH_LYRICS_PIN.store(false, Ordering::Relaxed);
                    }
                    if !p.visible {
                        p.visible = true;
                        p.toast_hit = false;
                        p.was_lyrics_pin = false;
                        p.lyrics_hit = false;
                        p.pin_entered = false;
                        p.outside_since = None;
                        set_visible(&app, &p.window, &p.event, true, &p.spec, false);
                    } else if p.spec.label == "notch" && !p.was_pinned {
                        if p.was_lyrics_pin {
                            p.was_lyrics_pin = false;
                            p.lyrics_hit = false;
                            set_toast_passthrough(&app, &p.window, false);
                        }
                        let w = p.window.clone();
                        let _ = app.run_on_main_thread(move || {
                            let _ = w.emit("notch-lyrics-pin", false);
                            let _ = w.emit("notch-compact", false);
                        });
                    }
                    p.was_pinned = true;
                    if flag_has(&TRANSIENT, p.spec.label) && !flag_has(&HELD, p.spec.label) {
                        let (win_rect, _) = layout(&p.spec, m);
                        let hit = panel_hit_rect(p.spec.label, win_rect, m.4);
                        if hit.contains(cursor.x, cursor.y) {
                            p.pin_entered = true;
                            p.outside_since = None;
                        } else if p.spec.label == "preview" && over_bar {
                            // Cursor is still on the bar (e.g. moving toward the preview card).
                            p.outside_since = None;
                        } else {
                            let since = *p.outside_since.get_or_insert_with(Instant::now);
                            let limit = if p.pin_entered { HIDE_DELAY } else { PIN_GRACE };
                            if since.elapsed() >= limit {
                                // Release the lock; the next iteration will hide the panel.
                                p.pinned.store(false, Ordering::Relaxed);
                                flag_set(&TRANSIENT, p.spec.label, false);
                            }
                        }
                    } else {
                        p.outside_since = None;
                    }
                    continue;
                }
                p.was_pinned = false;

                if flag_has(&DISMISSED, p.spec.label) {
                    flag_set(&DISMISSED, p.spec.label, false);
                    p.armed = false;
                    p.inside_since = None;
                    p.outside_since = None;
                    if p.spec.label == "notch" {
                        NOTCH_LYRICS_PIN.store(false, Ordering::Relaxed);
                        p.was_lyrics_pin = false;
                        p.lyrics_hit = false;
                    }
                    if p.visible {
                        p.visible = false;
                        set_visible(&app, &p.window, &p.event, false, &p.spec, false);
                    }
                    continue;
                }

                let (win_rect, trigger_rect) = layout(&p.spec, m);
                let win_rect = panel_hit_rect(p.spec.label, win_rect, m.4);

                if !p.visible {
                    if p.spec.label == "notch" {
                        let over_toast = toast_rect
                            .as_ref()
                            .is_some_and(|r| r.contains(cursor.x, cursor.y));
                        if over_toast != p.toast_hit {
                            p.toast_hit = over_toast;
                            set_toast_passthrough(&app, &p.window, !over_toast);
                        }
                    }
                    if !p.armed {
                        if !win_rect.contains(cursor.x, cursor.y)
                            && !trigger_rect.contains(cursor.x, cursor.y)
                        {
                            p.armed = true;
                        }
                        p.inside_since = None;
                    } else if left_mouse_down() || !is_panel_hotedge(p.spec.label) {
                        // Do not pop open notch/bar while dragging a window near the edge
                        // (lets vscroll-wm Snap Layouts capture top-edge drops unobstructed),
                        // or when hotedge is disabled for this panel.
                        p.inside_since = None;
                    } else if trigger_rect.contains(cursor.x, cursor.y) {
                        let since = *p.inside_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= EDGE_DWELL {
                            let notch_compact = if p.spec.label == "notch"
                                && effective_edge("notch", Edge::Top) == Edge::Top
                            {
                                crate::winlist::has_window_near_notch(
                                    m.0,
                                    m.1,
                                    m.2,
                                    m.4 * ui_scale(),
                                )
                            } else {
                                false
                            };
                            p.visible = true;
                            p.toast_hit = false;
                            p.inside_since = None;
                            p.outside_since = None;
                            set_visible(&app, &p.window, &p.event, true, &p.spec, notch_compact);
                        }
                    } else {
                        p.inside_since = None;
                    }
                } else if p.spec.label == "notch" && lyrics_pin {
                    let over = win_rect.contains(cursor.x, cursor.y);
                    if !p.was_lyrics_pin || over != p.lyrics_hit {
                        p.lyrics_hit = over;
                        set_toast_passthrough(&app, &p.window, !over);
                    }
                    p.was_lyrics_pin = true;
                    p.outside_since = None;
                } else {
                    if p.spec.label == "notch" && p.was_lyrics_pin {
                        p.was_lyrics_pin = false;
                        p.lyrics_hit = false;
                        set_toast_passthrough(&app, &p.window, false);
                    }
                    if win_rect.contains(cursor.x, cursor.y)
                        || flag_has(&HELD, p.spec.label)
                        || (p.spec.label == "bar" && over_preview)
                    {
                        // Cursor is inside, or the panel is being used for typing: do not close it.
                        p.outside_since = None;
                    } else {
                        let since = *p.outside_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= HIDE_DELAY {
                            p.visible = false;
                            p.outside_since = None;
                            set_visible(&app, &p.window, &p.event, false, &p.spec, false);
                        }
                    }
                }
            }
        }
    });
}
