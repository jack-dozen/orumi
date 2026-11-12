//! Alt+Tab replacement. The low-level keyboard hook only decides whether to "swallow or pass through"
//! and sends messages to the worker thread; all heavy work runs there because Windows removes slow
//! hooks. A watchdog reinstalls the hook if it stops receiving keystrokes.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    mem::size_of,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::{
        GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW,
            SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
            MSG, WH_KEYBOARD_LL,
        },
    },
};

use crate::{
    capture,
    edge::{self, Panels},
    winlist::{self, WindowEntry},
};

// ---------- Constants ----------

const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
/// Message to the hook thread: reinstall the hook (WM_APP + 1).
const WM_REINSTALL: u32 = 0x8001;

const LLKHF_INJECTED: u32 = 0x10;
/// Windows marks keyboard events that occur while Alt is held with this bit.
const LLKHF_ALTDOWN: u32 = 0x20;

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

/// The panel appears only if Alt+Tab is held this long; a quick tap does not show it.
const SHOW_DELAY: Duration = Duration::from_millis(60);
/// After this, if Alt is no longer held, the session is considered finished.
const ALT_POLL_GRACE: Duration = Duration::from_millis(300);
/// Windows taking longer than this are logged in the terminal.
const SLOW_LIST_MS: u128 = 40;
/// Minimum interval between hook reinstalls by the watchdog.
const REINSTALL_COOLDOWN_MS: u64 = 5000;
const WATCHDOG_POLL: Duration = Duration::from_millis(10);
const MISSED_TAB_GRACE_MS: u64 = 40;
const RECENT_TAB_SEEN_MS: u64 = 80;
/// Let the panel settle before thumbnail capture can compete for CPU time.
const REFRESH_DELAY: Duration = Duration::from_millis(400);
/// true = print timing lines to the terminal (set to false when no longer needed).
const DEBUG_TIMING: bool = true;
/// true = Ctrl+Alt+Tab opens this switcher too. false = it is left to Windows (its sticky
/// switcher). The launcher hotkey is Ctrl+Alt+J, so with false an Alt+Tab pressed while Ctrl is
/// still down opens the Windows one.
const HANDLE_CTRL_ALT_TAB: bool = true;

// Grid geometry (design units = CSS px). The window is sized to fit the grid when it opens.
const CARD_W: f64 = 200.0;
const CARD_H: f64 = 158.0;
const GRID_GAP: f64 = 10.0;
const GRID_PAD: f64 = 16.0;
/// The panel is at most this wide, and at most this fraction of the screen width.
const MAX_PANEL_W: f64 = 1800.0;
const MAX_W_FRACTION: f64 = 0.92;
/// Maximum panel height as a fraction of the screen; extra rows scroll inside the panel.
const MAX_H_FRACTION: f64 = 0.70;

// ---------- Shared state (atomics only; safe to use from the hook) ----------

static ENABLED: AtomicBool = AtomicBool::new(true);
static PREVIEWS_ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether an Alt+Tab session is in progress.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Keys whose keydown we swallowed; their keyup must also be swallowed.
static SWALLOWED: AtomicU8 = AtomicU8::new(0);
/// When the hook last saw a physical Tab keydown (ms since START). Used by the watchdog.
static LAST_TAB_SEEN: AtomicU64 = AtomicU64::new(0);
/// ID of the thread holding the hook (0 = not running yet).
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();
/// Whether the panel is currently shown (background thumbnail work stops when it is not).
static PANEL_OPEN: AtomicBool = AtomicBool::new(false);
/// Window size last applied to the switcher window (rounded design units).
static LAST_WINDOW: Mutex<Option<(i64, i64)>> = Mutex::new(None);
/// Diagnostics set by the hook, printed by the watchdog (never print inside the hook callback).
/// Bit 0: Alt+Tab was handed to Windows because Ctrl or Win was down.
static DIAG: AtomicU8 = AtomicU8::new(0);
/// What the hook decided for the last Tab keydown (bit 0 = a record is present, see TRACE_* below).
/// Printed by the watchdog for Alt+Tab diagnostics when DEBUG_TIMING is on.
static TAB_TRACE: AtomicU32 = AtomicU32::new(0);
const TRACE_PRESENT: u32 = 1;
const TRACE_ENABLED: u32 = 1 << 1;
const TRACE_INJECTED: u32 = 1 << 2;
const TRACE_ALT_FLAG: u32 = 1 << 3;
const TRACE_CTRL: u32 = 1 << 4;
const TRACE_WIN: u32 = 1 << 5;
const TRACE_ACTIVE: u32 = 1 << 6;
const TRACE_SWALLOWED: u32 = 1 << 7;
const TRACE_ALT_ASYNC: u32 = 1 << 8;

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Key state read directly from Windows. More reliable than tracking it ourselves,
/// because a missed key-up event cannot leave the state stuck.
fn async_down(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0 }
}

