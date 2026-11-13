//! Windows taskbar control with crash-safe recovery. Two modes:
//! - auto-hide through the official API (ABM_SETSTATE);
//! - fully hidden (stage 2): auto-hide plus `SW_HIDE` and moving the taskbar windows off-screen.
//!   The pattern follows Bloom (GPL-3.0): utils.rs `set_taskbar_visibility` + `open_system_tray`.
//! The original state and window positions are saved in a marker file, restored on normal exit,
//! and by a small watchdog if the app is forcibly terminated.
//!
//! A hidden taskbar has no UI Automation tree, so the tray chevron cannot be invoked. The tray
//! flyout temporarily reveals the taskbar fully transparent and click-through (`reveal_for_tray`),
//! then hides it again once the flyout is closed (`conceal_after_close`).
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    fs,
    mem::size_of,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::{
    core::{w, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, COLORREF, HWND, LPARAM, RECT},
        System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE},
        UI::{
            Shell::{SHAppBarMessage, APPBARDATA},
            WindowsAndMessaging::{
                EnumWindows, FindWindowW, GetClassNameW, GetWindowLongW, GetWindowRect,
                IsWindowVisible, SetLayeredWindowAttributes, SetWindowLongW, SetWindowPos,
                ShowWindow, GWL_EXSTYLE, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
                SW_HIDE, SW_SHOW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
            },
        },
    },
};

const ABM_GETSTATE: u32 = 4;
const ABM_GETTASKBARPOS: u32 = 5;
const ABM_SETSTATE: u32 = 10;
const ABS_AUTOHIDE: u32 = 1;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Park position for hidden taskbar windows (far off-screen, avoids a thin line artifact).
const PARK: i32 = -10000;

/// Hidden mode is active: the poller keeps the taskbar windows hidden.
static HIDDEN: AtomicBool = AtomicBool::new(false);
/// Number of tray reveals in progress; the poller and `hide_windows` wait while this is > 0.
static REVEAL: AtomicU32 = AtomicU32::new(0);
static POLLER_STARTED: AtomicBool = AtomicBool::new(false);
/// Original top-left positions of taskbar windows, keyed by HWND value.
static RECTS: Mutex<Option<HashMap<isize, (i32, i32)>>> = Mutex::new(None);

#[derive(Serialize, Deserialize)]
struct Marker {
    /// ABM_GETSTATE value before we changed it.
    original_state: u32,
    /// Original positions (hwnd, left, top) of taskbar windows, saved before hiding them.
    #[serde(default)]
    rects: Vec<(isize, i32, i32)>,
}

fn marker_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("taskbar-restore.json"))
}

fn read_marker(path: &Path) -> Option<Marker> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

unsafe fn appbar(msg: u32, lparam: u32) -> Option<usize> {
    let hwnd = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
    let mut abd: APPBARDATA = std::mem::zeroed();
    abd.cbSize = size_of::<APPBARDATA>() as u32;
    abd.hWnd = hwnd;
    abd.lParam = LPARAM(lparam as isize);
    Some(SHAppBarMessage(msg, &mut abd))
}

fn get_state() -> Option<u32> {
    unsafe { appbar(ABM_GETSTATE, 0) }.map(|s| s as u32)
}

fn set_state(state: u32) {
    let _ = unsafe { appbar(ABM_SETSTATE, state) };
}

// ---------- Taskbar windows ----------

fn primary_hwnd() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok() }
}

/// Primary taskbar plus every secondary taskbar (one per extra monitor).
fn taskbar_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<HWND>);
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class);
        if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "Shell_SecondaryTrayWnd" {
            list.push(hwnd);
        }
        BOOL(1)
    }

    let mut list = Vec::new();
    if let Some(p) = primary_hwnd() {
        list.push(p);
    }
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut Vec<HWND> as isize));
    }
    list
}

/// Window position if Explorer has actually placed it on screen (not parked, not empty).
fn placed_pos(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    (r.right > r.left && r.bottom > r.top && r.left > -9000 && r.top > -9000)
        .then_some((r.left, r.top))
}

