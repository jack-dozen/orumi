//! Helper Win32: posisi dan ukuran window, judul, dan filter "layak ditata".

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use layout::{Rect, WindowId};

use windows::core::{w, BOOL, HRESULT, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE,
    HLOCAL, HWND, LPARAM, RECT,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{rules, safety};

pub(crate) fn wid(h: HWND) -> WindowId {
    WindowId(h.0 as usize as u64)
}

/// Kenapa window ini TIDAK ditata (None = boleh ditata).
pub(crate) unsafe fn unmanaged_reason(hwnd: HWND) -> Option<&'static str> {
    if !IsWindowVisible(hwnd).as_bool() {
        return Some("not visible");
    }
    if IsIconic(hwnd).as_bool() {
        return Some("minimized");
    }
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if style & WS_CAPTION.0 != WS_CAPTION.0 {
        return Some("no title bar (WS_CAPTION)");
    }
    // tidak bisa di-resize (dialog, installer, utilitas kecil): biarkan mengambang
    if style & WS_THICKFRAME.0 == 0 {
        return Some("cannot be resized (no WS_THICKFRAME)");
    }
    if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
        return Some("tool window / cannot be activated");
    }
    if GetWindow(hwnd, GW_OWNER).map_or(false, |o| o != HWND::default()) {
        return Some("dialog / popup owned by another window");
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return Some("empty title");
    }
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut c_void,
        4,
    );
    if cloaked != 0 {
        return Some("cloaked (desktop virtual lain / app UWP tersembunyi)");
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
        return Some("jendela app ini sendiri");
    }
    rules::ignored(hwnd)
}

pub(crate) unsafe fn manageable(hwnd: HWND) -> bool {
    unmanaged_reason(hwnd).is_none()
}

pub(crate) unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    if manageable(hwnd) {
        list.push(hwnd);
    }
    BOOL(1)
}

pub(crate) unsafe fn work_area() -> Rect {
    let mut r = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut r as *mut RECT as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    Rect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    }
}

pub(crate) unsafe fn ext_bounds(hwnd: HWND) -> RECT {
    let mut ext = RECT::default();
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut ext as *mut RECT as *mut c_void,
        std::mem::size_of::<RECT>() as u32,
    );
    ext
}

/// Pindahkan window dengan kompensasi invisible border Windows 10/11.
/// Mengembalikan batas frame yang terlihat SETELAH dipindah (bisa beda dari
/// yang diminta kalau app punya ukuran minimum), dan apakah Windows menolak
/// (ACCESS_DENIED: jendela berjalan dengan hak lebih tinggi, mis. administrator).
pub(crate) unsafe fn place(hwnd: HWND, r: Rect) -> (RECT, bool) {
    if IsZoomed(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let mut win = RECT::default();
    let _ = GetWindowRect(hwnd, &mut win);
    let ext = ext_bounds(hwnd);
    let (l, t) = (ext.left - win.left, ext.top - win.top);
    let (rr, b) = (win.right - ext.right, win.bottom - ext.bottom);
    let result = SetWindowPos(
        hwnd,
        None,
        r.x - l,
        r.y - t,
        r.w + l + rr,
        r.h + t + b,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let denied = result.is_err_and(|e| e.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0));
    (ext_bounds(hwnd), denied)
}

pub(crate) unsafe fn set_border(h: HWND, color: u32) {
    safety::BORDER_TOUCHED.store(true, Ordering::Relaxed);
    let _ = DwmSetWindowAttribute(
        h,
        DWMWA_BORDER_COLOR,
        &color as *const u32 as *const c_void,
        4,
    );
}

pub(crate) unsafe fn title(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetWindowTextW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub(crate) type Bounds = (i32, i32, i32, i32);

pub(crate) fn bounds(r: RECT) -> Bounds {
    (r.left, r.top, r.right, r.bottom)
}

pub(crate) fn hits(area: Rect, x: i32, y: i32, w: i32, h: i32) -> bool {
    x < area.x + area.w && x + w > area.x && y < area.y + area.h && y + h > area.y
}

pub(crate) unsafe fn insets(hwnd: HWND) -> (i32, i32) {
    let mut win = RECT::default();
    let _ = GetWindowRect(hwnd, &mut win);
    let ext = ext_bounds(hwnd);
    (ext.left - win.left, ext.top - win.top)
}

/// Hormati pengaturan Windows "Animation effects".
pub(crate) unsafe fn animations_enabled() -> bool {
    let mut on = BOOL(1);
    let r = SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        Some(&mut on as *mut BOOL as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    r.is_err() || on.as_bool()
}

/// Cegah dua salinan berjalan bersamaan (mis. autostart + dijalankan manual):
/// dua hook keyboard dan dua penata window akan saling berebut.
pub(crate) unsafe fn single_instance() -> bool {
    match CreateMutexW(None, true, w!("Local\\vscroll-wm-single-instance")) {
        Ok(_handle) => GetLastError() != ERROR_ALREADY_EXISTS, // handle sengaja tidak ditutup
        Err(_) => true,
    }
}

unsafe fn token_is_elevated(token: HANDLE) -> bool {
    let mut elev = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    let ok = GetTokenInformation(
        token,
        TokenElevation,
        Some(&mut elev as *mut TOKEN_ELEVATION as *mut c_void),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut len,
    )
    .is_ok();
    ok && elev.TokenIsElevated != 0
}

pub(crate) unsafe fn is_process_elevated() -> bool {
    let mut tok = HANDLE::default();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
        return false;
    }
    let elevated = token_is_elevated(tok);
    let _ = CloseHandle(tok);
    elevated
}

pub(crate) fn is_elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe { is_process_elevated() })
}

/// True if the window's owning process is running with an elevated (administrator) token.
pub(crate) unsafe fn is_hwnd_elevated(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 || pid == GetCurrentProcessId() {
        return false;
    }
    let Ok(proc_h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        // Protected / inaccessible processes cannot be controlled by Medium-integrity ORUMI either.
        return true;
    };
    let mut tok = HANDLE::default();
    let elevated = if OpenProcessToken(proc_h, TOKEN_QUERY, &mut tok).is_ok() {
        let e = token_is_elevated(tok);
        let _ = CloseHandle(tok);
        e
    } else {
        true
    };
    let _ = CloseHandle(proc_h);
    elevated
}

/// String SID of the current process token user (e.g. `S-1-5-21-...`), used to build strict Named Pipe ACLs.
pub(crate) unsafe fn current_user_sid_string() -> Option<String> {
    let mut tok = HANDLE::default();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
        return None;
    }
    let mut needed = 0u32;
    let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut needed);
    if needed == 0 {
        let _ = CloseHandle(tok);
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = GetTokenInformation(
        tok,
        TokenUser,
        Some(buf.as_mut_ptr().cast::<c_void>()),
        needed,
        &mut needed,
    )
    .is_ok();
    let _ = CloseHandle(tok);
    if !ok {
        return None;
    }
    let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
    let mut pwstr = PWSTR::null();
    if ConvertSidToStringSidW(user.User.Sid, &mut pwstr).is_err() || pwstr.0.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *pwstr.0.add(len) != 0 {
        len += 1;
    }
    let sid = String::from_utf16_lossy(std::slice::from_raw_parts(pwstr.0, len));
    let _ = LocalFree(Some(HLOCAL(pwstr.0.cast::<c_void>())));
    Some(sid)
}