enum Msg {
    Start { reverse: bool },
    Next { reverse: bool },
    SetPreviews(bool),
    Step(i32),
    /// Up/down: move by one grid row.
    StepRow(i32),
    Cancel,
    Commit,
    Pick(usize),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(m: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(m);
    }
}

/// Ask the hook thread to reinstall its hook.
fn reinstall() {
    let tid = HOOK_THREAD.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            if let Err(error) = PostThreadMessageW(tid, WM_REINSTALL, WPARAM(0), LPARAM(0)) {
                eprintln!("[alt+tab] gagal meminta hook dipasang ulang: {error}");
            }
        }
    }
}

/// A launcher close can follow a focus transition that leaves the low-level hook detached.
pub fn launcher_closed() {
    if ENABLED.load(Ordering::Relaxed) {
        eprintln!("[alt+tab] launcher ditutup; meminta hook dipasang ulang");
        reinstall();
    }
}

/// Enable/disable the Alt+Tab replacement. Disabling also cancels any active session;
/// enabling reinstalls the hook in case Windows has removed it.
pub fn set_enabled(on: bool) {
    let was = ENABLED.swap(on, Ordering::Relaxed);
    if !on {
        ACTIVE.store(false, Ordering::Relaxed);
        send(Msg::Cancel);
    } else if !was {
        reinstall();
    }
}

// ---------- Hook ----------

fn swallow_bit(vk: u32) -> u8 {
    match vk {
        VK_TAB => 1,
        VK_ESCAPE => 2,
        VK_LEFT => 4,
        VK_RIGHT => 8,
        VK_UP => 16,
        VK_DOWN => 32,
        _ => 0,
    }
}

fn mark_swallowed(vk: u32) {
    SWALLOWED.fetch_or(swallow_bit(vk), Ordering::Relaxed);
}

/// A keydown that gets through means the old marker is stale: remove it so the next
/// key-up is not swallowed too.
fn clear_swallowed(vk: u32) {
    let bit = swallow_bit(vk);
    if bit != 0 {
        SWALLOWED.fetch_and(!bit, Ordering::Relaxed);
    }
}

/// true if we swallowed this key's keydown (and removed its marker).
fn take_swallowed(vk: u32) -> bool {
    let bit = swallow_bit(vk);
    bit != 0 && SWALLOWED.fetch_and(!bit, Ordering::Relaxed) & bit != 0
}

/// Inject one neutral key (VK 0xE8, unused by Windows). Without it, the active app
/// sees "Alt pressed and then released" (because we swallowed Tab) and opens its menu bar.
/// Called from the worker thread, NOT from the hook callback: injected events pass through all
/// other program hooks, and doing this inside the callback could cause Windows to remove our hook.
unsafe fn mask_alt_menu() {
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

/// Close the native switcher if it opened because the low-level hook missed Alt+Tab.
unsafe fn dismiss_native_switcher() -> bool {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_ESCAPE as u16),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    SendInput(&inputs, size_of::<INPUT>() as i32) == inputs.len() as u32
}

