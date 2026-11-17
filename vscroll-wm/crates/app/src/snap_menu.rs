//! Menu snap layouts: panel preset di tengah tepi atas layar yang muncul selama window
//! diseret ke atas (seperti Snap Layouts di Windows 11). Tiap preset digambar sebagai
//! kotak kecil berisi zonanya; zona di bawah kursor disorot. Window yang dilepas di
//! sebuah zona mengisi zona itu (lihat app/snap.rs).
//!
//! Selama seretan, kursor ditangkap oleh window yang sedang diseret, jadi panel ini
//! tidak menerima mouse sama sekali: letak kursor dibaca oleh app dan dicocokkan ke
//! `MenuLayout` lewat `hit` / `inside`. Panel tidak pernah merebut fokus. Geometri
//! (`layout_menu`, `hit`, `inside`) murni hitungan dan dipakai bersama oleh penggambar
//! dan pencocok klik, jadi yang tampak selalu sama dengan yang dikenai.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use layout::{Rect, SnapPreset};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject,
    DrawTextW, EndPaint, FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, UpdateWindow, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HGDIOBJ, LOGFONTW, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{rgb, OverviewColors};
use crate::overview::luminance;

/// Ukuran dasar (piksel pada 96 dpi); dikalikan skala dpi layar.
const THUMB_W: i32 = 128;
const THUMB_H: i32 = 80;
const LABEL_H: i32 = 24;
const PAD: i32 = 14;
const GAP: i32 = 12;
/// Jarak panel dari tepi atas area kerja (0 = menempel langsung ke tepi atas seperti panel edge ORUMI).
const TOP_MARGIN: i32 = 0;
/// Jarak isi kotak preset dari tepinya.
const INSET: i32 = 6;
/// Jarak antar zona di dalam satu kotak preset.
const ZONE_GAP: i32 = 4;

/// Satu zona di menu: `cell` = daerah yang dikenai kursor, `draw` = yang digambar
/// (sedikit lebih kecil supaya batas antar zona terlihat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneBox {
    pub cell: Rect,
    pub draw: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    /// Kotak preset (koordinat layar).
    pub rect: Rect,
    /// Tempat nama preset.
    pub label: Rect,
    pub zones: Vec<ZoneBox>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLayout {
    /// Panel seluruhnya (koordinat layar).
    pub panel: Rect,
    pub thumbs: Vec<Thumb>,
}

fn contains(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// Susun panel untuk `presets` di tengah tepi atas `work`. `scale` = dpi / 96.
pub fn layout_menu(work: Rect, presets: &[SnapPreset], scale: f32) -> MenuLayout {
    let s = |v: i32| ((v as f32) * scale).round() as i32;
    let (tw, th, lh, pad, gap) = (s(THUMB_W), s(THUMB_H), s(LABEL_H), s(PAD), s(GAP));
    let (inset, zgap) = (s(INSET), s(ZONE_GAP));
    let n = presets.len() as i32;
    if n == 0 {
        return MenuLayout {
            panel: Rect {
                x: work.x,
                y: work.y,
                w: 0,
                h: 0,
            },
            thumbs: Vec::new(),
        };
    }
    // sebanyak mungkin kotak per baris dalam 90% lebar layar
    let max_w = work.w * 9 / 10;
    let cols = ((max_w - 2 * pad + gap) / (tw + gap)).clamp(1, n);
    let rows = (n + cols - 1) / cols;
    let panel_w = cols * tw + (cols - 1) * gap + 2 * pad;
    let panel_h = rows * (th + lh) + (rows - 1) * gap + 2 * pad;
    let panel = Rect {
        x: work.x + (work.w - panel_w) / 2,
        y: work.y + s(TOP_MARGIN),
        w: panel_w,
        h: panel_h,
    };
    let half = zgap / 2;
    let thumbs = presets
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            let x = panel.x + pad + col * (tw + gap);
            let y = panel.y + pad + row * (th + lh + gap);
            let rect = Rect { x, y, w: tw, h: th };
            let label = Rect {
                x,
                y: y + th,
                w: tw,
                h: lh,
            };
            let zones = p
                .preview((tw - 2 * inset).max(1), (th - 2 * inset).max(1))
                .into_iter()
                .map(|r| {
                    let cell = Rect {
                        x: x + inset + r.x,
                        y: y + inset + r.y,
                        w: r.w,
                        h: r.h,
                    };
                    let draw = Rect {
                        x: cell.x + half,
                        y: cell.y + half,
                        w: (cell.w - 2 * half).max(1),
                        h: (cell.h - 2 * half).max(1),
                    };
                    ZoneBox { cell, draw }
                })
                .collect();
            Thumb { rect, label, zones }
        })
        .collect();
    MenuLayout { panel, thumbs }
}

