//! OSD workspace: panel kecil di bagian bawah layar yang muncul sebentar setiap pindah
//! workspace ("Workspace 2" + deretan kotak, yang aktif disorot), lalu hilang sendiri.
//!
//! Tidak pernah merebut fokus dan tidak menangkap klik. Warnanya mengikuti warna
//! overview. Dimatikan lewat `workspace_osd = off`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::Mutex;

use layout::Rect;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HGDIOBJ, LOGFONTW, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{rgb, OverviewColors};
use crate::overview::luminance;

/// Berapa lama panel tampil (ms).
const SHOW_MS: u32 = 900;
const TIMER_ID: usize = 1;
const PANEL_H: i32 = 96;
/// Jarak panel dari tepi bawah area kerja (piksel).
const BOTTOM_MARGIN: i32 = 48;
const MIN_PANEL_W: i32 = 240;
const PILL: i32 = 18;
const PILL_GAP: i32 = 12;

static OSD_HWND: AtomicIsize = AtomicIsize::new(0);
static SUPPRESSED: AtomicBool = AtomicBool::new(false);
static FONT: AtomicIsize = AtomicIsize::new(0);
/// Workspace yang terakhir diumumkan; panel hanya muncul kalau berubah.
static LAST: AtomicUsize = AtomicUsize::new(0);
/// (workspace aktif, jumlah workspace, warna)
static STATE: Mutex<(usize, usize, OverviewColors)> = Mutex::new((
    0,
    0,
    OverviewColors {
        selected: 0,
        active: 0,
        view: 0,
        background: 0,
        box_fill: 0,
    },
));

/// Lebar panel untuk `total` workspace.
pub fn panel_width(total: usize) -> i32 {
    let row = total as i32 * (PILL + PILL_GAP);
    (row + 60).max(MIN_PANEL_W)
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    if let Ok(g) = STATE.lock() {
        let (active, total, colors) = *g;
        let bg = CreateSolidBrush(COLORREF(colors.background));
        let on = CreateSolidBrush(COLORREF(colors.active));
        let off = CreateSolidBrush(COLORREF(colors.box_fill));
        FillRect(hdc, &client, bg);
        SetBkMode(hdc, TRANSPARENT);

        let text = if luminance(colors.background) <= 150 {
            rgb(232, 235, 243)
        } else {
            rgb(25, 28, 36)
        };
        SelectObject(hdc, HGDIOBJ(FONT.load(Ordering::Relaxed) as *mut _));
        SetTextColor(hdc, COLORREF(text));
        let mut label: Vec<u16> = format!("Workspace {}", active + 1).encode_utf16().collect();
        let mut r = RECT {
            left: 0,
            top: 12,
            right: client.right,
            bottom: 52,
        };
        DrawTextW(
            hdc,
            &mut label,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let n = total as i32;
        let row_w = n * PILL + (n - 1).max(0) * PILL_GAP;
        let x0 = (client.right - row_w) / 2;
        for i in 0..n {
            let x = x0 + i * (PILL + PILL_GAP);
            let rect = RECT {
                left: x,
                top: 62,
                right: x + PILL,
                bottom: 62 + PILL,
            };
            FillRect(hdc, &rect, if i as usize == active { on } else { off });
        }
        for b in [bg, on, off] {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn osd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT: klik menembus panel
        WM_MOUSEACTIVATE => LRESULT(3), // MA_NOACTIVATE
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Buat jendela panel (tersembunyi) sekali di awal.
pub unsafe fn init() -> bool {
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmOsd");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(osd_proc),
        hInstance: hinst,
        lpszClassName: class,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return false;
    }
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class,
        w!("vscroll-wm workspace"),
        WS_POPUP,
        0,
        0,
        100,
        100,
        None,
        None,
        Some(hinst),
        None,
    ) else {
        return false;
    };
    OSD_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    let round: u32 = 2; // DWMWCP_ROUND (Windows 11)
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &round as *const u32 as *const c_void,
        4,
    );
    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = -28;
    lf.lfWeight = 700;
    for (i, ch) in "Segoe UI".encode_utf16().enumerate() {
        lf.lfFaceName[i] = ch;
    }
    FONT.store(CreateFontIndirectW(&lf).0 as isize, Ordering::Relaxed);
    true
}

pub unsafe fn set_suppressed(on: bool) {
    SUPPRESSED.store(on, Ordering::Relaxed);
    if on {
        hide();
    }
}

/// Umumkan workspace aktif. Tidak melakukan apa-apa kalau workspace-nya sama dengan
/// yang terakhir diumumkan. `total` = jumlah workspace yang ditampilkan sebagai kotak.
pub unsafe fn notify(active: usize, total: usize, colors: OverviewColors, work: Rect) {
    let hw = OSD_HWND.load(Ordering::Relaxed);
    if hw == 0 || SUPPRESSED.load(Ordering::Relaxed) || LAST.swap(active, Ordering::Relaxed) == active {
        return;
    }
    if let Ok(mut g) = STATE.lock() {
        *g = (active, total.max(active + 1), colors);
    }
    let h = HWND(hw as *mut _);
    let w = panel_width(total.max(active + 1));
    let x = work.x + (work.w - w) / 2;
    let y = work.y + work.h - PANEL_H - BOTTOM_MARGIN;
    let _ = SetWindowPos(
        h,
        Some(HWND_TOPMOST),
        x,
        y,
        w,
        PANEL_H,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    let _ = InvalidateRect(Some(h), None, false);
    let _ = SetTimer(Some(h), TIMER_ID, SHOW_MS, None); // timer yang sama = hitungan diulang
}

/// Sembunyikan segera (mis. saat dijeda).
pub unsafe fn hide() {
    let hw = OSD_HWND.load(Ordering::Relaxed);
    if hw != 0 {
        let h = HWND(hw as *mut _);
        let _ = KillTimer(Some(h), TIMER_ID);
        let _ = ShowWindow(h, SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_grows_with_the_workspace_count_but_has_a_minimum() {
        assert_eq!(panel_width(1), MIN_PANEL_W);
        assert!(panel_width(9) > panel_width(5));
        assert!(panel_width(9) >= 9 * (PILL + PILL_GAP));
    }
}