fn with_rects<T>(f: impl FnOnce(&mut HashMap<isize, (i32, i32)>) -> T) -> T {
    let mut guard = RECTS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// Remember the current position of every taskbar window that is on screen.
fn remember_placed() {
    for hwnd in taskbar_windows() {
        if let Some(pos) = placed_pos(hwnd) {
            with_rects(|m| m.insert(hwnd.0 as isize, pos));
        }
    }
}

fn saved_pos(hwnd: HWND) -> Option<(i32, i32)> {
    if let Some(pos) = with_rects(|m| m.get(&(hwnd.0 as isize)).copied()) {
        return Some(pos);
    }
    // Fallback for the primary taskbar: ask Explorer for its AppBar position.
    if primary_hwnd() == Some(hwnd) {
        unsafe {
            let mut abd: APPBARDATA = std::mem::zeroed();
            abd.cbSize = size_of::<APPBARDATA>() as u32;
            abd.hWnd = hwnd;
            if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) != 0
                && abd.rc.right > abd.rc.left
                && abd.rc.left > -9000
                && abd.rc.top > -9000
            {
                return Some((abd.rc.left, abd.rc.top));
            }
        }
    }
    None
}

/// Make the window fully transparent and click-through (alpha 0), so it can be "shown" invisibly.
unsafe fn make_invisible(hwnd: HWND) {
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
    SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32 | WS_EX_TRANSPARENT.0 as i32);
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
}

/// Undo `make_invisible` (also cleans up leftovers from a killed session).
unsafe fn make_opaque(hwnd: HWND) {
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
    let cleaned = ex & !(WS_EX_LAYERED.0 as i32) & !(WS_EX_TRANSPARENT.0 as i32);
    if cleaned != ex {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        SetWindowLongW(hwnd, GWL_EXSTYLE, cleaned);
    }
}

unsafe fn move_to(hwnd: HWND, x: i32, y: i32) {
    let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Hide every taskbar window: `SW_HIDE` and park far off-screen. Saves original positions first.
fn hide_windows() {
    remember_placed();
    for hwnd in taskbar_windows() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            move_to(hwnd, PARK, PARK);
        }
    }
}

/// Bring every taskbar window back to its saved position and make it visible.
fn show_windows() {
    for hwnd in taskbar_windows() {
        unsafe {
            make_opaque(hwnd);
            if let Some((x, y)) = saved_pos(hwnd) {
                move_to(hwnd, x, y);
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
}

fn any_parked() -> bool {
    taskbar_windows().into_iter().any(|h| placed_pos(h).is_none())
}

/// Keeps the taskbar hidden: Explorer can re-show it (Win key, Explorer restart, new monitor).
fn start_poller() {
    if POLLER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(500));
        if !HIDDEN.load(Ordering::Relaxed) || REVEAL.load(Ordering::Relaxed) > 0 {
            continue;
        }
        let leaked = taskbar_windows()
            .into_iter()
            .any(|h| unsafe { IsWindowVisible(h).as_bool() } || placed_pos(h).is_some());
        if leaked {
            hide_windows();
        }
    });
}

// ---------- Marker ----------

fn write_marker(path: &Path, original: u32) -> bool {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let rects = with_rects(|m| m.iter().map(|(h, (x, y))| (*h, *x, *y)).collect());
    match serde_json::to_string(&Marker { original_state: original, rects }) {
        Ok(text) => fs::write(path, text).is_ok(),
        Err(_) => false,
    }
}

/// Restore the taskbar to its original state if a marker exists, then remove the marker.
fn restore_from(path: &Path) {
    HIDDEN.store(false, Ordering::SeqCst);
    if let Some(m) = read_marker(path) {
        with_rects(|r| {
            for (h, x, y) in m.rects {
                r.entry(h).or_insert((x, y));
            }
        });
        // Show first, then restore the AppBar state, so Explorer lays out a visible window.
        show_windows();
        set_state(m.original_state);
        if any_parked() {
            // Explorer did not move it back by itself: force a relayout.
            set_state(m.original_state | ABS_AUTOHIDE);
            set_state(m.original_state);
        }
    }
    let _ = fs::remove_file(path);
}

