//! Tipe data publik layout engine.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Height {
    Third,
    Half,
    Full,
    /// Tinggi bebas dari mode `step`: permil tinggi area konten (15-100%).
    Custom(i32),
}

impl Height {
    pub(crate) fn grow(self) -> Self {
        match self {
            Height::Third => Height::Half,
            Height::Custom(p) if p < 333 => Height::Third,
            Height::Custom(p) if p < 500 => Height::Half,
            _ => Height::Full,
        }
    }
    pub(crate) fn shrink(self) -> Self {
        match self {
            Height::Full => Height::Half,
            Height::Custom(p) if p > 500 => Height::Half,
            Height::Custom(p) if p > 333 => Height::Third,
            Height::Custom(_) => self, // sudah di bawah 1/3: tidak ada preset lebih kecil
            _ => Height::Third,
        }
    }
    /// Setengah dari tinggi ini (membagi dua window yang bertumpuk): Full -> Half, Half ->
    /// 1/4, selebihnya permil / 2 (minimal 15%).
    pub(crate) fn halved(self) -> Self {
        match self {
            Height::Full => Height::Half,
            Height::Half => Height::Custom(250),
            Height::Third => Height::Custom(166),
            Height::Custom(p) => Height::Custom((p / 2).max(150)),
        }
    }
    pub(crate) fn pixels(self, viewport_h: i32) -> i32 {
        match self {
            Height::Third => viewport_h / 3,
            Height::Half => viewport_h / 2,
            Height::Full => viewport_h,
            Height::Custom(p) => viewport_h * p / 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Full,
    Left,
    Right,
    /// Tersembunyi (di-minimize, cloaked, dll): tidak ikut layout, tapi slotnya
    /// (urutan, lebar `restore`, tinggi) diingat sampai muncul lagi.
    Hidden,
}

impl Span {
    pub(crate) fn opposite(self) -> Span {
        match self {
            Span::Left => Span::Right,
            Span::Right => Span::Left,
            Span::Full => Span::Full,
            Span::Hidden => Span::Hidden,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub id: WindowId,
    pub span: Span,
    pub height: Height,
    /// Lebar yang dipulihkan saat window muncul lagi (dipakai kalau `span` = Hidden).
    pub restore: Span,
    /// Lebar lajur kiri dalam permil (500 = 50%). Berlaku untuk seluruh blok;
    /// yang dipakai adalah nilai item lajur pertama di blok itu.
    pub split: i32,
}

/// Hasil layout untuk satu window. `visible = false` berarti seluruhnya
/// di luar viewport (platform layer bebas menyembunyikan / memarkir window itu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub id: WindowId,
    pub rect: Rect,
    pub visible: bool,
}

/// Tempat window disisipkan ke sebuah strip (drag di overview). Selalu relatif terhadap
/// window yang sudah ada di strip itu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    /// Di lajur seberang window setengah lebar ini, sejajar dengannya (satu blok).
    Beside(WindowId),
    /// Penuh lebar, di atas blok window ini (pemisah blok).
    FullBefore(WindowId),
    /// Penuh lebar, di bawah blok window ini (pemisah blok).
    FullAfter(WindowId),
    /// Membagi dua window ini; `bool` = window baru di sisi pertama (kiri / atas).
    ///  - window penuh lebar: lebarnya dibagi dua, berdampingan (tinggi tetap)
    ///  - window setengah lebar: tingginya dibagi dua, bertumpuk di lajur yang sama
    Split(WindowId, bool),
}

/// Satu window di peta overview (koordinat strip, tanpa scroll).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewItem {
    pub id: WindowId,
    pub rect: Rect,
}

/// Peta seluruh strip untuk tampilan overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub items: Vec<OverviewItem>,
    /// Lebar dan tinggi total strip.
    pub width: i32,
    pub height: i32,
    /// Bagian strip yang sedang tampil di layar: mulai dari `view_y`, setinggi `view_h`.
    pub view_y: i32,
    pub view_h: i32,
}

pub struct Strip {
    pub(crate) items: Vec<Item>,
    pub(crate) focused: Option<WindowId>,
    pub(crate) scroll_y: i32,
    pub(crate) viewport: Rect,
    /// Tinggi minimum yang diukur dari window asli (app menolak dikecilkan).
    pub(crate) min_heights: HashMap<WindowId, i32>,
    /// Lebar minimum yang diukur dari window asli (app menolak dipersempit).
    pub(crate) min_widths: HashMap<WindowId, i32>,
    /// Jarak antar window dan ke tepi layar (piksel).
    pub(crate) gap: i32,
    /// Tukar posisi terakhir window penuh lebar, belum diambil app (lihat `take_move_cue`).
    pub(crate) cue: Option<(WindowId, Dir)>,
}
