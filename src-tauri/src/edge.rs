use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
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
const DEBUG_SHOW: bool = true;

/// The edge thread, so opening or closing a panel can wake it from its polling sleep instead of
/// waiting for the interval to end (up to POLL_IDLE).
static EDGE_THREAD: OnceLock<thread::Thread> = OnceLock::new();

fn wake() {
    if let Some(t) = EDGE_THREAD.get() {
        t.unpark();
    }
}
/// "Near the edge" = within this distance of the screen edge (design units).
const NEAR_EDGE_DIP: f64 = 96.0;
/// How often to reread monitor dimensions.
const METRICS_REFRESH: Duration = Duration::from_secs(5);
/// Bar distance from the top and bottom screen edges (design units, before UI scaling).
const BAR_INSET_DIP: f64 = 12.0;

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

// ---------- Panels ----------

#[derive(Clone, Copy)]
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

/// Temporary window size that replaces PanelSpec.size (e.g. the launcher while the wallpaper picker is open).
static SIZE_OVERRIDE: Mutex<Vec<(&'static str, (f64, f64))>> = Mutex::new(Vec::new());

/// Launcher size (design units) while showing the wallpaper picker: wide and short.
const WALLPAPER_VIEW_SIZE: (f64, f64) = (1040.0, 300.0);

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

/// Launcher view changed: "wallpaper" expands it; otherwise, restore its usual size.
#[tauri::command]
pub fn launcher_view(view: String, app: AppHandle) {
    set_size_override("launcher", (view == "wallpaper").then_some(WALLPAPER_VIEW_SIZE));
    let a = app.clone();
    let _ = app.run_on_main_thread(move || crate::place_panel(&a, "launcher"));
}

/// Active notch content height in design units (used to shrink its hover hit area when short).
static NOTCH_HEIGHT: Mutex<Option<f64>> = Mutex::new(None);

#[tauri::command]
pub fn notch_set_height(height: f64) {
    if let Ok(mut h) = NOTCH_HEIGHT.lock() {
        *h = (height > 0.0).then_some(height);
    }
    wake();
}

/// Timestamp of the last close per panel, so a bar button click that causes focus loss (closing
/// the panel on mouse-down) does not immediately re-open it on mouse-up.
static LAST_CLOSE: Mutex<Vec<(&'static str, Instant)>> = Mutex::new(Vec::new());

/// Per-panel "forced open" flag (hotkey, tray, or button click).
pub struct Panels(pub HashMap<&'static str, Arc<AtomicBool>>);

impl Panels {
    /// Hotkey and tray: stays open until closed again.
    pub fn toggle(&self, label: &str) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            flag_set(&TRANSIENT, key, false);
            flag_set(&HELD, key, false);
            flag.fetch_xor(true, Ordering::Relaxed);
            wake();
        }
    }

    pub fn set(&self, label: &str, open: bool) {
        if let Some((key, flag)) = self.0.get_key_value(label) {
            if !open {
                flag_set(&TRANSIENT, key, false);
                flag_set(&HELD, key, false);
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
            flag_set(&TRANSIENT, key, true);
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
        panels.set_transient(&label);
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

/// Temporarily allow the bar/notch to take keyboard focus (for typing inside it).
/// enable = false returns it to "does not take focus" mode.
#[tauri::command]
pub fn panel_activation(
    label: String,
    enable: bool,
    app: AppHandle,
    panels: State<'_, Panels>,
) {
    if label != "bar" && label != "notch" {
        return;
    }
    if label == "notch" {
        eprintln!("[edge] notch keyboard activation: {enable}");
    }
    if let Some((key, _)) = panels.0.get_key_value(label.as_str()) {
        flag_set(&HELD, key, enable);
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

    match spec.edge {
        Edge::Left => {
            let inset = BAR_INSET_DIP * u;
            let win = Rect { x: mx, y: my + inset, w: size.0 * u, h: mh - 2.0 * inset };
            let trig = Rect { x: mx, y: my, w: EDGE_TRIGGER_DIP * scale, h: mh };
            (win, trig)
        }
        Edge::Top => {
            let w = size.0 * u;
            let mut h = size.1 * u;
            if spec.label == "notch" {
                if let Ok(nh) = NOTCH_HEIGHT.lock() {
                    if let Some(ch) = *nh {
                        h = (ch * u).min(h);
                    }
                }
            }
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
            let w = size.0 * u;
            let h = size.1 * u;
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
            let w = size.0 * u;
            let h = size.1 * u;
            let win = Rect { x: mx + mw - w, y: my + (mh - h) / 2.0, w, h };
            let trig = Rect {
                x: mx + mw - EDGE_TRIGGER_DIP * scale,
                y: win.y,
                w: EDGE_TRIGGER_DIP * scale,
                h,
            };
            (win, trig)
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
fn set_visible(app: &AppHandle, window: &WebviewWindow, event: &str, show: bool, spec: &PanelSpec) {
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
                let compact = crate::winlist::is_foreground_maximized();
                let _ = window.emit("launcher-compact", compact);
            }

            // The display can change (power, resolution, dock), so reset each time it appears.
            let _ = place(&window, &spec);
            if !is_topmost(&window) {
                let _ = window.set_always_on_top(true);
            }
        }
        let _ = window.set_ignore_cursor_events(!show);
        if show && spec.focus {
            let _ = window.set_focus();
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
                    && !crate::activities::is_open()
                {
                    crate::winlist::restore_external_foreground_if_ours();
                }
            }
            crate::altswitch::ensure_hook(if show { "panel opened" } else { "panel closed" });
            if show && spec.label == "launcher" {
                // Launcher.tsx focuses its <input> after 60 ms; refresh the hook once more afterward.
                thread::spawn(|| {
                    thread::sleep(Duration::from_millis(80));
                    crate::altswitch::ensure_hook("launcher input focused");
                });
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

struct Runtime {
    window: WebviewWindow,
    spec: PanelSpec,
    pinned: Arc<AtomicBool>,
    event: String,
    visible: bool,
    outside_since: Option<Instant>,
    inside_since: Option<Instant>,
    /// Whether the cursor entered the window after the panel was opened by clicking.
    pin_entered: bool,
    /// Hover-opened flag-only panels (launcher, overlay): the cursor must leave the trigger zone
    /// after a close before the panel can open again.
    armed: bool,
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
                outside_since: None,
                inside_since: None,
                pin_entered: false,
                armed: true,
            })
            .collect();
        if panels.is_empty() {
            return;
        }

        // Use one window to read cursor and monitor state.
        let probe = panels[0].window.clone();
        let mut metrics = monitor_metrics(&probe);
        let mut last_refresh = Instant::now();
        let mut interval = POLL_FAST;

        loop {
            thread::park_timeout(interval);

            if last_refresh.elapsed() >= METRICS_REFRESH {
                metrics = monitor_metrics(&probe).or(metrics);
                last_refresh = Instant::now();
            }
            let Some(m) = metrics else { continue };
            let Ok(cursor) = probe.cursor_position() else { continue };


            // Choose how often to poll on the next iteration.
            let margin = NEAR_EDGE_DIP * m.4;
            let near_edge = cursor.x < m.0 + margin
                || cursor.x > m.0 + m.2 - margin
                || cursor.y < m.1 + margin
                || cursor.y > m.1 + m.3 - margin;
            let busy = panels
                .iter()
                .any(|p| p.visible || p.pinned.load(Ordering::Relaxed));
            interval = if near_edge || busy { POLL_FAST } else { POLL_IDLE };

            // A thumbnail preview extends the bar: keep the bar open while the cursor is over it.
            let over_preview = panels
                .iter()
                .any(|q| q.spec.label == "preview" && q.visible)
                && hit_rect("preview")
                    .or_else(|| free_rect("preview"))
                    .map(|(x, y, w, h)| Rect { x, y, w, h }.contains(cursor.x, cursor.y))
                    .unwrap_or(false);

            for p in panels.iter_mut() {
                let pinned = p.pinned.load(Ordering::Relaxed);

                // Panel without hover: follow the flag only. The launcher and overlay may also be
                // opened by dwelling at their screen edge.
                if !p.spec.hover {
                    if matches!(p.spec.label, "overlay" | "launcher") {
                        let (win_rect, trigger_rect) = layout(&p.spec, m);
                        if !pinned && !p.visible {
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
                                            app.state::<Panels>().set("launcher", true);
                                        }
                                    }
                                } else {
                                    p.inside_since = None;
                                }
                            } else {
                                p.armed = true;
                                p.inside_since = None;
                            }
                        }
                        // Hover-opened overlay hides again once the cursor leaves it.
                        if pinned
                            && p.spec.label == "overlay"
                            && flag_has(&TRANSIENT, "overlay")
                        {
                            if win_rect.contains(cursor.x, cursor.y) {
                                p.pin_entered = true;
                                p.outside_since = None;
                            } else {
                                let since = *p.outside_since.get_or_insert_with(Instant::now);
                                let limit = if p.pin_entered { HIDE_DELAY } else { PIN_GRACE };
                                if since.elapsed() >= limit {
                                    p.pinned.store(false, Ordering::Relaxed);
                                    flag_set(&TRANSIENT, "overlay", false);
                                }
                            }
                        }
                    }
                    if pinned != p.visible {
                        p.visible = pinned;
                        if !pinned {
                            p.armed = false;
                        }
                        set_visible(&app, &p.window, &p.event, pinned, &p.spec);
                    }
                    continue;
                }

                // Forced open: ignore cursor position, except for panels opened by clicking.
                if pinned {
                    if !p.visible {
                        p.visible = true;
                        p.pin_entered = false;
                        p.outside_since = None;
                        set_visible(&app, &p.window, &p.event, true, &p.spec);
                    }
                    if flag_has(&TRANSIENT, p.spec.label) && !flag_has(&HELD, p.spec.label) {
                        let (win_rect, _) = layout(&p.spec, m);
                        // A free window may have a smaller logical hit area than the window itself.
                        let hit = hit_rect(p.spec.label)
                            .map(|(x, y, w, h)| Rect { x, y, w, h })
                            .unwrap_or(win_rect);
                        if hit.contains(cursor.x, cursor.y) {
                            p.pin_entered = true;
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

                let (win_rect, trigger_rect) = layout(&p.spec, m);
                // A free window may be larger than what it shows: keep it open only while the
                // cursor is over its logical hit area, not over the whole (transparent) window.
                let win_rect = hit_rect(p.spec.label)
                    .map(|(x, y, w, h)| Rect { x, y, w, h })
                    .unwrap_or(win_rect);

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
                } else if win_rect.contains(cursor.x, cursor.y)
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
                        set_visible(&app, &p.window, &p.event, false, &p.spec);
                    }
                }
            }
        }
    });
}
