//! Overview: peta seluruh strip dalam panel ringkas di tengah layar, seperti
//! Alt+Tab (Win+Tab / Win+Alt+Space).
//!
//! Jendela overlay sendiri. Data layout dan logika pilihan ada di thread utama;
//! di sini hanya menggambar dan meneruskan input (panah / Enter / Esc / klik)
//! sebagai pesan ke thread utama. Tiap kotak bisa menampilkan thumbnail langsung
//! dari window aslinya lewat DWM.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;

use layout::{Overview, Rect, Span};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmSetWindowAttribute,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HGDIOBJ, LOGFONTW, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{rgb, OverviewColors};

/// wParam = 0 kiri, 1 kanan, 2 atas, 3 bawah.
pub const WM_OV_MOVE: u32 = WM_APP + 7;
pub const WM_OV_ACCEPT: u32 = WM_APP + 8;
pub const WM_OV_CANCEL: u32 = WM_APP + 9;
/// wParam = id window yang diklik.
pub const WM_OV_PICK: u32 = WM_APP + 10;
/// wParam = 1 window berikutnya, 0 sebelumnya (urutan baca).
pub const WM_OV_CYCLE: u32 = WM_APP + 11;
/// wParam = 0 workspace sebelumnya, 1 berikutnya, n >= 2 workspace nomor n - 2 (mulai 0).
pub const WM_OV_WS: u32 = WM_APP + 13;
/// Window diseret dan dilepas di overview; detailnya diambil lewat `take_drop()`.
pub const WM_OV_DROP: u32 = WM_APP + 14;

/// Pengaturan tampilan panel.
pub struct Options {
    /// Tampilkan petunjuk tombol di bagian bawah.
    pub hint: bool,
    /// Ukuran maksimum panel, persen dari area kerja (30-100).
    pub size_pct: i32,
    pub colors: OverviewColors,
}

pub struct DrawItem {
    pub id: u64,
    pub rect: RECT,
    pub title: String,
    /// HWND window asli (0 = tanpa thumbnail).
    pub source: isize,
    /// Kalau ada thumbnail: pita di bawah kotak tempat label digambar.
    pub label_band: Option<RECT>,
    /// Lebar window di strip: penuh, atau lajur kiri / kanan (untuk titik drop).
    pub span: Span,
}

/// Satu workspace di panel.
pub struct DrawColumn {
    /// Nomor untuk label (mulai 1).
    pub number: usize,
    pub rect: RECT,
    /// Workspace yang sedang aktif.
    pub active: bool,
    /// Bingkai bagian strip yang sedang tampil di layar untuk workspace ini.
    pub view: RECT,
}

pub struct Draw {
    /// Posisi dan ukuran panel di layar (terpusat di area kerja).
    pub panel: Rect,
    /// Semua koordinat di bawah relatif terhadap sudut kiri atas panel.
    pub items: Vec<DrawItem>,
    /// Satu kolom per workspace yang terlihat, dari kiri ke kanan.
    pub columns: Vec<DrawColumn>,
    /// Lebih dari satu workspace: kolom diberi nomor dan petunjuk menyebut workspace.
    pub multi: bool,
    pub selected: u64,
    /// Window yang sedang aktif sekarang.
    pub focused: u64,
    pub hint: bool,
    pub colors: OverviewColors,
}

static DRAW: Mutex<Option<Draw>> = Mutex::new(None);

/// Drag window di overview (seperti Task View): tombol kiri ditekan di sebuah kotak lalu
/// digeser melewati ambang.
struct Drag {
    /// Window yang diseret.
    id: u64,
    start: (i32, i32),
    /// Sudah melewati ambang gerak (bukan sekadar klik).
    active: bool,
    /// Indeks kolom workspace di bawah kursor.
    over: Option<usize>,
    /// Titik tujuan di bawah kursor (hanya untuk panah penanda; dihitung ulang saat dilepas).
    slot: Option<DropSlot>,
}
static DRAG: Mutex<Option<Drag>> = Mutex::new(None);
/// Hasil drop yang menunggu diambil thread utama: (window yang diseret, tujuannya).
type DropInfo = (u64, DropSlot);
static DROP: Mutex<Option<DropInfo>> = Mutex::new(None);
/// Gerak minimum (piksel) sebelum klik dianggap drag.
const DRAG_START: i32 = 6;

/// Ambil hasil drop terakhir (sekali).
pub fn take_drop() -> Option<DropInfo> {
    DROP.lock().ok().and_then(|mut g| g.take())
}

fn point_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Isi panel di titik (x, y): (indeks kolom, nomor workspace mulai 1, window selain `skip`).
fn hit_at(x: i32, y: i32, skip: u64) -> (Option<usize>, Option<usize>, Option<u64>) {
    let Ok(g) = DRAW.lock() else {
        return (None, None, None);
    };
    let Some(d) = g.as_ref() else {
        return (None, None, None);
    };
    let col = d.columns.iter().position(|c| point_in(&c.rect, x, y));
    let number = col.map(|i| d.columns[i].number);
    let item = d
        .items
        .iter()
        .find(|it| it.id != skip && point_in(&it.rect, x, y))
        .map(|it| it.id);
    (col, number, item)
}

/// Di mana window yang dijatuhkan akan mendarat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    /// Setengah lebar, di lajur seberang window ini (sejajar).
    Beside(u64),
    /// Penuh lebar di atas blok window ini.
    Before(u64),
    /// Penuh lebar di bawah blok window ini.
    After(u64),
    /// Membagi dua window ini; `bool` = window baru di kiri / atas. Penuh lebar:
    /// berdampingan; setengah lebar: bertumpuk di lajurnya, masing-masing setengah tinggi.
    Split(u64, bool),
}

/// Titik tujuan drop: nomor workspace (mulai 1), jenisnya, dan area penandanya di panel.
#[derive(Debug, Clone, Copy)]
pub struct DropSlot {
    pub number: usize,
    pub kind: SlotKind,
    pub mark: RECT,
}

/// Tebal garis penanda "penuh lebar".
const BAR: i32 = 6;
/// Zona tepi atas / bawah strip (persen tinggi window di tepi itu, minimal piksel):
/// menjatuhkan di sana = penuh lebar di luar blok.
const EDGE_PCT: i32 = 22;
const EDGE_MIN: i32 = 10;
/// Kolom dianggap mencakup sedikit ruang di atas (label) dan di bawahnya.
const EXPAND_TOP: i32 = 24;
const EXPAND_BOTTOM: i32 = 16;

fn same_slot(a: &Option<DropSlot>, b: &Option<DropSlot>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.number == b.number && a.kind == b.kind,
        _ => false,
    }
}

/// Area di lajur seberang `x` (sejajar dengannya) di dalam kolom `col`.
fn beside_rect(x: &DrawItem, col: &RECT) -> RECT {
    let (l, r) = if x.span == Span::Right {
        (col.left, x.rect.left - MARGIN)
    } else {
        (x.rect.right + MARGIN, col.right)
    };
    RECT {
        left: l,
        top: x.rect.top,
        right: r.max(l + 8),
        bottom: x.rect.bottom,
    }
}

