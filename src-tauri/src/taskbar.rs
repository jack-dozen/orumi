//! Auto-hide the Windows taskbar through the official API (ABM_SETSTATE), with crash-safe recovery:
//! the original state is saved in a marker file, restored on normal exit, and by a small watchdog
//! if the app is forcibly terminated. The pattern follows Bloom (GPL-3.0): marker + watchdog.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    fs,
    mem::size_of,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, LPARAM},
        System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE},
        UI::{
            Shell::{SHAppBarMessage, APPBARDATA},
            WindowsAndMessaging::FindWindowW,
        },
    },
};

const ABM_GETSTATE: u32 = 4;
const ABM_SETSTATE: u32 = 10;
const ABS_AUTOHIDE: u32 = 1;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[derive(Serialize, Deserialize)]
struct Marker {
    /// ABM_GETSTATE value before we changed it.
    original_state: u32,
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

/// Restore the taskbar to its original state if a marker exists, then remove the marker.
fn restore_from(path: &Path) {
    if let Some(m) = read_marker(path) {
        set_state(m.original_state);
    }
    let _ = fs::remove_file(path);
}

/// Enable or disable auto-hide. Safe to call repeatedly.
pub fn apply(app: &AppHandle, enabled: bool) {
    let Some(path) = marker_path(app) else { return };
    if !enabled {
        restore_from(&path);
        return;
    }

    // A remaining marker means the previous session did not restore the state:
    // the true original state is in the marker, not the current reading.
    let original = match read_marker(&path) {
        Some(m) => m.original_state,
        None => match get_state() {
            Some(s) => s,
            None => return,
        },
    };

    if original & ABS_AUTOHIDE != 0 {
        // The user already had auto-hide enabled: nothing needs to be changed or restored.
        let _ = fs::remove_file(&path);
        return;
    }

    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&Marker { original_state: original }) {
        if fs::write(&path, text).is_err() {
            return; // without a marker, do not change anything
        }
    }
    set_state(original | ABS_AUTOHIDE);
}

/// Called when the app exits normally.
pub fn restore(app: &AppHandle) {
    if let Some(path) = marker_path(app) {
        restore_from(&path);
    }
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
