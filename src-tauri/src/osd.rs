//! Menyembunyikan flyout volume/brightness bawaan Windows 11.
//! Diadaptasi dari Bloom (GPL-3.0): github.com/SehajveerSingh2005/bloom, services.rs.
//!
//! Flyout-nya window `XamlExplorerHostIslandWindow` milik Explorer. Bukan disembunyikan,
//! tapi dibuat transparan penuh dan tembus klik, jadi Windows tetap merasa
//! menampilkannya sementara tidak ada yang terlihat.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HWND, RECT},
        UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetLayeredWindowAttributes, GetSystemMetrics,
            GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, SetLayeredWindowAttributes,
            SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN, WS_EX_LAYERED,
            WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
        },
    },
};

/// false setelah restore() dipanggil, supaya thread tidak menyembunyikannya lagi saat keluar.
static ACTIVE: AtomicBool = AtomicBool::new(true);

/// Window kelas yang sama juga dipakai Alt+Tab dan Task View, tapi itu layar penuh,
/// jadi dikecualikan lewat pengecekan ukuran.
unsafe fn native_osd_windows() -> Vec<HWND> {
    let mut found = Vec::new();
    let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) else {
        return found;
    };
    let mut explorer_pid = 0u32;
    GetWindowThreadProcessId(tray, Some(&mut explorer_pid));
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);

    let mut after: Option<HWND> = None;
    while let Ok(hwnd) = FindWindowExW(
        None,
        after,
        w!("XamlExplorerHostIslandWindow"),
        PCWSTR::null(),
    ) {
        after = Some(hwnd);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut rect = RECT::default();
        let small = GetWindowRect(hwnd, &mut rect).is_ok()
            && (rect.right - rect.left) < screen_w / 2
            && (rect.bottom - rect.top) < screen_h / 2;

        if pid == explorer_pid && (ex_style & WS_EX_NOACTIVATE.0) != 0 && small {
            found.push(hwnd);
        }
    }
    found
}

unsafe fn set_suppressed(suppress: bool) {
    for hwnd in native_osd_windows() {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut alpha = 255u8;
        // Explorer tidak pernah menjadikan window ini layered, jadi layered dengan alpha 0
        // pasti ulah kita, termasuk dari sesi sebelumnya yang crash.
        let suppressed = (ex_style & WS_EX_LAYERED.0) != 0
            && GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), None).is_ok()
            && alpha == 0;

        if suppress && !suppressed {
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                (ex_style | WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32,
            );
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        } else if !suppress && suppressed {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                (ex_style & !(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0)) as i32,
            );
        }
    }
}

/// Jalankan terus: Explorer bisa membuat ulang window flyout kapan saja.
pub fn start() {
    thread::spawn(|| {
        while ACTIVE.load(Ordering::Relaxed) {
            unsafe { set_suppressed(true) };
            thread::sleep(Duration::from_millis(500));
        }
    });
}

/// Kembalikan flyout seperti semula. Dipanggil saat app keluar.
pub fn restore() {
    ACTIVE.store(false, Ordering::Relaxed);
    unsafe { set_suppressed(false) };
}