/// Titik tujuan untuk window `src` yang dijatuhkan di (x, y) panel. Murni (hanya membaca
/// `d`), supaya bisa diuji. Berlaku sama untuk workspace asal maupun workspace lain
/// (di atas window yang diseret sendiri: tidak ada tujuan):
///  - di atas sebuah window: membaginya dua. Penuh lebar -> berdampingan (sisi
///    tempat kursor); setengah lebar -> bertumpuk di lajurnya (separuh tempat kursor)
///  - di ruang kosong di sebelah window setengah lebar: mengisi lajur seberang
///  - di zona tepi atas / bawah kolom: penuh lebar di luar blok
///  - di window penuh lebar yang diapit window setengah lebar (dibagi dua akan menyatukan
///    blok-blok di sekitarnya): penuh lebar di atas / bawah window itu
pub fn slot_at(d: &Draw, x: i32, y: i32, src: u64) -> Option<DropSlot> {
    let col = d.columns.iter().find(|c| {
        x >= c.rect.left
            && x < c.rect.right
            && y >= c.rect.top - EXPAND_TOP
            && y < c.rect.bottom + EXPAND_BOTTOM
    })?;
    let in_col = |it: &&DrawItem| {
        point_in(
            &col.rect,
            (it.rect.left + it.rect.right) / 2,
            (it.rect.top + it.rect.bottom) / 2,
        )
    };
    let src_here = d.items.iter().any(|it| it.id == src && in_col(&it));
    let items: Vec<&DrawItem> = d
        .items
        .iter()
        .filter(|it| it.id != src)
        .filter(in_col)
        .collect();

    // di atas dirinya sendiri: tidak ada tujuan. Selain itu, workspace asal diperlakukan
    // sama dengan workspace lain.
    if src_here
        && d.items
            .iter()
            .any(|it| it.id == src && point_in(&it.rect, x, y))
    {
        return None;
    }

    let top_item = items
        .iter()
        .copied()
        .min_by_key(|it| (it.rect.top, it.rect.left))?;
    let bottom_item = items
        .iter()
        .copied()
        .max_by_key(|it| (it.rect.bottom, -it.rect.left))?;
    let edge = |it: &DrawItem| ((it.rect.bottom - it.rect.top) * EDGE_PCT / 100).max(EDGE_MIN);
    let bar = |yy: i32| RECT {
        left: col.rect.left,
        top: yy - BAR / 2,
        right: col.rect.right,
        bottom: yy + BAR / 2,
    };

    let (kind, mark) = if y < top_item.rect.top + edge(top_item) {
        (
            SlotKind::Before(top_item.id),
            bar(top_item.rect.top - MARGIN),
        )
    } else if y >= bottom_item.rect.bottom - edge(bottom_item) {
        (
            SlotKind::After(bottom_item.id),
            bar(bottom_item.rect.bottom + MARGIN),
        )
    } else {
        // window yang memuat y (di lajur terdekat dengan x); di celah antar baris, yang
        // paling dekat secara vertikal
        let over = items
            .iter()
            .copied()
            .filter(|it| y >= it.rect.top && y < it.rect.bottom)
            .min_by_key(|it| {
                if x < it.rect.left {
                    it.rect.left - x
                } else if x >= it.rect.right {
                    x - it.rect.right + 1
                } else {
                    0
                }
            });
        let near = over.or_else(|| {
            items.iter().copied().min_by_key(|it| {
                if y < it.rect.top {
                    it.rect.top - y
                } else {
                    y - it.rect.bottom + 1
                }
            })
        })?;
        let r = near.rect;
        let inside = point_in(&r, x, y);
        match near.span {
            Span::Full if inside && clean_full(&items, near) => {
                let mid = (r.left + r.right) / 2;
                let first = x < mid;
                let mark = RECT {
                    left: if first { r.left } else { mid },
                    right: if first { mid } else { r.right },
                    top: r.top,
                    bottom: r.bottom,
                };
                (SlotKind::Split(near.id, first), mark)
            }
            Span::Full => {
                if y < (r.top + r.bottom) / 2 {
                    (SlotKind::Before(near.id), bar(r.top - MARGIN))
                } else {
                    (SlotKind::After(near.id), bar(r.bottom + MARGIN))
                }
            }
            _ if inside => {
                let mid = (r.top + r.bottom) / 2;
                let first = y < mid;
                let mark = RECT {
                    left: r.left,
                    right: r.right,
                    top: if first { r.top } else { mid },
                    bottom: if first { mid } else { r.bottom },
                };
                (SlotKind::Split(near.id, first), mark)
            }
            _ => (SlotKind::Beside(near.id), beside_rect(near, &col.rect)),
        }
    };
    Some(DropSlot {
        number: col.number,
        kind,
        mark,
    })
}

/// Window penuh lebar `x` yang tetangga atas / bawahnya (kalau ada) juga penuh lebar:
/// dibagi dua hanya memengaruhi barisnya sendiri. Kalau ada tetangga setengah lebar,
/// blok-blok di sekitarnya akan menyatu, jadi tidak ditawarkan.
fn clean_full(items: &[&DrawItem], x: &DrawItem) -> bool {
    const ADJ: i32 = 10; // jarak antar kotak yang berdekatan
    !items.iter().any(|o| {
        o.id != x.id
            && o.span != Span::Full
            && ((o.rect.top < x.rect.top && o.rect.bottom + ADJ >= x.rect.top)
                || (o.rect.bottom > x.rect.bottom && o.rect.top <= x.rect.bottom + ADJ))
    })
}

/// `slot_at` pada panel yang sedang ditampilkan.
fn slot_for(x: i32, y: i32, src: u64) -> Option<DropSlot> {
    let g = DRAW.lock().ok()?;
    slot_at(g.as_ref()?, x, y, src)
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

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}
/// Handle thumbnail DWM yang sedang terpasang.
static THUMBS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static OV_HWND: AtomicIsize = AtomicIsize::new(0);
static FONT: AtomicIsize = AtomicIsize::new(0);
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const MARGIN: i32 = 3;
const LABEL_BAND: i32 = 30;
const PAD: i32 = 20;
const HINT_H: i32 = 44;
/// Lebar minimum panel supaya teks petunjuk tidak terpotong.
const MIN_PANEL_W_WITH_HINT: i32 = 460;
/// Petunjuk dengan keterangan workspace lebih panjang.
const MIN_PANEL_W_WITH_HINT_WS: i32 = 600;
/// Jarak antar kolom workspace dan tinggi baris nomor di atasnya (piksel layar).
const COL_GAP: i32 = 16;
const COL_LABEL_H: i32 = 26;
/// Kolom workspace yang terlihat sekaligus; sisanya digulir mengikuti kursor.
pub const MAX_VISIBLE: usize = 5;
/// Lebar (dan skala) kolom yang bukan kolom kursor, persen dari kolom kursor.
const SIDE_PCT: i32 = 60;