/// true = swallow this event (do not pass it to the app). Must be fast and avoid heavy calls.
unsafe fn handle(msg: usize, kb: &KBDLLHOOKSTRUCT) -> bool {
    let vk = kb.vkCode;
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !down && !up {
        return false;
    }

    let alt_flag = kb.flags.0 & LLKHF_ALTDOWN != 0;
    let alt_async = (vk == VK_TAB || ACTIVE.load(Ordering::Relaxed)) && async_down(VK_MENU);
    let alt_down = alt_flag || alt_async;

    if vk == VK_TAB && down {
        if kb.flags.0 & LLKHF_INJECTED == 0 {
            LAST_TAB_SEEN.store(now_ms(), Ordering::Relaxed);
        }
        let mut t = TRACE_PRESENT;
        let mut set = |on: bool, bit: u32| {
            if on {
                t |= bit;
            }
        };
        set(ENABLED.load(Ordering::Relaxed), TRACE_ENABLED);
        set(kb.flags.0 & LLKHF_INJECTED != 0, TRACE_INJECTED);
        set(alt_flag, TRACE_ALT_FLAG);
        set(alt_async, TRACE_ALT_ASYNC);
        set(async_down(VK_CONTROL), TRACE_CTRL);
        set(async_down(VK_LWIN) || async_down(VK_RWIN), TRACE_WIN);
        set(ACTIVE.load(Ordering::Relaxed), TRACE_ACTIVE);
        TAB_TRACE.store(t, Ordering::Relaxed);
    }
    if !ENABLED.load(Ordering::Relaxed) {
        return false;
    }

    let injected = kb.flags.0 & LLKHF_INJECTED != 0;
    let is_alt_key = matches!(vk, VK_MENU | VK_LMENU | VK_RMENU);

    // Alt released: session ends. Always pass the Alt-up event through.
    if is_alt_key && up {
        if ACTIVE.swap(false, Ordering::Relaxed) {
            send(Msg::Commit);
        }
        return false;
    }
    if injected {
        return false; // including our own neutral key
    }

    // Release keys whose keydown we already swallowed.
    if up {
        return take_swallowed(vk);
    }

    let active = ACTIVE.load(Ordering::Relaxed);

    // Any key without Alt during an active session means Alt-up was missed: end the session,
    // then pass the key through unchanged.
    if active && !alt_down && !is_alt_key {
        ACTIVE.store(false, Ordering::Relaxed);
        send(Msg::Commit);
        return false;
    }

    if vk == VK_TAB
        && alt_down
        && ((!HANDLE_CTRL_ALT_TAB && async_down(VK_CONTROL))
            || async_down(VK_LWIN)
            || async_down(VK_RWIN))
    {
        DIAG.fetch_or(1, Ordering::Relaxed);
    }

    let swallow = match vk {
        // Require Alt, with async key state as a fallback for a missing low-level flag.
        // Read Ctrl/Win/Shift state directly from Windows.
        VK_TAB
            if alt_down
                && (HANDLE_CTRL_ALT_TAB || !async_down(VK_CONTROL))
                && !async_down(VK_LWIN)
                && !async_down(VK_RWIN) =>
        {
            let reverse = async_down(VK_SHIFT);
            if active {
                send(Msg::Next { reverse });
            } else {
                ACTIVE.store(true, Ordering::Relaxed);
                send(Msg::Start { reverse });
            }
            true
        }
        VK_ESCAPE if active => {
            ACTIVE.store(false, Ordering::Relaxed);
            send(Msg::Cancel);
            true
        }
        VK_LEFT if active => {
            send(Msg::Step(-1));
            true
        }
        VK_RIGHT if active => {
            send(Msg::Step(1));
            true
        }
        VK_UP if active => {
            send(Msg::StepRow(-1));
            true
        }
        VK_DOWN if active => {
            send(Msg::StepRow(1));
            true
        }
        _ => false,
    };

    if swallow {
        if vk == VK_TAB {
            TAB_TRACE.fetch_or(TRACE_SWALLOWED, Ordering::Relaxed);
        }
        mark_swallowed(vk);
    } else {
        clear_swallowed(vk);
    }
    swallow
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && handle(wparam.0, &*(lparam.0 as *const KBDLLHOOKSTRUCT)) {
        return LRESULT(1);
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn install_hook() -> Option<HHOOK> {
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0).ok() }
}

// ---------- Worker thread ----------

struct Session {
    items: Vec<WindowEntry>,
    index: usize,
    shown: bool,
    started: Instant,
    layout: GridLayout,
    /// Window size (design units) that fits the grid.
    window: (f64, f64),
    /// Thumbnail cache directory and the files the UI was told about when preparing.
    dir: Option<PathBuf>,
    known: HashMap<i64, PathBuf>,
}