/// Apply the taskbar mode. Safe to call repeatedly.
/// `hidden` (full hide) takes precedence over `autohide`; both false = leave the taskbar alone.
pub fn apply(app: &AppHandle, autohide: bool, hidden: bool) {
    let Some(path) = marker_path(app) else { return };
    if !autohide && !hidden {
        restore_from(&path);
        return;
    }

    // A remaining marker means the previous session did not restore the state:
    // the true original state is in the marker, not the current reading.
    let marker = read_marker(&path);
    let original = match &marker {
        Some(m) => m.original_state,
        None => match get_state() {
            Some(s) => s,
            None => return,
        },
    };

    if !hidden {
        // Plain auto-hide (also when leaving hidden mode): windows must be visible again.
        if let Some(m) = marker {
            with_rects(|r| {
                for (h, x, y) in m.rects {
                    r.entry(h).or_insert((x, y));
                }
            });
        }
        if HIDDEN.swap(false, Ordering::SeqCst) || any_parked() {
            show_windows();
        }
        if original & ABS_AUTOHIDE != 0 {
            // The user already had auto-hide enabled: nothing needs to be changed or restored.
            set_state(original);
            let _ = fs::remove_file(&path);
            return;
        }
        if !write_marker(&path, original) {
            return; // without a marker, do not change anything
        }
        set_state(original | ABS_AUTOHIDE);
        return;
    }

    // Hidden mode. Positions from an old marker first, then whatever is on screen now.
    if let Some(m) = marker {
        with_rects(|r| {
            for (h, x, y) in m.rects {
                r.entry(h).or_insert((x, y));
            }
        });
    }
    remember_placed();
    if !write_marker(&path, original) {
        return; // without a marker, do not change anything
    }
    set_state(original | ABS_AUTOHIDE);
    HIDDEN.store(true, Ordering::SeqCst);
    hide_windows();
    start_poller();
}

/// Called when the app exits normally.
pub fn restore(app: &AppHandle) {
    if let Some(path) = marker_path(app) {
        restore_from(&path);
    }
}

// ---------- Tray flyout support ----------

/// Temporarily make the hidden taskbar available to UI Automation (alpha 0, click-through).
/// Returns true if a reveal started; pair it with `conceal_after_close`.
pub fn reveal_for_tray() -> bool {
    if !HIDDEN.load(Ordering::SeqCst) {
        return false;
    }
    REVEAL.fetch_add(1, Ordering::SeqCst);
    for hwnd in taskbar_windows() {
        unsafe {
            make_invisible(hwnd);
            if let Some((x, y)) = saved_pos(hwnd) {
                move_to(hwnd, x, y);
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
    thread::sleep(Duration::from_millis(80)); // let Explorer realize the taskbar is back
    true
}

/// Hide the taskbar again once the flyout is gone. `is_open` reports whether the flyout is visible;
/// `wait_for_open` waits (up to 3 s) for it to appear first. Runs on its own thread.
pub fn conceal_after_close(is_open: fn() -> bool, wait_for_open: bool) {
    thread::spawn(move || {
        if wait_for_open {
            for _ in 0..30 {
                if is_open() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        while is_open() {
            thread::sleep(Duration::from_millis(200));
        }
        let last = REVEAL.fetch_sub(1, Ordering::SeqCst) == 1;
        if last && HIDDEN.load(Ordering::SeqCst) {
            hide_windows();
            for hwnd in taskbar_windows() {
                unsafe { make_opaque(hwnd) };
            }
        }
    });
}

/// Launch a small copy of this process that waits for it to exit, then restores the state.
/// DETACHED_PROCESS: it survives Ctrl+C and terminal closure.
pub fn spawn_guard(app: &AppHandle) {
    let Some(marker) = marker_path(app) else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = Command::new(exe)
        .arg("--guard")
        .arg(std::process::id().to_string())
        .arg(marker)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Watchdog mode (`--guard <pid> <marker>`): no UI. Wait for the parent process to exit,
/// then restore the Windows flyout and taskbar.
pub fn run_guard(args: &[String]) {
    let Some(i) = args.iter().position(|a| a == "--guard") else { return };
    let pid: u32 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let marker = args.get(i + 2).map(PathBuf::from);

    if pid != 0 {
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                let _ = WaitForSingleObject(handle, INFINITE);
                let _ = CloseHandle(handle);
            }
        }
    }

    crate::osd::restore();
    if let Some(m) = marker {
        restore_from(&m);
    }
}