/// Zona di titik (x, y): (nomor preset, nomor zona).
pub fn hit(l: &MenuLayout, x: i32, y: i32) -> Option<(usize, usize)> {
    if !contains(&l.panel, x, y) {
        return None;
    }
    for (pi, t) in l.thumbs.iter().enumerate() {
        for (zi, z) in t.zones.iter().enumerate() {
            if contains(&z.cell, x, y) {
                return Some((pi, zi));
            }
        }
    }
    None
}

/// Titik (x, y) di dalam panel yang diperlebar `margin` piksel?
pub fn inside(l: &MenuLayout, x: i32, y: i32, margin: i32) -> bool {
    let p = l.panel;
    x >= p.x - margin && x < p.x + p.w + margin && y >= p.y - margin && y < p.y + p.h + margin
}

struct State {
    layout: MenuLayout,
    names: Vec<String>,
    hover: Option<(usize, usize)>,
    colors: OverviewColors,
    start_y: i32,
    anim_start: Option<Instant>,
    anim_ms: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static MENU_HWND: AtomicIsize = AtomicIsize::new(0);
static FONT: AtomicIsize = AtomicIsize::new(0);
static SCALE_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f32

fn channels(c: u32) -> (u32, u32, u32) {
    (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
}

/// Campur warna `c` ke arah `toward` sebesar `pct` persen.
pub(crate) fn mix(c: u32, toward: u32, pct: u32) -> u32 {
    let (r, g, b) = channels(c);
    let (tr, tg, tb) = channels(toward);
    let f = |a: u32, t: u32| (a * (100 - pct) + t * pct) / 100;
    rgb(f(r, tr), f(g, tg), f(b, tb))
}

fn local(r: Rect, origin: Rect) -> RECT {
    RECT {
        left: r.x - origin.x,
        top: r.y - origin.y,
        right: r.x - origin.x + r.w,
        bottom: r.y - origin.y + r.h,
    }
}

/// Bingkai persegi (garis tebal `t`) di sekeliling `r`.
unsafe fn frame(hdc: HDC, r: &RECT, brush: HBRUSH, t: i32) {
    for e in [
        RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + t },
        RECT { left: r.left, top: r.bottom - t, right: r.right, bottom: r.bottom },
        RECT { left: r.left, top: r.top, right: r.left + t, bottom: r.bottom },
        RECT { left: r.right - t, top: r.top, right: r.right, bottom: r.bottom },
    ] {
        FillRect(hdc, &e, brush);
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    if let Ok(g) = STATE.lock() {
        if let Some(st) = g.as_ref() {
            let c = st.colors;
            let text = if luminance(c.background) <= 150 {
                rgb(232, 235, 243)
            } else {
                rgb(25, 28, 36)
            };
            let bg = CreateSolidBrush(COLORREF(c.background));
            let boxb = CreateSolidBrush(COLORREF(c.box_fill));
            let zone = CreateSolidBrush(COLORREF(mix(c.box_fill, text, 30)));
            let hot = CreateSolidBrush(COLORREF(c.active));
            let sel = CreateSolidBrush(COLORREF(c.selected));
            FillRect(hdc, &client, bg);
            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, HGDIOBJ(FONT.load(Ordering::Relaxed) as *mut _));
            SetTextColor(hdc, COLORREF(text));

            let origin = st.layout.panel;
            for (pi, t) in st.layout.thumbs.iter().enumerate() {
                let r = local(t.rect, origin);
                FillRect(hdc, &r, boxb);
                if st.hover.map_or(false, |(hp, _)| hp == pi) {
                    frame(hdc, &r, sel, 2);
                }
                for (zi, z) in t.zones.iter().enumerate() {
                    let zr = local(z.draw, origin);
                    let on = st.hover == Some((pi, zi));
                    FillRect(hdc, &zr, if on { hot } else { zone });
                }
                let mut label: Vec<u16> = st
                    .names
                    .get(pi)
                    .map_or("", |n| n.as_str())
                    .encode_utf16()
                    .collect();
                let mut lr = local(t.label, origin);
                DrawTextW(
                    hdc,
                    &mut label,
                    &mut lr,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
            for b in [bg, boxb, zone, hot, sel] {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT: klik menembus panel
        WM_MOUSEACTIVATE => LRESULT(3), // MA_NOACTIVATE
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Buat jendela panel (tersembunyi) sekali di awal. `scale` = dpi / 96 untuk huruf.
pub unsafe fn init(scale: f32) -> bool {
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmSnapMenu");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(menu_proc),
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
        w!("vscroll-wm snap layouts"),
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
    MENU_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);

    // Flat top edge (flush against top screen edge); bottom corners are rounded via SetWindowRgn.
    let no_round: u32 = 1; // DWMWCP_DONOTROUND
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &no_round as *const u32 as *const c_void,
        4,
    );
    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = -((15.0 * scale).round() as i32);
    lf.lfWeight = 400;
    for (i, ch) in "Segoe UI".encode_utf16().enumerate() {
        lf.lfFaceName[i] = ch;
    }
    FONT.store(CreateFontIndirectW(&lf).0 as isize, Ordering::Relaxed);
    true
}

fn hwnd() -> HWND {
    HWND(MENU_HWND.load(Ordering::Relaxed) as *mut _)
}

/// Tampilkan panel `layout` dengan nama-nama preset `names`, meluncur turun dari atas layar.
pub unsafe fn show(
    layout: MenuLayout,
    names: Vec<String>,
    colors: OverviewColors,
    anim_ms: u32,
) {
    if MENU_HWND.load(Ordering::Relaxed) == 0 {
        return;
    }
    let p = layout.panel;
    let start_y = p.y - p.h;
    let anim_ms = anim_ms.max(1);
    if let Ok(mut g) = STATE.lock() {
        *g = Some(State {
            layout,
            names,
            hover: None,
            colors,
            start_y,
            anim_start: Some(Instant::now()),
            anim_ms,
        });
    }
    let h = hwnd();
    let scale = f32::from_bits(SCALE_BITS.load(Ordering::Relaxed)).max(0.5);
    let r = (18.0 * scale).round() as i32;
    let rgn = CreateRoundRectRgn(0, -r, p.w + 1, p.h + 1, r * 2, r * 2);
    if !rgn.is_invalid() && SetWindowRgn(h, Some(rgn), false) == 0 {
        let _ = DeleteObject(HGDIOBJ(rgn.0));
    }
    let _ = SetWindowPos(
        h,
        Some(HWND_TOPMOST),
        p.x,
        start_y,
        p.w,
        p.h,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    let _ = InvalidateRect(Some(h), None, false);
    let _ = UpdateWindow(h);
}

/// Lanjutkan animasi slide turun menu snap jika masih berjalan.
pub unsafe fn tick_anim() {
    if MENU_HWND.load(Ordering::Relaxed) == 0 {
        return;
    }
    let step = {
        let Ok(mut g) = STATE.lock() else {
            return;
        };
        let Some(st) = g.as_mut() else {
            return;
        };
        let Some(t0) = st.anim_start else {
            return;
        };
        let u = (t0.elapsed().as_secs_f32() * 1000.0 / st.anim_ms as f32).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - u).powi(3);
        let p = st.layout.panel;
        let y = if u >= 1.0 {
            st.anim_start = None;
            p.y
        } else {
            st.start_y + ((p.y - st.start_y) as f32 * ease).round() as i32
        };
        Some((p.x, y, p.w, p.h))
    };
    if let Some((x, y, w, h_px)) = step {
        let _ = SetWindowPos(
            hwnd(),
            Some(HWND_TOPMOST),
            x,
            y,
            w,
            h_px,
            SWP_NOACTIVATE | SWP_NOSIZE,
        );
    }
}

/// Sorot zona (nomor preset, nomor zona) di bawah kursor; None = tidak ada.
pub unsafe fn set_hover(hover: Option<(usize, usize)>) {
    if let Ok(mut g) = STATE.lock() {
        if let Some(st) = g.as_mut() {
            if st.hover == hover {
                return;
            }
            st.hover = hover;
        }
    }
    if MENU_HWND.load(Ordering::Relaxed) != 0 {
        let _ = InvalidateRect(Some(hwnd()), None, false);
    }
}

pub unsafe fn hide() {
    if MENU_HWND.load(Ordering::Relaxed) != 0 {
        let _ = ShowWindow(hwnd(), SW_HIDE);
    }
    if let Ok(mut g) = STATE.lock() {
        *g = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1040,
        }
    }

    fn presets() -> Vec<SnapPreset> {
        layout::builtin_presets()
    }

    #[test]
    fn the_panel_is_centered_at_the_top_and_inside_the_screen() {
        let l = layout_menu(work(), &presets(), 1.0);
        let p = l.panel;
        assert_eq!(p.y, TOP_MARGIN);
        assert_eq!(p.x * 2 + p.w, 1920); // simetris
        assert!(p.x >= 0 && p.x + p.w <= 1920);
        assert_eq!(l.thumbs.len(), presets().len());
    }

    #[test]
    fn many_presets_wrap_into_rows_on_a_narrow_screen() {
        let narrow = Rect {
            x: 0,
            y: 0,
            w: 500,
            h: 800,
        };
        let l = layout_menu(narrow, &presets(), 1.0);
        assert!(l.panel.w <= 500);
        let rows: std::collections::BTreeSet<i32> = l.thumbs.iter().map(|t| t.rect.y).collect();
        assert!(rows.len() > 1);
        // tidak ada kotak yang keluar dari panel
        for t in &l.thumbs {
            assert!(t.rect.x >= l.panel.x && t.rect.x + t.rect.w <= l.panel.x + l.panel.w);
            assert!(t.label.y + t.label.h <= l.panel.y + l.panel.h);
        }
    }

    #[test]
    fn dpi_scale_grows_the_panel() {
        let a = layout_menu(work(), &presets(), 1.0).panel;
        let b = layout_menu(work(), &presets(), 2.0).panel;
        assert!(b.h > a.h);
    }

    #[test]
    fn hit_finds_the_zone_under_the_cursor() {
        let ps = presets();
        let l = layout_menu(work(), &ps, 1.0);
        for (pi, t) in l.thumbs.iter().enumerate() {
            for (zi, z) in t.zones.iter().enumerate() {
                let (cx, cy) = (z.draw.x + z.draw.w / 2, z.draw.y + z.draw.h / 2);
                assert_eq!(hit(&l, cx, cy), Some((pi, zi)), "{} zona {zi}", ps[pi].name);
            }
        }
        // di luar panel
        assert_eq!(hit(&l, 0, 1000), None);
        assert_eq!(hit(&l, l.panel.x - 5, l.panel.y + 5), None);
    }

    #[test]
    fn halves_has_two_side_by_side_zones() {
        let ps = vec![SnapPreset::parse("halves", "L, R").unwrap()];
        let l = layout_menu(work(), &ps, 1.0);
        let z = &l.thumbs[0].zones;
        assert_eq!(z.len(), 2);
        assert!(z[0].cell.x + z[0].cell.w <= z[1].cell.x + 1);
        assert_eq!(z[0].cell.y, z[1].cell.y);
    }

    #[test]
    fn inside_includes_the_margin() {
        let l = layout_menu(work(), &presets(), 1.0);
        let p = l.panel;
        assert!(inside(&l, p.x + 1, p.y + 1, 0));
        assert!(!inside(&l, p.x - 3, p.y + 1, 0));
        assert!(inside(&l, p.x - 3, p.y + 1, 10));
    }

    #[test]
    fn no_presets_no_panel() {
        let l = layout_menu(work(), &[], 1.0);
        assert!(l.thumbs.is_empty());
        assert_eq!(hit(&l, 10, 10), None);
    }
}