/// Grid geometry sent to the UI (CSS px).
#[derive(Clone, Copy, Serialize)]
struct GridLayout {
    cols: usize,
    rows: usize,
    card_w: f64,
    card_h: f64,
    gap: f64,
    pad: f64,
}

/// Columns and rows for `n` windows on a screen of `screen` design units, plus the window size
/// that fits them. Many windows wrap into several balanced rows, like the native Alt+Tab.
fn grid_layout(n: usize, screen: (f64, f64)) -> (GridLayout, (f64, f64)) {
    let n = n.max(1);
    let max_w = (screen.0 * MAX_W_FRACTION).min(MAX_PANEL_W);
    let cols_max = (((max_w - 2.0 * GRID_PAD + GRID_GAP) / (CARD_W + GRID_GAP)).floor() as usize).max(1);
    let rows_all = n.div_ceil(cols_max);
    // Balance the rows instead of filling the first one and leaving a single card below.
    let cols = n.div_ceil(rows_all);
    let rows_max =
        (((screen.1 * MAX_H_FRACTION - 2.0 * GRID_PAD + GRID_GAP) / (CARD_H + GRID_GAP)).floor() as usize)
            .max(1);
    let rows = rows_all.min(rows_max);

    let w = cols as f64 * (CARD_W + GRID_GAP) - GRID_GAP + 2.0 * GRID_PAD;
    let h = rows as f64 * (CARD_H + GRID_GAP) - GRID_GAP + 2.0 * GRID_PAD;
    let layout = GridLayout {
        cols,
        rows: rows_all,
        card_w: CARD_W,
        card_h: CARD_H,
        gap: GRID_GAP,
        pad: GRID_PAD,
    };
    (layout, (w, h))
}

/// Primary monitor size in design units.
fn screen_size(app: &AppHandle) -> Option<(f64, f64)> {
    let m = app.get_webview_window("switcher")?.primary_monitor().ok()??;
    let u = m.scale_factor() * edge::ui_scale();
    let size = m.size();
    Some((size.width as f64 / u, size.height as f64 / u))
}

/// One window for the UI: the WindowEntry fields plus the newest thumbnail file, if any.
#[derive(Clone, Serialize)]
struct SwItem<'a> {
    #[serde(flatten)]
    entry: &'a WindowEntry,
    thumb: Option<String>,
}

#[derive(Clone, Serialize)]
struct ItemsPayload<'a> {
    items: Vec<SwItem<'a>>,
    index: usize,
    layout: GridLayout,
}

#[derive(Clone, Serialize)]
struct ThumbPayload {
    hwnd: i64,
    thumb: String,
}

static CAPTURE_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();

/// A single background worker keeps thumbnail capture from competing heavily with the UI.
fn capture_pool() -> &'static rayon::ThreadPool {
    CAPTURE_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("switcher-capture-{i}"))
            .build()
            .expect("switcher capture thread pool")
    })
}

/// Capture fresh thumbnails in the background and tell the UI about every file that changed.
/// `known` is what the preparing payload already contained. Waits for the opening animation to
/// finish first, and does nothing if the panel was closed in the meantime (a quick Alt+Tab tap
/// never captures anything).
fn refresh_thumbs(app: &AppHandle, dir: PathBuf, known: HashMap<i64, PathBuf>, hwnds: Vec<i64>) {
    let app = app.clone();
    capture_pool().spawn(move || {
        use rayon::prelude::*;
        thread::sleep(REFRESH_DELAY);
        hwnds.par_iter().for_each(|&hwnd| {
            if !PANEL_OPEN.load(Ordering::Relaxed)
                || !PREVIEWS_ENABLED.load(Ordering::Relaxed)
            {
                return;
            }
            let Some(path) = capture::refresh(&dir, hwnd) else { return };
            if !PANEL_OPEN.load(Ordering::Relaxed)
                || !PREVIEWS_ENABLED.load(Ordering::Relaxed)
            {
                return;
            }
            if known.get(&hwnd) == Some(&path) {
                return;
            }
            let _ = app.asset_protocol_scope().allow_file(&path);
            let _ = app.emit(
                "switcher-thumb",
                ThumbPayload { hwnd, thumb: path.to_string_lossy().into_owned() },
            );
        });
    });
}

