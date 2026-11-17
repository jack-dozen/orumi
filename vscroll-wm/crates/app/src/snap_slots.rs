//! Daftar window untuk zona snap layout yang masih kosong (Snap Assist): satu panel di
//! atas tiap zona kosong, berisi kartu semua window yang sedang terbuka (thumbnail DWM
//! + judul). Klik kartu = window itu mengisi zona tersebut.
//!
//! Panel tidak pernah merebut fokus (WS_EX_NOACTIVATE): klik tetap sampai ke sini tapi
//! window yang sedang aktif tidak berganti. Esc dan klik di luar ditangani app
//! (hook keyboard / pelacak fokus). Penggambar hanya meneruskan klik sebagai pesan ke
//! thread utama: `WM_SNAP_PICK` (kartu) dan `WM_SNAP_CANCEL` (tombol tutup / klik kanan).
//!
//! Beberapa panel dibuat sekali di awal (satu per zona maksimum) lalu dipakai ulang.

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;

use layout::Rect;
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DRAW_TEXT_FORMAT, HBRUSH, HDC, HGDIOBJ, LOGFONTW,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{rgb, OverviewColors};
use crate::consts::{WM_SNAP_CANCEL, WM_SNAP_PICK};
use crate::overview::luminance;
use crate::snap_menu::mix;

/// Jumlah panel yang disiapkan = zona terbanyak satu preset.
const POOL: usize = layout::MAX_ZONES;
const HEADER_H: i32 = 34;
const PAD: i32 = 12;
const CARD_GAP: i32 = 10;
const LABEL_BAND: i32 = 26;
const MIN_CARD_W: i32 = 70;
const MIN_CARD_H: i32 = 54;

/// Zona kosong yang perlu daftar window: `slot` = id slot di strip, `rect` = tempatnya
/// di layar.
#[derive(Debug, Clone, Copy)]
pub struct SlotView {
    pub slot: u64,
    pub rect: Rect,
}

/// Satu window yang bisa dipilih.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: u64,
    pub label: String,
    /// HWND window aslinya (untuk thumbnail); 0 = tanpa thumbnail.
    pub source: isize,
}

struct Card {
    id: u64,
    label: String,
    rect: RECT,
    /// Pita label di bawah thumbnail; None = tanpa thumbnail (label di tengah kartu).
    band: Option<RECT>,
}

#[derive(Default)]
struct Panel {
    hwnd: isize,
    slot: u64,
    cards: Vec<Card>,
    close: RECT,
    hover: Option<usize>,
    hover_close: bool,
    thumbs: Vec<isize>,
}

static HANDLES: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static PANELS: Mutex<Vec<Panel>> = Mutex::new(Vec::new());
static COLORS: Mutex<OverviewColors> = Mutex::new(OverviewColors {
    selected: 0,
    active: 0,
    view: 0,
    background: 0,
    box_fill: 0,
});
static FONT: AtomicIsize = AtomicIsize::new(0);
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

/// Letak `n` kartu di panel berukuran `w` x `h` (koordinat relatif panel): di bawah
/// header, kolom dan baris dipilih supaya kartunya sebesar mungkin. Kalau tidak semua
/// muat dengan ukuran minimum, hanya yang muat yang diberi tempat (hasilnya bisa
/// kurang dari `n`).
pub(crate) fn card_grid(n: usize, w: i32, h: i32) -> Vec<Rect> {
    let aw = w - 2 * PAD;
    let ah = h - HEADER_H - PAD;
    if n == 0 || aw < 1 || ah < 1 {
        return Vec::new();
    }
    for count in (1..=n as i32).rev() {
        let mut best: Option<(i32, i32, i32, i32)> = None; // (nilai, kolom, lebar, tinggi)
        for cols in 1..=count {
            let rows = (count + cols - 1) / cols;
            let cw = (aw - (cols - 1) * CARD_GAP) / cols;
            let ch = (ah - (rows - 1) * CARD_GAP) / rows;
            if cw < MIN_CARD_W || ch < MIN_CARD_H {
                continue;
            }
            // thumbnail memakai rasio kira-kira 4:3: nilai = sisi terpendeknya
            let score = cw.min(ch * 4 / 3);
            if best.map_or(true, |b| score > b.0) {
                best = Some((score, cols, cw, ch));
            }
        }
        if let Some((_, cols, cw, ch)) = best {
            return (0..count)
                .map(|k| {
                    let (c, r) = (k % cols, k / cols);
                    Rect {
                        x: PAD + c * (cw + CARD_GAP),
                        y: HEADER_H + r * (ch + CARD_GAP),
                        w: cw,
                        h: ch,
                    }
                })
                .collect();
        }
    }
    // panel terlalu kecil untuk kartu biasa: satu kartu memenuhi ruangnya
    vec![Rect {
        x: PAD,
        y: HEADER_H,
        w: aw,
        h: ah,
    }]
}

