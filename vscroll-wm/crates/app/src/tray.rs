//! Ikon tray (menu klik kanan: Jeda / Jalankan saat login / Keluar) dan
//! pengaturan autostart lewat registry HKCU\...\Run.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Pesan dari ikon tray ke window tersembunyi kita.
const WM_TRAY: u32 = WM_APP + 4;
/// Dikirim ke thread utama: ganti status jeda.
pub const WM_TRAY_PAUSE: u32 = WM_APP + 5;
/// Dikirim ke thread utama: muat ulang file konfigurasi.
pub const WM_TRAY_RELOAD: u32 = WM_APP + 6;
/// Dikirim ke thread utama: buka / tutup cheatsheet.
pub const WM_TRAY_HELP: u32 = WM_APP + 12;

/// True = app dijeda: window dikembalikan ke posisi asli dan semua hotkey
/// diteruskan ke Windows seperti biasa.
pub static PAUSED: AtomicBool = AtomicBool::new(false);

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static ICON: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

const ID_PAUSE: usize = 1;
const ID_AUTOSTART: usize = 2;
const ID_QUIT: usize = 3;
const ID_RELOAD: usize = 4;
const ID_HELP: usize = 5;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "vscroll-wm";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---------------------------------------------------------------- autostart

fn reg() -> Command {
    let mut c = Command::new("reg");
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

pub fn autostart_enabled() -> bool {
    reg()
        .args(["query", RUN_KEY, "/v", RUN_VALUE])
        .output()
        .map_or(false, |o| o.status.success())
}

pub fn set_autostart(on: bool) -> bool {
    let out = if on {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let value = format!("\"{}\"", exe.display());
        reg()
            .args([
                "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &value, "/f",
            ])
            .output()
    } else {
        reg()
            .args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])
            .output()
    };
    out.map_or(false, |o| o.status.success())
}

// --------------------------------------------------------------------- ikon

/// Ikon 32x32 gambar sendiri: dua panel kiri bertumpuk + satu panel kanan tinggi.
/// Simetris vertikal, jadi urutan baris bitmap tidak berpengaruh.
unsafe fn make_icon() -> Option<HICON> {
    const N: usize = 32;
    let in_rect = |x: usize, y: usize, x0: usize, x1: usize, y0: usize, y1: usize| {
        x >= x0 && x < x1 && y >= y0 && y < y1
    };
    let mut xor = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let pane = in_rect(x, y, 3, 15, 3, 15)
                || in_rect(x, y, 3, 15, 17, 29)
                || in_rect(x, y, 17, 29, 3, 29);
            let (b, g, r) = if pane {
                (0xF5, 0xA8, 0x4F)
            } else {
                (0x30, 0x26, 0x20)
            };
            let i = (y * N + x) * 4;
            xor[i] = b;
            xor[i + 1] = g;
            xor[i + 2] = r;
            xor[i + 3] = 0xFF;
        }
    }
    let and = vec![0u8; N * N / 8];
    CreateIcon(None, N as i32, N as i32, 1, 32, and.as_ptr(), xor.as_ptr()).ok()
}

unsafe fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut n: NOTIFYICONDATAW = std::mem::zeroed();
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = 1;
    n
}

fn copy_tip(n: &mut NOTIFYICONDATAW, text: &str) {
    let w: Vec<u16> = text.encode_utf16().take(127).collect();
    n.szTip = [0; 128];
    n.szTip[..w.len()].copy_from_slice(&w);
}

unsafe fn add_icon(hwnd: HWND) {
    let icon = HICON(ICON.load(Ordering::Relaxed) as *mut _);
    let mut n = nid(hwnd);
    n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = icon;
    copy_tip(&mut n, "vscroll-wm");
    let _ = Shell_NotifyIconW(NIM_ADD, &n);
}

pub unsafe fn set_tip(text: &str) {
    let hwnd = HWND(TRAY_HWND.load(Ordering::Relaxed) as *mut _);
    let mut n = nid(hwnd);
    n.uFlags = NIF_TIP;
    copy_tip(&mut n, text);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
}

pub unsafe fn remove() {
    let hwnd = HWND(TRAY_HWND.load(Ordering::Relaxed) as *mut _);
    if !hwnd.0.is_null() {
        let n = nid(hwnd);
        let _ = Shell_NotifyIconW(NIM_DELETE, &n);
    }
}

// --------------------------------------------------------------------- menu

unsafe fn post_main(msg: u32) {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        msg,
        WPARAM(0),
        LPARAM(0),
    );
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let paused = PAUSED.load(Ordering::Relaxed);
    let pause_txt = wide(if paused { "Resume" } else { "Pause" });
    let reload_txt = wide("Reload configuration");
    let help_txt = wide("Keyboard shortcuts");
    let auto_txt = wide("Run at login");
    let quit_txt = wide("Quit (restore windows)");

    let _ = AppendMenuW(menu, MF_STRING, ID_PAUSE, PCWSTR(pause_txt.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, PCWSTR(reload_txt.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_HELP, PCWSTR(help_txt.as_ptr()));
    let auto_flags = if autostart_enabled() {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let _ = AppendMenuW(menu, auto_flags, ID_AUTOSTART, PCWSTR(auto_txt.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(quit_txt.as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // wajib supaya menu tertutup saat user mengklik di tempat lain
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);

    match cmd.0 as usize {
        ID_PAUSE => post_main(WM_TRAY_PAUSE),
        ID_RELOAD => post_main(WM_TRAY_RELOAD),
        ID_HELP => post_main(WM_TRAY_HELP),
        ID_AUTOSTART => {
            let _ = set_autostart(!autostart_enabled());
        }
        ID_QUIT => post_main(crate::safety::WM_QUIT_REQUEST),
        _ => {}
    }
}

unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY {
        let ev = lparam.0 as u32;
        if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP || ev == WM_CONTEXTMENU {
            show_menu(hwnd);
        }
        return LRESULT(0);
    }
    let tb = TASKBAR_CREATED.load(Ordering::Relaxed);
    if tb != 0 && msg == tb {
        add_icon(hwnd); // explorer di-restart: pasang ikon lagi
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Buat window tersembunyi penerima pesan tray dan pasang ikonnya.
/// Harus dipanggil dari thread yang menjalankan message loop.
pub unsafe fn create(main_thread: u32) -> bool {
    MAIN_THREAD.store(main_thread, Ordering::Relaxed);
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmTray");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(tray_proc),
        hInstance: hinst,
        lpszClassName: class,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return false;
    }
    // Window top-level (bukan message-only) supaya menerima broadcast "TaskbarCreated".
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        class,
        w!("vscroll-wm"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(hinst),
        None,
    ) else {
        return false;
    };
    TRAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    TASKBAR_CREATED.store(
        RegisterWindowMessageW(w!("TaskbarCreated")),
        Ordering::Relaxed,
    );

    let icon = make_icon().unwrap_or_else(|| LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    ICON.store(icon.0 as isize, Ordering::Relaxed);
    add_icon(hwnd);
    true
}
