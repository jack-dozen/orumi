//! Auto-hide taskbar Windows lewat API resmi (ABM_SETSTATE), dengan pemulihan tahan-crash:
//! keadaan asli disimpan di file penanda, dipulihkan saat keluar normal, dan oleh proses
//! penjaga kecil kalau app mati paksa. Polanya mengikuti Bloom (GPL-3.0): penanda + pengawas.
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
    /// Nilai ABM_GETSTATE sebelum kita mengubahnya.
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

/// Kembalikan taskbar ke keadaan asli kalau ada penanda, lalu hapus penandanya.
fn restore_from(path: &Path) {
    if let Some(m) = read_marker(path) {
        set_state(m.original_state);
    }
    let _ = fs::remove_file(path);
}

/// Nyalakan atau matikan auto-hide. Aman dipanggil berulang.
pub fn apply(app: &AppHandle, enabled: bool) {
    let Some(path) = marker_path(app) else { return };
    if !enabled {
        restore_from(&path);
        return;
    }

    // Penanda yang masih ada berarti sesi sebelumnya tidak sempat memulihkan:
    // keadaan asli yang sebenarnya ada di penanda, bukan yang terbaca sekarang.
    let original = match read_marker(&path) {
        Some(m) => m.original_state,
        None => match get_state() {
            Some(s) => s,
            None => return,
        },
    };

    if original & ABS_AUTOHIDE != 0 {
        // Pengguna memang sudah memakai auto-hide: tidak ada yang perlu diubah atau dipulihkan.
        let _ = fs::remove_file(&path);
        return;
    }

    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&Marker { original_state: original }) {
        if fs::write(&path, text).is_err() {
            return; // tanpa penanda, jangan mengubah apa pun
        }
    }
    set_state(original | ABS_AUTOHIDE);
}

/// Dipanggil saat app keluar secara normal.
pub fn restore(app: &AppHandle) {
    if let Some(path) = marker_path(app) {
        restore_from(&path);
    }
}

/// Jalankan salinan kecil diri sendiri yang menunggu proses ini berakhir lalu memulihkan.
/// DETACHED_PROCESS: tidak ikut mati oleh Ctrl+C atau penutupan terminal.
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

/// Mode penjaga (`--guard <pid> <penanda>`): tanpa UI. Menunggu proses induk berakhir,
/// lalu memulihkan flyout Windows dan taskbar.
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