/// Do the expensive work while the panel is still hidden: size the window to the grid and send
/// the list to the UI (which renders it and decodes the images). `show` then only reveals it.
fn prepare(app: &AppHandle, s: &mut Session) {
    let t = Instant::now();
    let size = (s.window.0.round() as i64, s.window.1.round() as i64);
    let changed = LAST_WINDOW
        .lock()
        .map(|mut l| {
            let c = *l != Some(size);
            *l = Some(size);
            c
        })
        .unwrap_or(true);
    edge::set_size_override("switcher", Some(s.window));
    if changed {
        let a = app.clone();
        let _ = app.run_on_main_thread(move || crate::place_panel(&a, "switcher"));
    }

    // Thumbnails already on disk are used right away; fresh ones arrive after the panel is shown.
    s.dir = if PREVIEWS_ENABLED.load(Ordering::Relaxed) {
        capture::cache_dir(app)
    } else {
        None
    };
    s.known = s.dir.as_deref().map(capture::latest_all).unwrap_or_default();
    let scope = app.asset_protocol_scope();
    let items: Vec<SwItem> = s
        .items
        .iter()
        .map(|entry| {
            let thumb = s.known.get(&entry.hwnd).map(|p| {
                let _ = scope.allow_file(p);
                p.to_string_lossy().into_owned()
            });
            SwItem { entry, thumb }
        })
        .collect();
    let _ = app.emit(
        "switcher-items",
        ItemsPayload { items, index: s.index, layout: s.layout },
    );
    if DEBUG_TIMING {
        eprintln!(
            "[alt+tab] prepared {} items in {} ms (window {}x{}, resized: {changed})",
            s.items.len(),
            t.elapsed().as_millis(),
            size.0,
            size.1
        );
    }
}

fn show(app: &AppHandle, s: &mut Session) {
    s.shown = true;
    PANEL_OPEN.store(true, Ordering::Relaxed);
    let _ = app.emit("switcher-select", s.index);
    app.state::<Panels>().set("switcher", true);
    if DEBUG_TIMING {
        eprintln!("[alt+tab] panel requested {} ms after the key press", s.started.elapsed().as_millis());
    }

    refresh_session_thumbs(app, s);
}

fn refresh_session_thumbs(app: &AppHandle, s: &Session) {
    if !PREVIEWS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(dir) = s.dir.clone() {
        refresh_thumbs(app, dir, s.known.clone(), s.items.iter().map(|e| e.hwnd).collect());
    }
}

fn hide_panel(app: &AppHandle) {
    PANEL_OPEN.store(false, Ordering::Relaxed);
    app.state::<Panels>().set("switcher", false);
}

/// Tell the UI which item is selected (showing the panel first if it is not visible yet).
fn select(app: &AppHandle, s: &mut Session) {
    if s.shown {
        let _ = app.emit("switcher-select", s.index);
    } else {
        show(app, s); // user is cycling through items: show it now
    }
}

fn step(app: &AppHandle, session: &mut Option<Session>, delta: i32) {
    let Some(s) = session.as_mut() else { return };
    let n = s.items.len() as i32;
    if n == 0 {
        return;
    }
    s.index = (((s.index as i32 + delta) % n + n) % n) as usize;
    select(app, s);
}

/// Up/down: move one row, wrapping to the other end of the same column.
fn step_row(app: &AppHandle, session: &mut Option<Session>, delta: i32) {
    let Some(s) = session.as_mut() else { return };
    let n = s.items.len();
    if n == 0 {
        return;
    }
    let cols = s.layout.cols.max(1);
    let cur = s.index;
    s.index = if delta > 0 {
        if cur + cols < n { cur + cols } else { cur % cols }
    } else if cur >= cols {
        cur - cols
    } else {
        let last_row_start = ((n - 1) / cols) * cols;
        (last_row_start + cur % cols).min(n - 1)
    };
    select(app, s);
}

/// End the session: hide the panel and switch to the selected window.
fn finish(app: &AppHandle, session: &mut Option<Session>) {
    let Some(s) = session.take() else { return };
    if s.shown {
        hide_panel(app);
    }
    if let Some(w) = s.items.get(s.index) {
        winlist::activate_window(w.hwnd);
    }
}

fn cancel(app: &AppHandle, session: &mut Option<Session>) {
    if let Some(s) = session.take() {
        if s.shown {
            hide_panel(app);
        }
    }
}