fn point_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

/// Muat sumber berukuran (sw x sh) ke `area` tanpa mengubah rasio, di tengah.
fn fit(area: RECT, sw: i32, sh: i32) -> RECT {
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    if sw <= 0 || sh <= 0 || aw <= 0 || ah <= 0 {
        return area;
    }
    let scale = (aw as f32 / sw as f32).min(ah as f32 / sh as f32);
    let w = ((sw as f32 * scale) as i32).max(1);
    let h = ((sh as f32 * scale) as i32).max(1);
    let l = area.left + (aw - w) / 2;
    let t = area.top + (ah - h) / 2;
    RECT {
        left: l,
        top: t,
        right: l + w,
        bottom: t + h,
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

unsafe fn text(hdc: HDC, s: &str, r: &RECT, flags: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    let mut rr = *r;
    DrawTextW(hdc, &mut wide, &mut rr, flags);
}

/// Pasang thumbnail DWM `source` di kartu `card`. Mengembalikan (handle thumbnail,
/// pita label) kalau kartunya cukup besar.
unsafe fn register_thumb(dest: HWND, source: isize, card: &RECT) -> Option<(isize, RECT)> {
    let inner = RECT {
        left: card.left + 3,
        top: card.top + 3,
        right: card.right - 3,
        bottom: card.bottom - 3,
    };
    let area = RECT {
        left: inner.left + 4,
        top: inner.top + 4,
        right: inner.right - 4,
        bottom: inner.bottom - LABEL_BAND,
    };
    if area.right - area.left < 40 || area.bottom - area.top < 30 {
        return None; // terlalu kecil: cukup label teks
    }
    let thumb = DwmRegisterThumbnail(dest, HWND(source as *mut _)).ok()?;
    let size = DwmQueryThumbnailSourceSize(thumb).unwrap_or(SIZE { cx: 0, cy: 0 });
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
        rcDestination: fit(area, size.cx, size.cy),
        rcSource: RECT::default(),
        opacity: 255,
        fVisible: BOOL(1),
        fSourceClientAreaOnly: BOOL(0),
    };
    if DwmUpdateThumbnailProperties(thumb, &props).is_err() {
        let _ = DwmUnregisterThumbnail(thumb);
        return None;
    }
    Some((
        thumb,
        RECT {
            left: inner.left,
            top: inner.bottom - LABEL_BAND,
            right: inner.right,
            bottom: inner.bottom,
        },
    ))
}

unsafe fn post(msg: u32, wparam: usize, lparam: isize) {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        msg,
        WPARAM(wparam),
        LPARAM(lparam),
    );
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let colors = COLORS.lock().map(|c| *c).unwrap_or_default();

    if let Ok(g) = PANELS.lock() {
        if let Some(p) = g.iter().find(|p| p.hwnd == hwnd.0 as isize) {
            let fg = if luminance(colors.background) <= 150 {
                rgb(232, 235, 243)
            } else {
                rgb(25, 28, 36)
            };
            let bg = CreateSolidBrush(COLORREF(colors.background));
            let card = CreateSolidBrush(COLORREF(colors.box_fill));
            let card_hot = CreateSolidBrush(COLORREF(mix(colors.box_fill, colors.active, 40)));
            let accent = CreateSolidBrush(COLORREF(colors.active));
            let sel = CreateSolidBrush(COLORREF(colors.selected));
            FillRect(hdc, &client, bg);
            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, HGDIOBJ(FONT.load(Ordering::Relaxed) as *mut _));
            SetTextColor(hdc, COLORREF(fg));

            // header: petunjuk + tombol tutup
            let head = RECT {
                left: PAD,
                top: 0,
                right: p.close.left,
                bottom: HEADER_H,
            };
            text(
                hdc,
                "Choose a window for this zone  (Esc cancels)",
                &head,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if p.hover_close {
                FillRect(hdc, &p.close, accent);
            }
            text(
                hdc,
                "\u{00D7}",
                &p.close,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            for (i, c) in p.cards.iter().enumerate() {
                let hot = p.hover == Some(i);
                FillRect(hdc, &c.rect, if hot { card_hot } else { card });
                if hot {
                    frame(hdc, &c.rect, sel, 2);
                }
                match c.band {
                    Some(band) => {
                        let r = RECT {
                            left: band.left + 6,
                            top: band.top,
                            right: band.right - 6,
                            bottom: band.bottom,
                        };
                        text(
                            hdc,
                            &c.label,
                            &r,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                    }
                    None => {
                        let r = RECT {
                            left: c.rect.left + 8,
                            top: c.rect.top,
                            right: c.rect.right - 8,
                            bottom: c.rect.bottom,
                        };
                        text(
                            hdc,
                            &c.label,
                            &r,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                    }
                }
            }
            // tepi panel: menandai zona yang akan diisi
            frame(hdc, &client, accent, 3);
            for b in [bg, card, card_hot, accent, sel] {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

/// Perbarui sorotan di bawah kursor; gambar ulang kalau berubah.
unsafe fn hover_at(hwnd: HWND, x: i32, y: i32) {
    let mut changed = false;
    if let Ok(mut g) = PANELS.lock() {
        if let Some(p) = g.iter_mut().find(|p| p.hwnd == hwnd.0 as isize) {
            let hover = p.cards.iter().position(|c| point_in(&c.rect, x, y));
            let hover_close = point_in(&p.close, x, y);
            if hover != p.hover || hover_close != p.hover_close {
                p.hover = hover;
                p.hover_close = hover_close;
                changed = true;
            }
        }
    }
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe fn click_at(hwnd: HWND, x: i32, y: i32) {
    // (id window yang dipilih, id slot) atau tutup
    let mut action: Option<(Option<u64>, u64)> = None;
    if let Ok(g) = PANELS.lock() {
        if let Some(p) = g.iter().find(|p| p.hwnd == hwnd.0 as isize) {
            if point_in(&p.close, x, y) {
                action = Some((None, p.slot));
            } else if let Some(c) = p.cards.iter().find(|c| point_in(&c.rect, x, y)) {
                action = Some((Some(c.id), p.slot));
            }
        }
    }
    match action {
        Some((Some(id), slot)) => post(WM_SNAP_PICK, id as usize, slot as isize),
        Some((None, _)) => post(WM_SNAP_CANCEL, 0, 0),
        None => {}
    }
}

unsafe extern "system" fn slot_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(3), // MA_NOACTIVATE: klik tidak mengubah window aktif
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lparam);
            hover_at(hwnd, x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lparam);
            click_at(hwnd, x, y);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            post(WM_SNAP_CANCEL, 0, 0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Buat panel-panel (tersembunyi) sekali di awal. `scale` = dpi / 96 untuk huruf.
pub unsafe fn init(main_thread: u32, scale: f32) -> bool {
    MAIN_THREAD.store(main_thread, Ordering::Relaxed);
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmSnapSlot");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(slot_proc),
        hInstance: hinst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return false;
    }
    let mut handles = Vec::new();
    for _ in 0..POOL {
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("vscroll-wm snap assist"),
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
        handles.push(hwnd.0 as isize);
    }
    if let Ok(mut g) = HANDLES.lock() {
        *g = handles;
    }
    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = -((16.0 * scale).round() as i32);
    lf.lfWeight = 400;
    for (i, ch) in "Segoe UI".encode_utf16().enumerate() {
        lf.lfFaceName[i] = ch;
    }
    FONT.store(CreateFontIndirectW(&lf).0 as isize, Ordering::Relaxed);
    true
}

/// True kalau `h` salah satu panel milik modul ini.
pub fn is_ours(h: HWND) -> bool {
    HANDLES
        .lock()
        .map_or(false, |g| g.contains(&(h.0 as isize)))
}

/// Tampilkan satu panel di atas tiap zona kosong `slots`, masing-masing berisi `cands`.
/// Memanggil ulang menggantikan tampilan sebelumnya. `thumbnails` = tampilan mini
/// langsung dari window aslinya (kalau tidak, hanya judul).
pub unsafe fn show(
    slots: &[SlotView],
    cands: &[Candidate],
    colors: OverviewColors,
    thumbnails: bool,
) {
    hide();
    let handles: Vec<isize> = HANDLES.lock().map(|g| g.clone()).unwrap_or_default();
    if let Ok(mut c) = COLORS.lock() {
        *c = colors;
    }
    let mut panels: Vec<Panel> = Vec::new();
    for (view, &hw) in slots.iter().zip(handles.iter()) {
        let r = view.rect;
        let (w, h) = (r.w.max(1), r.h.max(1));
        let dest = HWND(hw as *mut _);
        let mut cards = Vec::new();
        let mut thumbs = Vec::new();
        for (cand, cr) in cands.iter().zip(card_grid(cands.len(), w, h)) {
            let rect = RECT {
                left: cr.x,
                top: cr.y,
                right: cr.x + cr.w,
                bottom: cr.y + cr.h,
            };
            let mut band = None;
            if thumbnails && cand.source != 0 {
                if let Some((t, b)) = register_thumb(dest, cand.source, &rect) {
                    thumbs.push(t);
                    band = Some(b);
                }
            }
            cards.push(Card {
                id: cand.id,
                label: cand.label.clone(),
                rect,
                band,
            });
        }
        panels.push(Panel {
            hwnd: hw,
            slot: view.slot,
            cards,
            close: RECT {
                left: w - HEADER_H,
                top: 0,
                right: w,
                bottom: HEADER_H,
            },
            hover: None,
            hover_close: false,
            thumbs,
        });
    }
    // sebelum ditampilkan, supaya WM_PAINT pertama sudah menemukan datanya
    let shown: Vec<(isize, Rect)> = panels
        .iter()
        .zip(slots.iter())
        .map(|(p, v)| (p.hwnd, v.rect))
        .collect();
    if let Ok(mut g) = PANELS.lock() {
        *g = panels;
    }
    for (hw, r) in shown {
        let h = HWND(hw as *mut _);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            r.x,
            r.y,
            r.w.max(1),
            r.h.max(1),
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(Some(h), None, false);
    }
}

/// Sembunyikan semua panel dan lepaskan thumbnail-nya.
pub unsafe fn hide() {
    let old: Vec<Panel> = PANELS
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default();
    for p in &old {
        for t in &p.thumbs {
            let _ = DwmUnregisterThumbnail(*t);
        }
        let _ = ShowWindow(HWND(p.hwnd as *mut _), SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(outer: (i32, i32), r: &Rect) -> bool {
        r.x >= 0 && r.y >= HEADER_H && r.x + r.w <= outer.0 && r.y + r.h <= outer.1
    }

    fn overlap(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn cards_fit_inside_the_panel_below_the_header_without_overlapping() {
        for n in 1..=12usize {
            let cards = card_grid(n, 900, 700);
            assert_eq!(cards.len(), n, "n = {n}");
            for (i, c) in cards.iter().enumerate() {
                assert!(inside((900, 700), c), "n = {n}: {c:?}");
                assert!(c.w >= MIN_CARD_W && c.h >= MIN_CARD_H);
                for d in &cards[i + 1..] {
                    assert!(!overlap(c, d), "n = {n}: {c:?} vs {d:?}");
                }
            }
        }
    }

    #[test]
    fn a_wide_panel_prefers_columns_and_a_tall_panel_prefers_rows() {
        let wide = card_grid(2, 1200, 300);
        assert_eq!(wide[0].y, wide[1].y); // sebaris
        let tall = card_grid(2, 300, 1000);
        assert_eq!(tall[0].x, tall[1].x); // sekolom
    }

    #[test]
    fn too_many_windows_for_a_small_panel_are_truncated_not_squeezed() {
        let cards = card_grid(30, 400, 300);
        assert!(!cards.is_empty() && cards.len() < 30);
        assert!(cards.iter().all(|c| c.w >= MIN_CARD_W && c.h >= MIN_CARD_H));
    }

    #[test]
    fn a_tiny_panel_still_gets_one_card() {
        let cards = card_grid(3, 90, 70);
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn nothing_to_show_gives_no_cards() {
        assert!(card_grid(0, 900, 700).is_empty());
    }
}