fn channels(c: u32) -> (u32, u32, u32) {
    (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
}

/// Campur warna `c` ke arah `toward` sebesar `pct` persen.
fn mix(c: u32, toward: u32, pct: u32) -> u32 {
    let (r, g, b) = channels(c);
    let (tr, tg, tb) = channels(toward);
    let f = |a: u32, t: u32| (a * (100 - pct) + t * pct) / 100;
    rgb(f(r, tr), f(g, tg), f(b, tb))
}

/// Terang gelapnya warna, 0-255.
pub(crate) fn luminance(c: u32) -> u32 {
    let (r, g, b) = channels(c);
    (r * 299 + g * 587 + b * 114) / 1000
}

/// Data panel: peta tiap workspace yang berisi window dan bagian mana yang terlihat.
pub struct Model {
    /// (nomor workspace mulai 0, petanya), urut dari kiri.
    pub maps: Vec<(usize, Overview)>,
    /// Kolom terlihat: `maps[first .. first + visible]`. Jumlahnya tetap selama overview
    /// terbuka, jadi ukuran panel tidak berubah saat kursor pindah workspace.
    pub first: usize,
    pub visible: usize,
    /// Indeks di `maps` kolom tempat kursor berada; digambar paling besar.
    pub big: usize,
    /// Nomor workspace yang aktif (disorot).
    pub active_ws: usize,
}

/// Kolom yang terlihat untuk `columns` kolom dengan kursor di `big`: `(first, visible)`.
/// Kolom kursor ditaruh di tengah jendela kalau memungkinkan; tidak ada slot kosong.
pub fn window_for(columns: usize, big: usize) -> (usize, usize) {
    let visible = columns.min(MAX_VISIBLE);
    let first = big.saturating_sub(visible / 2).min(columns - visible);
    (first, visible)
}

/// `first` baru supaya kolom `sel` terlihat: digeser seperlunya, tidak bergerak kalau
/// kolomnya sudah terlihat.
pub fn scroll(first: usize, visible: usize, sel: usize) -> usize {
    if sel < first {
        sel
    } else if sel >= first + visible {
        sel + 1 - visible
    } else {
        first
    }
}

/// Ubah data layout jadi panel siap gambar. `work` adalah area kerja layar;
/// panel dipusatkan di sana dan tidak melebihi `size_pct` persennya. Kolom kursor
/// (`model.big`) paling besar, kolom lain `SIDE_PCT` persennya, semua rata atas dan
/// tanpa ruang kosong; skala tidak bergantung pada kolom mana yang besar.
pub fn build(
    model: &Model,
    titles: &HashMap<u64, String>,
    sources: &HashMap<u64, isize>,
    selected: u64,
    focused: u64,
    work: Rect,
    opts: &Options,
) -> Draw {
    let pct = opts.size_pct.clamp(30, 100);
    let max_w = work.w * pct / 100;
    let max_h = work.h * pct / 100;
    let hint_h = if opts.hint { HINT_H } else { 0 };
    let multi = model.maps.len() > 1;
    let label_h = if multi { COL_LABEL_H } else { 0 };
    let k = model.visible.max(1) as i32;
    let gap = if k > 1 { COL_GAP } else { 0 };
    let side = SIDE_PCT as f32 / 100.0;
    let avail_w = (max_w - 2 * PAD).max(1);
    let avail_h = (max_h - 2 * PAD - hint_h - label_h).max(1);

    let sw = model.maps.iter().map(|(_, o)| o.width).max().unwrap_or(1).max(1) as f32;
    let sh = model.maps.iter().map(|(_, o)| o.height).max().unwrap_or(1).max(1) as f32;
    let units = sw * (1.0 + side * (k - 1) as f32);
    let cols_w = (avail_w - gap * (k - 1)).max(k) as f32;
    let scale = (cols_w / units).min(avail_h as f32 / sh);
    let (big_w, side_w) = ((sw * scale) as i32, (sw * scale * side) as i32);
    let (map_w, map_h) = (big_w + (k - 1) * (side_w + gap), (sh * scale) as i32);

    let min_w = if !opts.hint {
        0
    } else if multi {
        MIN_PANEL_W_WITH_HINT_WS
    } else {
        MIN_PANEL_W_WITH_HINT
    };
    let panel_w = (map_w + 2 * PAD).max(min_w);
    let panel_h = map_h + 2 * PAD + hint_h + label_h;
    let panel = Rect {
        x: work.x + (work.w - panel_w) / 2,
        y: work.y + (work.h - panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    let top = PAD + label_h;
    let mut cx = (panel_w - map_w) / 2;
    let mut items = Vec::new();
    let mut columns = Vec::new();
    let end = (model.first + model.visible).min(model.maps.len());
    for i in model.first.min(end)..end {
        let (ws, ov) = &model.maps[i];
        let is_big = i == model.big;
        let cs = if is_big { scale } else { scale * side };
        let col_w = if is_big { big_w } else { side_w };
        let sc = |v: i32| (v as f32 * cs).round() as i32;
        for it in &ov.items {
            let l = cx + sc(it.rect.x) + MARGIN;
            let t = top + sc(it.rect.y) + MARGIN;
            let r = cx + sc(it.rect.x + it.rect.w) - MARGIN;
            let b = top + sc(it.rect.y + it.rect.h) - MARGIN;
            items.push(DrawItem {
                id: it.id.0,
                rect: RECT {
                    left: l,
                    top: t,
                    right: r.max(l + 4),
                    bottom: b.max(t + 4),
                },
                title: titles.get(&it.id.0).cloned().unwrap_or_default(),
                source: sources.get(&it.id.0).copied().unwrap_or(0),
                label_band: None,
                // dari bentuk peta: penuh lebar = hampir selebar strip; lajur kanan mulai
                // jauh dari tepi kiri
                span: if it.rect.w * 100 >= ov.width * 90 {
                    Span::Full
                } else if it.rect.x * 100 < ov.width * 12 {
                    Span::Left
                } else {
                    Span::Right
                },
            });
        }
        let vt = top + sc(ov.view_y);
        columns.push(DrawColumn {
            number: ws + 1,
            rect: RECT {
                left: cx,
                top,
                right: cx + col_w,
                bottom: top + sc(ov.height),
            },
            active: *ws == model.active_ws,
            view: RECT {
                left: cx,
                top: vt,
                right: cx + col_w,
                bottom: vt + sc(ov.view_h),
            },
        });
        cx += col_w + gap;
    }
    Draw {
        panel,
        items,
        columns,
        multi,
        selected,
        focused,
        hint: opts.hint,
        colors: opts.colors,
    }
}

/// Muat sebuah sumber berukuran (sw x sh) ke `area` tanpa mengubah rasio, di tengah.
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

/// Pasang thumbnail DWM untuk tiap kotak yang cukup besar. Kotak yang dapat
/// thumbnail mendapat `label_band` untuk labelnya; yang lain tetap berlabel di tengah.
unsafe fn register_thumbs(dest: HWND, draw: &mut Draw) -> Vec<isize> {
    let mut handles = Vec::new();
    for it in draw.items.iter_mut() {
        if it.source == 0 {
            continue;
        }
        let inner = RECT {
            left: it.rect.left + 3,
            top: it.rect.top + 3,
            right: it.rect.right - 3,
            bottom: it.rect.bottom - 3,
        };
        let area = RECT {
            left: inner.left + 4,
            top: inner.top + 4,
            right: inner.right - 4,
            bottom: inner.bottom - LABEL_BAND,
        };
        if area.right - area.left < 40 || area.bottom - area.top < 30 {
            continue; // terlalu kecil: cukup label teks
        }
        let Ok(thumb) = DwmRegisterThumbnail(dest, HWND(it.source as *mut _)) else {
            continue;
        };
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
            continue;
        }
        handles.push(thumb);
        it.label_band = Some(RECT {
            left: inner.left,
            top: inner.bottom - LABEL_BAND,
            right: inner.right,
            bottom: inner.bottom,
        });
    }
    handles
}

unsafe fn unregister_thumbs() {
    if let Ok(mut t) = THUMBS.lock() {
        for th in t.drain(..) {
            let _ = DwmUnregisterThumbnail(th);
        }
    }
}

unsafe fn post(msg: u32, wparam: usize) {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        msg,
        WPARAM(wparam),
        LPARAM(0),
    );
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    if let Ok(guard) = DRAW.lock() {
        let colors = guard
            .as_ref()
            .map_or_else(OverviewColors::default, |d| d.colors);
        let brush = |c: u32| CreateSolidBrush(COLORREF(c));
        let bg = brush(colors.background);
        let fill = brush(colors.box_fill);
        let fill_sel = brush(mix(colors.box_fill, 0xFF_FFFF, 14));
        let edge = brush(mix(colors.box_fill, 0xFF_FFFF, 18));
        let edge_active = brush(colors.active);
        let edge_sel = brush(colors.selected);
        let frame_brush = brush(colors.view);
        let col_bg = brush(mix(colors.background, 0xFF_FFFF, 5));
        let col_bg_active = brush(mix(colors.background, 0xFF_FFFF, 11));
        let col_hi = brush(mix(colors.background, colors.selected, 30));
        let slot_fill = brush(mix(colors.background, colors.selected, 60));
        // kolom di bawah kursor saat window diseret
        let drop_col = DRAG
            .lock()
            .ok()
            .and_then(|g| g.as_ref().filter(|d| d.active).and_then(|d| d.over));
        let drop_slot = DRAG
            .lock()
            .ok()
            .and_then(|g| g.as_ref().filter(|d| d.active).and_then(|d| d.slot));

        FillRect(hdc, &client, bg);
        let font = FONT.load(Ordering::Relaxed);
        if font != 0 {
            SelectObject(hdc, HGDIOBJ(font as *mut _));
        }
        SetBkMode(hdc, TRANSPARENT);

        if let Some(d) = guard.as_ref() {
            let text_color = if luminance(colors.box_fill) > 150 {
                rgb(25, 28, 36)
            } else {
                rgb(235, 238, 245)
            };
            SetTextColor(hdc, COLORREF(text_color));
            // kolom workspace: latar tipis + nomor di atasnya (hanya kalau ada > 1 workspace)
            if d.multi {
                let dim = if luminance(colors.background) > 150 {
                    rgb(90, 96, 110)
                } else {
                    rgb(150, 158, 175)
                };
                for (ci, c) in d.columns.iter().enumerate() {
                    let hi = drop_col == Some(ci);
                    FillRect(
                        hdc,
                        &c.rect,
                        if hi {
                            col_hi
                        } else if c.active {
                            col_bg_active
                        } else {
                            col_bg
                        },
                    );
                    if hi {
                        // bingkai tebal di sekeliling kolom tujuan
                        let r = c.rect;
                        for edge_rect in [
                            RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + 3 },
                            RECT { left: r.left, top: r.bottom - 3, right: r.right, bottom: r.bottom },
                            RECT { left: r.left, top: r.top, right: r.left + 3, bottom: r.bottom },
                            RECT { left: r.right - 3, top: r.top, right: r.right, bottom: r.bottom },
                        ] {
                            FillRect(hdc, &edge_rect, edge_sel);
                        }
                    }
                    let mut label: Vec<u16> = c.number.to_string().encode_utf16().collect();
                    let mut lr = RECT {
                        left: c.rect.left,
                        top: c.rect.top - COL_LABEL_H,
                        right: c.rect.right,
                        bottom: c.rect.top,
                    };
                    SetTextColor(hdc, COLORREF(if c.active { colors.active } else { dim }));
                    DrawTextW(
                        hdc,
                        &mut label,
                        &mut lr,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                SetTextColor(hdc, COLORREF(text_color));
            }
            for it in &d.items {
                let selected = it.id == d.selected;
                let active = it.id == d.focused;
                let r = it.rect;
                let border = if selected {
                    edge_sel
                } else if active {
                    edge_active
                } else {
                    edge
                };
                FillRect(hdc, &r, border);
                let inner = RECT {
                    left: r.left + 3,
                    top: r.top + 3,
                    right: (r.right - 3).max(r.left + 4),
                    bottom: (r.bottom - 3).max(r.top + 4),
                };
                FillRect(hdc, &inner, if selected { fill_sel } else { fill });
                if !it.title.is_empty() {
                    let mut text: Vec<u16> = it.title.encode_utf16().collect();
                    match it.label_band {
                        // ada thumbnail: label di pita bawah, rata kiri
                        Some(band) => {
                            let mut tr = RECT {
                                left: band.left + 8,
                                top: band.top,
                                right: (band.right - 8).max(band.left + 9),
                                bottom: band.bottom,
                            };
                            DrawTextW(
                                hdc,
                                &mut text,
                                &mut tr,
                                DT_LEFT
                                    | DT_VCENTER
                                    | DT_SINGLELINE
                                    | DT_END_ELLIPSIS
                                    | DT_NOPREFIX,
                            );
                        }
                        // tanpa thumbnail: label di tengah kotak
                        None => {
                            let mut tr = RECT {
                                left: inner.left + 8,
                                top: inner.top,
                                right: (inner.right - 8).max(inner.left + 9),
                                bottom: inner.bottom,
                            };
                            DrawTextW(
                                hdc,
                                &mut text,
                                &mut tr,
                                DT_CENTER
                                    | DT_VCENTER
                                    | DT_SINGLELINE
                                    | DT_END_ELLIPSIS
                                    | DT_NOPREFIX,
                            );
                        }
                    }
                }
            }

            // bingkai bagian (baris) yang sedang tampil di layar, per workspace
            for c in &d.columns {
                let v = c.view;
                let t = 3;
                let brush = if c.active { frame_brush } else { edge };
                for edge_rect in [
                    RECT {
                        left: v.left,
                        top: v.top,
                        right: v.right,
                        bottom: v.top + t,
                    },
                    RECT {
                        left: v.left,
                        top: v.bottom - t,
                        right: v.right,
                        bottom: v.bottom,
                    },
                    RECT {
                        left: v.left,
                        top: v.top,
                        right: v.left + t,
                        bottom: v.bottom,
                    },
                    RECT {
                        left: v.right - t,
                        top: v.top,
                        right: v.right,
                        bottom: v.bottom,
                    },
                ] {
                    FillRect(hdc, &edge_rect, brush);
                }
            }

            if d.hint {
                let hint_color = if luminance(colors.background) > 150 {
                    rgb(90, 96, 110)
                } else {
                    rgb(150, 158, 175)
                };
                SetTextColor(hdc, COLORREF(hint_color));
                let hint_text = if d.multi {
                    "←↑↓→ select (←→ at an edge = other workspace)  ·  Enter jump  ·  Esc cancel"
                } else {
                    "←↑↓→ select  ·  Enter jump  ·  Esc cancel"
                };
                let mut hint: Vec<u16> = hint_text.encode_utf16().collect();
                let mut hr = RECT {
                    left: client.left,
                    top: client.bottom - HINT_H,
                    right: client.right,
                    bottom: client.bottom - 4,
                };
                DrawTextW(
                    hdc,
                    &mut hint,
                    &mut hr,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }

        // penanda tujuan drop: di mana window yang diseret akan mendarat
        if let Some(sl) = drop_slot {
            match sl.kind {
                SlotKind::Beside(_) | SlotKind::Split(..) => {
                    FillRect(hdc, &sl.mark, slot_fill);
                    frame(hdc, &sl.mark, edge_sel, 3);
                }
                SlotKind::Before(_) | SlotKind::After(_) => {
                    FillRect(hdc, &sl.mark, edge_sel);
                }
            }
        }

        for b in [
            bg,
            fill,
            fill_sel,
            edge,
            edge_active,
            edge_sel,
            frame_brush,
            col_bg,
            col_bg_active,
            col_hi,
            slot_fill,
        ] {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn ov_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            if ctrl && vk == VK_LEFT.0 {
                post(WM_OV_WS, 0);
            } else if ctrl && vk == VK_RIGHT.0 {
                post(WM_OV_WS, 1);
            } else if (0x31..=0x39).contains(&vk) {
                post(WM_OV_WS, 2 + (vk - 0x31) as usize); // tombol 1..9 = workspace n
            } else if vk == VK_LEFT.0 {
                post(WM_OV_MOVE, 0);
            } else if vk == VK_RIGHT.0 {
                post(WM_OV_MOVE, 1);
            } else if vk == VK_UP.0 {
                post(WM_OV_MOVE, 2);
            } else if vk == VK_DOWN.0 {
                post(WM_OV_MOVE, 3);
            } else if vk == VK_RETURN.0 {
                post(WM_OV_ACCEPT, 0);
            } else if vk == VK_ESCAPE.0 {
                // saat window sedang diseret, Esc hanya membatalkan seretan
                let dragging = DRAG.lock().ok().map_or(false, |g| g.is_some());
                if dragging {
                    if let Ok(mut g) = DRAG.lock() {
                        *g = None;
                    }
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else {
                    post(WM_OV_CANCEL, 0);
                }
            } else if vk == VK_TAB.0 {
                let back = GetKeyState(VK_SHIFT.0 as i32) < 0;
                post(WM_OV_CYCLE, if back { 0 } else { 1 });
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            if let (_, _, Some(id)) = hit_at(x, y, 0) {
                if let Ok(mut g) = DRAG.lock() {
                    *g = Some(Drag {
                        id,
                        start: (x, y),
                        active: false,
                        over: None,
                        slot: None,
                    });
                }
                let _ = SetCapture(hwnd); // gerakan mouse tetap sampai ke sini saat diseret
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lparam);
            let src = DRAG.lock().ok().and_then(|g| g.as_ref().map(|d| d.id));
            if let Some(src) = src {
                let (col, _, _) = hit_at(x, y, src);
                let slot = slot_for(x, y, src);
                let mut repaint = false;
                if let Ok(mut g) = DRAG.lock() {
                    if let Some(d) = g.as_mut() {
                        if !d.active
                            && ((x - d.start.0).abs() > DRAG_START
                                || (y - d.start.1).abs() > DRAG_START)
                        {
                            d.active = true;
                            repaint = true;
                        }
                        if d.active && (d.over != col || !same_slot(&d.slot, &slot)) {
                            d.over = col;
                            d.slot = slot;
                            repaint = true;
                        }
                    }
                }
                if repaint {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let active = DRAG
                .lock()
                .ok()
                .map_or(false, |g| g.as_ref().map_or(false, |d| d.active));
            if active {
                if let Ok(c) = LoadCursorW(None, IDC_SIZEALL) {
                    let _ = SetCursor(Some(c));
                }
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CAPTURECHANGED => {
            if let Ok(mut g) = DRAG.lock() {
                *g = None;
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lparam);
            let drag = DRAG.lock().ok().and_then(|mut g| g.take());
            let _ = ReleaseCapture();
            if let Some(d) = drag.filter(|d| d.active) {
                // dilepas setelah diseret: ke titik tujuan di bawah kursor; di tempat yang
                // tidak berlaku, seretan batal begitu saja
                if let Some(slot) = slot_for(x, y, d.id) {
                    if let Ok(mut g) = DROP.lock() {
                        *g = Some((d.id, slot));
                    }
                    post(WM_OV_DROP, 0);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                return LRESULT(0);
            }
            // klik biasa: loncat ke window di bawah kursor, atau batal kalau di ruang kosong
            match hit_at(x, y, 0).2 {
                Some(id) => post(WM_OV_PICK, id as usize),
                None => post(WM_OV_CANCEL, 0),
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            post(WM_OV_CANCEL, 0);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // klik di luar panel / pindah ke app lain: batalkan
            if (wparam.0 & 0xFFFF) == 0 && VISIBLE.load(Ordering::Relaxed) {
                post(WM_OV_CANCEL, 0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Buat jendela overlay (tersembunyi) sekali di awal. Sengaja bukan jendela
/// berlapis (layered): thumbnail DWM tidak tampil benar di jendela berlapis.
pub unsafe fn init(main_thread: u32) -> bool {
    MAIN_THREAD.store(main_thread, Ordering::Relaxed);
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmOverview");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(ov_proc),
        hInstance: hinst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return false;
    }
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        w!("vscroll-wm overview"),
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
    OV_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    // sudut membulat (Windows 11; di Windows 10 diabaikan)
    let round: u32 = 2; // DWMWCP_ROUND
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &round as *const u32 as *const c_void,
        4,
    );

    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = -22;
    lf.lfWeight = 400;
    for (i, ch) in "Segoe UI".encode_utf16().enumerate() {
        lf.lfFaceName[i] = ch;
    }
    let font = CreateFontIndirectW(&lf);
    FONT.store(font.0 as isize, Ordering::Relaxed);
    true
}

static SUPPRESSED: AtomicBool = AtomicBool::new(false);

pub fn is_suppressed() -> bool {
    SUPPRESSED.load(Ordering::Relaxed)
}

pub unsafe fn set_suppressed(on: bool) {
    SUPPRESSED.store(on, Ordering::Relaxed);
    if on {
        hide();
    }
}

/// True kalau jendela overlay berhasil dibuat.
pub fn available() -> bool {
    !is_suppressed() && OV_HWND.load(Ordering::Relaxed) != 0
}

fn hwnd() -> HWND {
    HWND(OV_HWND.load(Ordering::Relaxed) as *mut _)
}

/// Tampilkan panel di posisinya dan ambil fokus keyboard.
pub unsafe fn show(mut draw: Draw, thumbnails: bool) {
    if OV_HWND.load(Ordering::Relaxed) == 0 {
        return;
    }
    let h = hwnd();
    unregister_thumbs(); // jaga-jaga kalau sisa dari tampilan sebelumnya
    let handles = if thumbnails {
        register_thumbs(h, &mut draw)
    } else {
        Vec::new()
    };
    if let Ok(mut t) = THUMBS.lock() {
        *t = handles;
    }
    let p = draw.panel;
    if let Ok(mut g) = DRAW.lock() {
        *g = Some(draw);
    }
    VISIBLE.store(true, Ordering::Relaxed);
    let _ = SetWindowPos(h, Some(HWND_TOPMOST), p.x, p.y, p.w, p.h, SWP_SHOWWINDOW);
    let _ = SetForegroundWindow(h);
    let _ = InvalidateRect(Some(h), None, false);
}

/// Ganti window yang sedang dipilih (kursor overview).
pub unsafe fn set_selected(id: u64) {
    if let Ok(mut g) = DRAW.lock() {
        if let Some(d) = g.as_mut() {
            d.selected = id;
        }
    }
    let _ = InvalidateRect(Some(hwnd()), None, false);
}

pub unsafe fn hide() {
    VISIBLE.store(false, Ordering::Relaxed);
    unregister_thumbs();
    if OV_HWND.load(Ordering::Relaxed) != 0 {
        let _ = ShowWindow(hwnd(), SW_HIDE);
    }
    if let Ok(mut g) = DRAW.lock() {
        *g = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout::{OverviewItem, WindowId};

    fn ov() -> Overview {
        let item = |id, x, y, w, h| OverviewItem {
            id: WindowId(id),
            rect: Rect { x, y, w, h },
        };
        Overview {
            items: vec![
                item(1, 0, 0, 960, 540),
                item(2, 960, 0, 960, 540),
                item(3, 0, 540, 1920, 540),
            ],
            width: 1920,
            height: 1080,
            view_y: 0,
            view_h: 1080,
        }
    }

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    fn opts(hint: bool, size_pct: i32) -> Options {
        Options {
            hint,
            size_pct,
            colors: OverviewColors::default(),
        }
    }

    fn single(o: &Overview) -> Model {
        Model {
            maps: vec![(0, o.clone())],
            first: 0,
            visible: 1,
            big: 0,
            active_ws: 0,
        }
    }

    /// `n` workspace berisi peta yang sama; kursor dan workspace aktif di `big`.
    fn model(n: usize, big: usize) -> Model {
        let (first, visible) = window_for(n, big);
        Model {
            maps: (0..n).map(|i| (i, ov())).collect(),
            first,
            visible,
            big,
            active_ws: big,
        }
    }

    fn build_with(o: &Options) -> Draw {
        build(&single(&ov()), &HashMap::new(), &HashMap::new(), 1, 1, WORK, o)
    }

    #[test]
    fn panel_is_centered_and_within_the_requested_size() {
        let d = build_with(&opts(true, 70));
        assert!(d.panel.w <= WORK.w * 70 / 100 + 1);
        assert!(d.panel.h <= WORK.h * 70 / 100 + 1);
        let (cx, cy) = (d.panel.x + d.panel.w / 2, d.panel.y + d.panel.h / 2);
        assert!((cx - WORK.w / 2).abs() <= 1 && (cy - WORK.h / 2).abs() <= 1);
    }

    #[test]
    fn boxes_fit_inside_the_panel_and_do_not_overlap() {
        let d = build_with(&opts(true, 70));
        assert_eq!(d.items.len(), 3);
        for it in &d.items {
            assert!(it.rect.left >= 0 && it.rect.top >= 0);
            assert!(it.rect.right <= d.panel.w && it.rect.bottom <= d.panel.h);
            assert!(it.rect.right > it.rect.left && it.rect.bottom > it.rect.top);
        }
        let (a, b, c) = (d.items[0].rect, d.items[1].rect, d.items[2].rect);
        assert!(a.right <= b.left);
        assert!(a.bottom <= c.top && b.bottom <= c.top);
    }

    #[test]
    fn bigger_size_setting_gives_bigger_panel() {
        let (small, big) = (build_with(&opts(true, 40)), build_with(&opts(true, 90)));
        assert!(small.panel.w < big.panel.w && small.panel.h < big.panel.h);
    }

    #[test]
    fn hiding_the_hint_shrinks_the_space_below_the_map() {
        let (on, off) = (build_with(&opts(true, 70)), build_with(&opts(false, 70)));
        let below = |d: &Draw| d.panel.h - d.items[2].rect.bottom;
        assert!(below(&off) < below(&on));
        assert!(!off.hint && on.hint);
    }

    #[test]
    fn narrow_strip_still_leaves_room_for_the_hint_text() {
        let mut o = ov();
        o.height = 8000; // strip sangat tinggi -> peta sangat sempit
        let d = build(
            &single(&o),
            &HashMap::new(),
            &HashMap::new(),
            1,
            1,
            WORK,
            &opts(true, 70),
        );
        assert!(d.panel.w >= MIN_PANEL_W_WITH_HINT);
        let d2 = build(
            &single(&o),
            &HashMap::new(),
            &HashMap::new(),
            1,
            1,
            WORK,
            &opts(false, 70),
        );
        assert!(d2.panel.w < MIN_PANEL_W_WITH_HINT);
    }

    #[test]
    fn titles_sources_selection_and_colors_are_carried_over() {
        let mut titles = HashMap::new();
        titles.insert(2u64, "Claude".to_string());
        let mut sources = HashMap::new();
        sources.insert(2u64, 1234isize);
        let mut o = opts(true, 70);
        o.colors.selected = rgb(1, 2, 3);
        let d = build(&single(&ov()), &titles, &sources, 2, 3, WORK, &o);
        assert_eq!(d.items[1].title, "Claude");
        assert_eq!(d.items[1].source, 1234);
        assert_eq!(d.items[0].title, "");
        assert_eq!(d.items[0].source, 0);
        assert_eq!((d.selected, d.focused), (2, 3));
        assert_eq!(d.colors.selected, rgb(1, 2, 3));
    }

    #[test]
    fn viewport_frame_tracks_the_visible_part() {
        let mut o = ov();
        o.height = 2160; // dua layar tinggi; yang tampil: layar bawah
        o.view_y = 1080;
        let d = build(
            &single(&o),
            &HashMap::new(),
            &HashMap::new(),
            1,
            1,
            WORK,
            &opts(true, 70),
        );
        let view = d.columns[0].view;
        assert!(view.bottom > view.top);
        assert!(view.bottom <= d.panel.h);
        assert!(view.top > d.items[0].rect.top); // bingkai tidak di puncak peta
    }

    #[test]
    fn fit_keeps_aspect_ratio_and_stays_inside() {
        let area = RECT {
            left: 100,
            top: 100,
            right: 500,
            bottom: 300,
        }; // 400x200
        let r = fit(area, 1920, 1080);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        assert!(r.left >= area.left && r.right <= area.right);
        assert!(r.top >= area.top && r.bottom <= area.bottom);
        assert!((w as f32 / h as f32 - 1920.0 / 1080.0).abs() < 0.05);
        assert_eq!(h, 200); // dibatasi tinggi
    }

    #[test]
    fn fit_falls_back_to_the_area_for_unknown_source_size() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 50,
        };
        let r = fit(area, 0, 0);
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 100, 50));
    }

    #[test]
    fn color_helpers_behave() {
        assert!(luminance(rgb(255, 255, 255)) > 250);
        assert!(luminance(rgb(0, 0, 0)) < 5);
        assert!(luminance(rgb(20, 22, 28)) < 150); // latar gelap bawaan -> teks terang
        let lighter = mix(rgb(40, 40, 40), 0xFF_FFFF, 20);
        assert!(luminance(lighter) > luminance(rgb(40, 40, 40)));
        assert_eq!(mix(rgb(10, 20, 30), rgb(200, 200, 200), 0), rgb(10, 20, 30));
    }

    fn build_model(m: &Model) -> Draw {
        build(m, &HashMap::new(), &HashMap::new(), 1, 1, WORK, &opts(true, 70))
    }

    fn width(c: &DrawColumn) -> i32 {
        c.rect.right - c.rect.left
    }

    #[test]
    fn cursor_column_is_bigger_than_the_others() {
        let d = build_model(&model(3, 1));
        assert_eq!(d.columns.len(), 3);
        let (l, m, r) = (width(&d.columns[0]), width(&d.columns[1]), width(&d.columns[2]));
        assert!(m > l && m > r);
        assert_eq!(l, r);
    }

    #[test]
    fn panel_size_does_not_depend_on_which_column_is_big() {
        let a = build_model(&model(4, 0));
        let b = build_model(&model(4, 3));
        assert_eq!((a.panel.w, a.panel.h), (b.panel.w, b.panel.h));
        let c = build_model(&model(9, 0));
        let d = build_model(&model(9, 8));
        assert_eq!((c.panel.w, c.panel.h), (d.panel.w, d.panel.h));
    }

    #[test]
    fn columns_touch_no_empty_slots_and_margins_are_equal() {
        let o = opts(false, 70); // tanpa petunjuk: tidak ada lebar minimum
        for n in 1..=9 {
            for big in 0..n {
                let d = build(&model(n, big), &HashMap::new(), &HashMap::new(), 1, 1, WORK, &o);
                let (first, last) = (d.columns.first().unwrap(), d.columns.last().unwrap());
                assert!(
                    (first.rect.left - (d.panel.w - last.rect.right)).abs() <= 1,
                    "n={n} big={big}"
                );
                assert!(first.rect.left >= 0 && last.rect.right <= d.panel.w);
                for pair in d.columns.windows(2) {
                    assert_eq!(pair[1].rect.left - pair[0].rect.right, COL_GAP);
                    assert_eq!(pair[0].rect.top, pair[1].rect.top); // rata atas
                }
            }
        }
    }

    #[test]
    fn cursor_column_is_centered_whenever_both_sides_have_the_same_count() {
        for n in 1..=9usize {
            for big in 0..n {
                let (first, visible) = window_for(n, big);
                let (left, right) = (big - first, first + visible - 1 - big);
                if left != right {
                    continue;
                }
                let d = build_model(&model(n, big));
                let c = d.columns.iter().find(|c| c.active).unwrap();
                let mid = (c.rect.left + c.rect.right) / 2;
                assert!((mid - d.panel.w / 2).abs() <= 1, "n={n} big={big}");
            }
        }
    }

    #[test]
    fn window_for_never_leaves_slots_empty_and_contains_the_cursor() {
        assert_eq!(window_for(1, 0), (0, 1));
        assert_eq!(window_for(3, 1), (0, 3));
        assert_eq!(window_for(9, 0), (0, MAX_VISIBLE));
        assert_eq!(window_for(9, 4), (2, MAX_VISIBLE));
        assert_eq!(window_for(9, 8), (4, MAX_VISIBLE));
        for n in 1..=12usize {
            for big in 0..n {
                let (first, visible) = window_for(n, big);
                assert_eq!(visible, n.min(MAX_VISIBLE));
                assert!(first + visible <= n);
                assert!(big >= first && big < first + visible);
            }
        }
    }

    #[test]
    fn scroll_keeps_the_selected_column_visible_and_stays_put_otherwise() {
        for n in 1..=12usize {
            let visible = n.min(MAX_VISIBLE);
            for first in 0..=(n - visible) {
                for sel in 0..n {
                    let f = scroll(first, visible, sel);
                    assert!(f + visible <= n, "n={n} first={first} sel={sel}");
                    assert!(sel >= f && sel < f + visible);
                    if sel >= first && sel < first + visible {
                        assert_eq!(f, first);
                    }
                }
            }
        }
    }

    #[test]
    fn only_the_visible_window_of_columns_is_drawn() {
        let mut m = model(7, 3);
        m.first = 1;
        m.visible = 5;
        let d = build_model(&m);
        assert_eq!(d.columns.len(), 5);
        assert_eq!(d.items.len(), 15); // 5 kolom x 3 window
        assert_eq!(d.columns[0].number, 2);
    }

    #[test]
    fn columns_use_workspace_numbers_and_flag_only_the_active_one() {
        let m = Model {
            maps: vec![(0, ov()), (2, ov()), (3, ov())],
            first: 0,
            visible: 3,
            big: 1,
            active_ws: 2,
        };
        let d = build_model(&m);
        let nums: Vec<usize> = d.columns.iter().map(|c| c.number).collect();
        let act: Vec<bool> = d.columns.iter().map(|c| c.active).collect();
        assert_eq!(nums, vec![1, 3, 4]);
        assert_eq!(act, vec![false, true, false]);
        assert!(d.multi);
    }

    #[test]
    fn single_workspace_has_no_label_row() {
        let d = build_with(&opts(true, 70));
        assert!(!d.multi);
        assert_eq!(d.columns.len(), 1);
    }

    // ---- titik drop ----

    fn item(id: u64, span: Span, l: i32, t: i32, r: i32, b: i32) -> DrawItem {
        DrawItem {
            id,
            rect: RECT { left: l, top: t, right: r, bottom: b },
            title: String::new(),
            source: 0,
            label_band: None,
            span,
        }
    }

    fn column(number: usize, l: i32, t: i32, r: i32, b: i32) -> DrawColumn {
        let rect = RECT { left: l, top: t, right: r, bottom: b };
        DrawColumn { number, rect, active: false, view: rect }
    }

    /// Kolom 1: A (kiri, atas), B (penuh, tengah), C (kiri, bawah). Kolom 2: D (penuh).
    fn drop_draw() -> Draw {
        Draw {
            panel: Rect { x: 0, y: 0, w: 600, h: 500 },
            items: vec![
                item(1, Span::Left, 3, 53, 97, 203),
                item(2, Span::Full, 3, 203, 197, 303),
                item(3, Span::Left, 3, 303, 97, 447),
                item(4, Span::Full, 303, 53, 497, 447),
            ],
            columns: vec![column(1, 0, 50, 200, 450), column(2, 300, 50, 500, 450)],
            multi: true,
            selected: 4,
            focused: 4,
            hint: false,
            colors: OverviewColors::default(),
        }
    }

    fn kind(d: &Draw, x: i32, y: i32) -> Option<(usize, SlotKind)> {
        slot_at(d, x, y, 4).map(|s| (s.number, s.kind))
    }

    #[test]
    fn dropping_on_a_half_width_window_stacks_in_its_lane() {
        let d = drop_draw();
        // A (kiri, 53..203): separuh atas -> window baru di atas A, separuh bawah -> di bawah
        assert_eq!(kind(&d, 50, 100), Some((1, SlotKind::Split(1, true))));
        assert_eq!(kind(&d, 50, 180), Some((1, SlotKind::Split(1, false))));
        let m = slot_at(&d, 50, 180, 4).unwrap().mark; // separuh bawah A
        assert_eq!((m.left, m.right, m.bottom), (3, 97, 203));
        assert_eq!(m.top, 128);
        // C (kiri, 303..447)
        assert_eq!(kind(&d, 50, 340), Some((1, SlotKind::Split(3, true))));
        assert_eq!(kind(&d, 50, 400), Some((1, SlotKind::Split(3, false))));
    }

    #[test]
    fn the_empty_half_next_to_a_window_is_filled_beside_it() {
        let d = drop_draw();
        assert_eq!(kind(&d, 150, 128), Some((1, SlotKind::Beside(1)))); // sebelah A
        assert_eq!(kind(&d, 150, 360), Some((1, SlotKind::Beside(3)))); // sebelah C
        let m = slot_at(&d, 150, 128, 4).unwrap().mark;
        assert_eq!((m.left, m.top, m.bottom, m.right), (100, 53, 203, 200));
    }

    #[test]
    fn dropping_on_a_lone_full_window_splits_it_side_by_side() {
        // kolom 1 hanya berisi E (penuh); window yang diseret D ada di kolom 2
        let mut d = drop_draw();
        d.items = vec![
            item(5, Span::Full, 3, 53, 197, 447),
            item(4, Span::Full, 303, 53, 497, 447),
        ];
        assert_eq!(kind(&d, 50, 250), Some((1, SlotKind::Split(5, true)))); // sisi kiri
        assert_eq!(kind(&d, 150, 250), Some((1, SlotKind::Split(5, false)))); // sisi kanan
        let m = slot_at(&d, 150, 250, 4).unwrap().mark;
        assert_eq!((m.left, m.right, m.top, m.bottom), (100, 197, 53, 447));
        // tepinya tetap penuh lebar di luar window
        assert_eq!(kind(&d, 100, 60), Some((1, SlotKind::Before(5))));
        assert_eq!(kind(&d, 100, 440), Some((1, SlotKind::After(5))));
    }

    #[test]
    fn a_full_window_between_full_windows_can_be_split_too() {
        let mut d = drop_draw();
        d.items = vec![
            item(5, Span::Full, 3, 53, 197, 163),
            item(6, Span::Full, 3, 163, 197, 303),
            item(7, Span::Full, 3, 303, 197, 447),
            item(4, Span::Full, 303, 53, 497, 447),
        ];
        assert_eq!(kind(&d, 50, 230), Some((1, SlotKind::Split(6, true))));
    }

    #[test]
    fn dropping_at_the_top_or_bottom_edge_makes_it_full_width() {
        let d = drop_draw();
        assert_eq!(kind(&d, 50, 60), Some((1, SlotKind::Before(1)))); // zona atas A
        assert_eq!(kind(&d, 150, 40), Some((1, SlotKind::Before(1)))); // di atas kolom (label)
        assert_eq!(kind(&d, 50, 430), Some((1, SlotKind::After(3)))); // zona bawah C
        assert_eq!(kind(&d, 150, 460), Some((1, SlotKind::After(3)))); // di bawah kolom
        // penanda berupa garis selebar kolom
        let m = slot_at(&d, 50, 60, 4).unwrap().mark;
        assert_eq!((m.left, m.right), (0, 200));
        assert!(m.bottom - m.top <= BAR + 1);
    }

    #[test]
    fn dropping_on_a_full_width_window_splits_by_halves() {
        let d = drop_draw();
        assert_eq!(kind(&d, 100, 220), Some((1, SlotKind::Before(2)))); // separuh atas B
        assert_eq!(kind(&d, 100, 290), Some((1, SlotKind::After(2)))); // separuh bawah B
    }

    #[test]
    fn dropping_outside_any_column_has_no_target() {
        let d = drop_draw();
        assert_eq!(kind(&d, 250, 200), None); // celah antar kolom
        assert_eq!(kind(&d, 550, 200), None);
        assert_eq!(kind(&d, 100, 20), None); // jauh di atas
    }

    #[test]
    fn inside_the_source_workspace_behaves_like_other_workspaces() {
        let d = drop_draw();
        // window yang diseret C (kolom 1): di atas A (setengah lebar) -> membagi A
        let s = slot_at(&d, 50, 100, 3).unwrap();
        assert_eq!((s.number, s.kind), (1, SlotKind::Split(1, true)));
        // ruang kosong di sebelah A -> mengisi lajur seberang
        let s = slot_at(&d, 150, 128, 3).unwrap();
        assert_eq!(s.kind, SlotKind::Beside(1));
        // di atas dirinya sendiri: tidak ada tujuan
        assert!(slot_at(&d, 50, 360, 3).is_none());
    }

    #[test]
    fn the_span_of_each_box_comes_from_the_overview_map() {
        let ov = Overview {
            items: vec![
                OverviewItem { id: WindowId(1), rect: Rect { x: 0, y: 0, w: 960, h: 540 } },
                OverviewItem { id: WindowId(2), rect: Rect { x: 960, y: 0, w: 960, h: 540 } },
                OverviewItem { id: WindowId(3), rect: Rect { x: 0, y: 540, w: 1920, h: 540 } },
            ],
            width: 1920,
            height: 1080,
            view_y: 0,
            view_h: 1080,
        };
        let m = Model { maps: vec![(0, ov)], first: 0, visible: 1, big: 0, active_ws: 0 };
        let d = build(
            &m,
            &HashMap::new(),
            &HashMap::new(),
            1,
            1,
            Rect { x: 0, y: 0, w: 1920, h: 1080 },
            &Options { hint: false, size_pct: 60, colors: OverviewColors::default() },
        );
        let spans: Vec<Span> = d.items.iter().map(|i| i.span).collect();
        assert_eq!(spans, vec![Span::Left, Span::Right, Span::Full]);
    }
}