fn handle_msg(app: &AppHandle, session: &mut Option<Session>, msg: Msg) {
    match msg {
        Msg::Start { reverse } => {
            // First, suppress the active app's menu bar while Alt is still held.
            unsafe { mask_alt_menu() };

            let t = Instant::now();
            let items = winlist::collect_windows();
            let ms = t.elapsed().as_millis();
            if ms >= SLOW_LIST_MS {
                eprintln!("[alt+tab] daftar window lambat: {} item dalam {ms} ms", items.len());
            }
            if items.is_empty() {
                ACTIVE.store(false, Ordering::Relaxed);
                *session = None;
                return;
            }
            let n = items.len();
            // The first item is the active window, so the initial selection is the next window.
            // If the active window is not in the list (e.g. the desktop), start with the first item.
            let base = if items[0].focused { 1 } else { 0 };
            let index = if n == 1 {
                0
            } else if reverse {
                n - 1
            } else {
                base.min(n - 1)
            };
            let (layout, window) = grid_layout(n, screen_size(app).unwrap_or((1536.0, 864.0)));
            let mut s = Session {
                items,
                index,
                shown: false,
                started: Instant::now(),
                layout,
                window,
                dir: None,
                known: HashMap::new(),
            };
            // Expensive work happens now, before the panel is revealed (see prepare).
            prepare(app, &mut s);
            *session = Some(s);
        }
        Msg::Next { reverse } => step(app, session, if reverse { -1 } else { 1 }),
        Msg::SetPreviews(enabled) => {
            if let Some(s) = session.as_mut() {
                prepare(app, s);
                if enabled && s.shown {
                    refresh_session_thumbs(app, s);
                }
            }
        }
        Msg::Step(d) => step(app, session, d),
        Msg::StepRow(d) => step_row(app, session, d),
        Msg::Cancel => cancel(app, session),
        Msg::Commit => finish(app, session),
        Msg::Pick(i) => {
            ACTIVE.store(false, Ordering::Relaxed);
            if let Some(s) = session.as_mut() {
                s.index = i.min(s.items.len().saturating_sub(1));
            }
            finish(app, session);
        }
    }
}

fn worker(app: AppHandle, rx: Receiver<Msg>) {
    let mut session: Option<Session> = None;
    loop {
        // No session: sleep until a message arrives (no polling while idle).
        let msg = if let Some(s) = session.as_ref() {
            // Wake exactly when the panel is due (SHOW_DELAY) instead of on a 40 ms grid.
            let tick = Duration::from_millis(40);
            let wait = if s.shown {
                tick
            } else {
                SHOW_DELAY
                    .saturating_sub(s.started.elapsed())
                    .clamp(Duration::from_millis(1), tick)
            };
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        if let Some(m) = msg {
            handle_msg(&app, &mut session, m);
        }

        let Some(s) = session.as_mut() else { continue };
        let age = s.started.elapsed();
        if age >= ALT_POLL_GRACE && !async_down(VK_MENU) {
            // Alt-up was missed (hook removed, desktop is safe, etc.): consider the session finished.
            ACTIVE.store(false, Ordering::Relaxed);
            finish(&app, &mut session);
        } else if !s.shown && age >= SHOW_DELAY {
            show(&app, s);
        }
    }
}

/// Watchdog: if Alt+Tab is physically pressed but the hook never sees Tab, Windows may have
/// removed the hook (or the active window may be running as administrator).
/// Reinstall it no more than once per REINSTALL_COOLDOWN_MS.
fn watchdog() {
    let mut last_reinstall: Option<u64> = None;
    let mut combo: Option<(u64, u64, bool)> = None;
    loop {
        thread::sleep(WATCHDOG_POLL);
        if DIAG.swap(0, Ordering::Relaxed) & 1 != 0 {
            eprintln!("[alt+tab] Alt+Tab handed to Windows: Ctrl or Win was held down");
        }
        let trace = TAB_TRACE.swap(0, Ordering::Relaxed);
        let alt_context =
            trace & (TRACE_ALT_FLAG | TRACE_ALT_ASYNC) != 0 || async_down(VK_MENU) && async_down(VK_TAB);
        if DEBUG_TIMING && trace & TRACE_PRESENT != 0 && alt_context {
            let f = |bit: u32| trace & bit != 0;
            eprintln!(
                "[alt+tab] Tab with Alt: enabled={} injected={} alt_flag={} alt_async={} ctrl={} win={} \
                 session_active={} -> swallowed={}",
                f(TRACE_ENABLED),
                f(TRACE_INJECTED),
                f(TRACE_ALT_FLAG),
                f(TRACE_ALT_ASYNC),
                f(TRACE_CTRL),
                f(TRACE_WIN),
                f(TRACE_ACTIVE),
                f(TRACE_SWALLOWED),
            );
        }
        if !ENABLED.load(Ordering::Relaxed) {
            combo = None;
            continue;
        }
        if !(async_down(VK_MENU) && async_down(VK_TAB)) {
            combo = None;
            continue;
        }
        let now = now_ms();
        let last_tab_seen = LAST_TAB_SEEN.load(Ordering::Relaxed);
        let (started, seen_at_start, mut hook_saw_tab) = *combo.get_or_insert_with(|| {
            (
                now,
                last_tab_seen,
                last_tab_seen != 0 && now.saturating_sub(last_tab_seen) <= RECENT_TAB_SEEN_MS,
            )
        });
        if last_tab_seen != seen_at_start {
            hook_saw_tab = true;
        }
        combo = Some((started, seen_at_start, hook_saw_tab));
        let cooldown_elapsed = last_reinstall
            .map(|last| now.saturating_sub(last) >= REINSTALL_COOLDOWN_MS)
            .unwrap_or(true);
        if !hook_saw_tab && now.saturating_sub(started) >= MISSED_TAB_GRACE_MS && cooldown_elapsed {
            last_reinstall = Some(now);
            combo = None;
            eprintln!("[alt+tab] Tab terlewat; memasang ulang hook");
            reinstall();
            if async_down(VK_MENU) && async_down(VK_TAB) {
                eprintln!("[alt+tab] membatalkan switcher Windows dan membuka switcher kustom");
                if !unsafe { dismiss_native_switcher() } {
                    eprintln!("[alt+tab] gagal membatalkan switcher Windows setelah Tab terlewat");
                }
                let reverse = async_down(VK_SHIFT);
                if ACTIVE.swap(true, Ordering::Relaxed) {
                    send(Msg::Next { reverse });
                } else {
                    send(Msg::Start { reverse });
                }
            }
        }
    }
}

/// Dev aid: the UI prints a line to the terminal (frame-timing probe in Switcher.tsx).
#[tauri::command]
pub fn dev_log(line: String) {
    eprintln!("[ui] {line}");
}

/// Click a panel item to select and activate that window.
#[tauri::command]
pub fn switcher_pick(index: usize) {
    send(Msg::Pick(index));
}

/// Enable or disable thumbnail previews for the current process.
#[tauri::command]
pub fn switcher_set_previews(enabled: bool) {
    PREVIEWS_ENABLED.store(enabled, Ordering::Relaxed);
    send(Msg::SetPreviews(enabled));
}

#[tauri::command]
pub fn switcher_get_previews() -> bool {
    PREVIEWS_ENABLED.load(Ordering::Relaxed)
}

/// Install the hook and start the worker and watchdog threads.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let _ = TX.set(tx);
    let (hook_ready_tx, hook_ready_rx) = mpsc::sync_channel(1);
    thread::spawn(move || worker(app, rx));
    thread::spawn(watchdog);

    thread::spawn(move || unsafe {
        // The hook thread must be scheduled immediately; if delayed, the callback times out
        // and Windows removes the hook.
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        HOOK_THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);

        let mut hook = install_hook();
        if hook.is_none() {
            eprintln!("[alt+tab] gagal memasang hook (Alt+Tab bawaan tetap aktif)");
        }
        let _ = hook_ready_tx.send(hook.is_some());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_REINSTALL {
                if let Some(h) = hook.take() {
                    let _ = UnhookWindowsHookEx(h);
                }
                hook = install_hook();
                eprintln!(
                    "[alt+tab] hook dipasang ulang: {}",
                    if hook.is_some() { "ok" } else { "gagal" }
                );
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });

    if let Err(error) = hook_ready_rx.recv_timeout(Duration::from_secs(2)) {
        eprintln!("[alt+tab] menunggu hook keyboard terlalu lama: {error}");
    }
}
