//! Operasi yang mengubah strip: tambah/hapus, snap, fokus, geser, pembagi lajur.

use std::collections::HashMap;

use crate::flow::Flow;
use crate::types::*;

impl Strip {
    pub fn new(viewport: Rect) -> Self {
        Self {
            items: Vec::new(),
            focused: None,
            scroll_y: 0,
            viewport,
            min_heights: HashMap::new(),
            min_widths: HashMap::new(),
            gap: 0,
            cue: None,
        }
    }

    /// Atur gap. Antar window dan ke tepi layar sama besar.
    pub fn set_gap(&mut self, gap: i32) {
        self.gap = gap.max(0);
        self.ensure_visible();
    }

    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn set_viewport(&mut self, viewport: Rect) {
        self.viewport = viewport;
        self.ensure_visible();
    }

    /// Catat tinggi minimum window. True kalau nilainya naik (layout berubah).
    pub fn set_min_height(&mut self, id: WindowId, h: i32) -> bool {
        let h = h.min((self.content().h - 2 * (self.gap / 2)).max(0));
        let cur = self.min_heights.get(&id).copied().unwrap_or(0);
        if h > cur {
            self.min_heights.insert(id, h);
            self.ensure_visible();
            true
        } else {
            false
        }
    }

    /// Catat lebar minimum window. True kalau nilainya naik (layout berubah).
    pub fn set_min_width(&mut self, id: WindowId, w: i32) -> bool {
        let w = w.min((self.content().w - 2 * (self.gap / 2)).max(0));
        let cur = self.min_widths.get(&id).copied().unwrap_or(0);
        if w > cur {
            self.min_widths.insert(id, w);
            true
        } else {
            false
        }
    }

    /// Geser pembagi lajur pada blok window yang fokus: `delta_px` > 0 ke kanan,
    /// < 0 ke kiri. Lebar dimiliki lajur, jadi semua window di lajur itu ikut.
    /// False kalau tidak ada yang bergeser (window penuh lebar, atau sudah di batas).
    pub fn adjust_split(&mut self, delta_px: i32) -> bool {
        let Some(i) = self.focused.and_then(|f| self.index_of(f)) else {
            return false;
        };
        if !matches!(self.items[i].span, Span::Left | Span::Right) {
            return false;
        }
        let (lo, hi) = self.run_bounds(i);
        let (left, right) = self.lanes_in(lo, hi);
        let cw = self.content().w;
        let stored = self.run_split(&left, &right);
        // mulai dari lebar SEBENARNYA (sudah termasuk batas app), bukan nilai tersimpan,
        // supaya menekan terus di batas tidak menumpuk
        let current = self.left_width(cw, stored, &left, &right);
        let target = (current + delta_px).clamp(cw * 15 / 100, cw * 85 / 100);
        let new_split = (target as i64 * 1000 / cw.max(1) as i64) as i32;
        if new_split == stored {
            return false;
        }
        // lajur yang menyempit diukur ulang: batas lamanya mungkin sudah tidak berlaku
        let shrinking = if delta_px > 0 { &right } else { &left };
        for &k in shrinking {
            self.min_widths.remove(&self.items[k].id);
        }
        for k in lo..hi {
            if self.items[k].span != Span::Full {
                self.items[k].split = new_split;
            }
        }
        true
    }

    /// Kembalikan pembagi blok window yang fokus ke 50/50.
    pub fn reset_split(&mut self) -> bool {
        let Some(i) = self.focused.and_then(|f| self.index_of(f)) else {
            return false;
        };
        if !matches!(self.items[i].span, Span::Left | Span::Right) {
            return false;
        }
        let (lo, hi) = self.run_bounds(i);
        let (left, right) = self.lanes_in(lo, hi);
        let mut changed = self.run_split(&left, &right) != 500;
        for &k in left.iter().chain(right.iter()) {
            changed |= self.min_widths.remove(&self.items[k].id).is_some();
        }
        for k in lo..hi {
            if self.items[k].span != Span::Full {
                self.items[k].split = 500;
            }
        }
        changed
    }

    /// Mode `step`: ubah tinggi window yang fokus sebesar `delta_px` (< 0 lebih
    /// pendek, > 0 lebih tinggi), pasangan `adjust_split`. Mulai dari tinggi
    /// SEBENARNYA (sudah termasuk batas app) supaya menekan terus di batas tidak
    /// menumpuk. Dijepit ke 15%-100% area konten. Kalau jatuh dalam setengah
    /// langkah dari 1/3, 1/2, atau penuh, dibulatkan ke preset itu (magnet),
    /// kecuali preset yang sedang dipakai. False kalau tidak ada yang berubah.
    pub fn adjust_height(&mut self, delta_px: i32) -> bool {
        let Some(id) = self.focused else {
            return false;
        };
        let Some(i) = self.index_of(id) else {
            return false;
        };
        let ch = self.content().h;
        let Some(current) = self.flow().iter().find(|f| f.id == id).map(|f| f.h) else {
            return false;
        };
        let target = (current + delta_px).clamp(ch * 15 / 100, ch);
        if target == current {
            return false; // sudah di batas atas / bawah
        }
        let old = self.items[i].height;
        let nearest = [Height::Third, Height::Half, Height::Full]
            .into_iter()
            .filter(|p| *p != old)
            .min_by_key(|p| (p.pixels(ch) - target).abs())
            .filter(|p| (p.pixels(ch) - target).abs() <= delta_px.abs() / 2);
        let new = nearest
            .unwrap_or_else(|| Height::Custom((target as i64 * 1000 / ch.max(1) as i64) as i32));
        if new == old {
            return false;
        }
        // tinggi diubah user -> ukur ulang minimum window ini dari nol
        self.min_heights.remove(&id);
        self.items[i].height = new;
        self.ensure_visible();
        true
    }

    /// Window baru: penuh lebar & tinggi, tepat setelah window yang fokus.
    pub fn add_window(&mut self, id: WindowId) {
        if self.index_of(id).is_some() {
            return;
        }
        // Di bawah SELURUH blok window yang fokus (bukan tepat setelahnya), supaya
        // window di lajur sebelah tidak ikut terdorong ke bawah window baru.
        let at = self
            .focused
            .and_then(|f| self.index_of(f))
            .map_or(self.items.len(), |f| {
                if self.items[f].span == Span::Full {
                    f + 1
                } else {
                    (f + 1..self.items.len())
                        .find(|&k| self.items[k].span == Span::Full)
                        .unwrap_or(self.items.len())
                }
            });
        self.items.insert(
            at,
            Item {
                id,
                span: Span::Full,
                height: Height::Full,
                restore: Span::Full,
                split: 500,
            },
        );
        self.focused = Some(id);
        self.ensure_visible();
    }

    /// Tempat window baru di lajur seberang item `i` (setengah lebar), sejajar dengannya:
    /// (indeks sisip, lebar). Urutan ke-n di lajurnya = sejajar; kalau lajur seberang
    /// lebih pendek, di ujung bawahnya.
    fn beside_slot(&self, i: usize) -> (usize, Span) {
        let t = self.items[i];
        let (lo, hi) = self.run_bounds(i);
        let (left, right) = self.lanes_in(lo, hi);
        let (mine, other) = if t.span == Span::Left {
            (&left, &right)
        } else {
            (&right, &left)
        };
        let rank = mine.iter().position(|&k| k == i).unwrap_or(0);
        let index = other.get(rank).copied().unwrap_or(hi);
        let span = if t.span == Span::Left {
            Span::Right
        } else {
            Span::Left
        };
        (index, span)
    }

    /// Sisipkan window baru `id` pada `at` (drag dari workspace lain di overview):
    ///  - `Beside(x)`: lajur seberang `x`, sejajar dengannya, tinggi dan pembagi sama
    ///    dengan `x`; hanya untuk `x` setengah lebar
    ///  - `FullBefore(x)` / `FullAfter(x)`: penuh lebar di batas blok `x` (tidak pernah
    ///    membelah blok), jadi pemisah blok baru
    ///  - `Split(x, first)`: membagi dua `x`. Penuh lebar -> berdampingan: `x` ke satu sisi,
    ///    window baru di sisi lain, tinggi sama. Setengah lebar -> bertumpuk di lajur yang
    ///    sama: tinggi `x` dan window baru masing-masing setengah dari tinggi `x`
    ///    (Full -> 1/2, 1/2 -> 1/4, ...). `first` = window baru di kiri / atas.
    /// False kalau `id` sudah ada, `x` tidak dikenal / tersembunyi, atau `Beside` pada
    /// window penuh lebar. Window baru menjadi yang fokus.
    pub fn insert_window(&mut self, id: WindowId, at: InsertPosition) -> bool {
        if self.index_of(id).is_some() {
            return false;
        }
        let target = match at {
            InsertPosition::Beside(t)
            | InsertPosition::FullBefore(t)
            | InsertPosition::FullAfter(t)
            | InsertPosition::Split(t, _) => t,
        };
        let Some(i) = self.index_of(target) else {
            return false;
        };
        let t = self.items[i];
        if t.span == Span::Hidden {
            return false;
        }
        let full = |id| Item {
            id,
            span: Span::Full,
            height: Height::Full,
            restore: Span::Full,
            split: 500,
        };
        let (index, item) = match at {
            InsertPosition::Beside(_) => {
                if t.span == Span::Full {
                    return false;
                }
                let (index, span) = self.beside_slot(i);
                (
                    index,
                    Item {
                        id,
                        span,
                        height: t.height,
                        restore: span,
                        split: t.split,
                    },
                )
            }
            InsertPosition::Split(_, first) => match t.span {
                Span::Full => {
                    // lebar dibagi dua: target ke satu sisi, window baru di sisi lainnya
                    let (mine, theirs) = if first {
                        (Span::Right, Span::Left)
                    } else {
                        (Span::Left, Span::Right)
                    };
                    self.items[i].span = mine;
                    self.items[i].restore = mine;
                    let (index, _) = self.beside_slot(i);
                    (
                        index,
                        Item {
                            id,
                            span: theirs,
                            height: t.height,
                            restore: theirs,
                            split: t.split,
                        },
                    )
                }
                _ => {
                    // tinggi dibagi dua, bertumpuk di lajur yang sama
                    let h = t.height.halved();
                    self.items[i].height = h;
                    (
                        if first { i } else { i + 1 },
                        Item {
                            id,
                            span: t.span,
                            height: h,
                            restore: t.span,
                            split: t.split,
                        },
                    )
                }
            },
            InsertPosition::FullBefore(_) => {
                let index = if t.span == Span::Full {
                    i
                } else {
                    self.run_bounds(i).0
                };
                (index, full(id))
            }
            InsertPosition::FullAfter(_) => {
                let index = if t.span == Span::Full {
                    i + 1
                } else {
                    self.run_bounds(i).1
                };
                (index, full(id))
            }
        };
        self.items.insert(index, item);
        self.focused = Some(id);
        self.ensure_visible();
        true
    }

    /// Pindahkan window `id` yang sudah ada ke `at` di strip yang sama.
    /// Kalau penempatan tidak berlaku, susunan awal dikembalikan.
    pub fn reinsert_window(&mut self, id: WindowId, at: InsertPosition) -> bool {
        let Some(idx) = self.index_of(id) else {
            return false;
        };
        let item = self.items.remove(idx);
        if self.insert_window(id, at) {
            true
        } else {
            self.items.insert(idx, item);
            false
        }
    }

    pub fn remove_window(&mut self, id: WindowId) {
        let Some(idx) = self.index_of(id) else { return };
        self.items.remove(idx);
        self.min_heights.remove(&id);
        self.min_widths.remove(&id);
        if self.focused == Some(id) {
            self.focused = self.pick_focus(idx);
        }
        self.ensure_visible();
    }

    /// Window terlihat terdekat dari posisi `idx` (maju dulu, lalu mundur).
    fn pick_focus(&self, idx: usize) -> Option<WindowId> {
        (idx..self.items.len())
            .find(|&k| self.items[k].span != Span::Hidden)
            .or_else(|| {
                (0..idx.min(self.items.len()))
                    .rev()
                    .find(|&k| self.items[k].span != Span::Hidden)
            })
            .map(|k| self.items[k].id)
    }

    /// Sembunyikan / tampilkan window tanpa kehilangan slotnya. Dipakai saat
    /// window di-minimize atau muncul lagi. True kalau ada perubahan.
    pub fn set_hidden(&mut self, id: WindowId, hide: bool) -> bool {
        let Some(i) = self.index_of(id) else {
            return false;
        };
        if hide {
            if self.items[i].span == Span::Hidden {
                return false;
            }
            self.items[i].restore = self.items[i].span;
            self.items[i].span = Span::Hidden;
            if self.focused == Some(id) {
                self.focused = self.pick_focus(i);
            }
        } else {
            if self.items[i].span != Span::Hidden {
                return false;
            }
            self.items[i].span = self.items[i].restore;
            if self.focused.is_none() {
                self.focused = Some(id);
            }
        }
        self.ensure_visible();
        true
    }

    pub fn set_focus(&mut self, id: WindowId) {
        if self
            .index_of(id)
            .map_or(false, |i| self.items[i].span != Span::Hidden)
        {
            self.focused = Some(id);
            self.ensure_visible();
        }
    }

    /// Snap window yang fokus.
    /// Up: tinggi makin pendek, Down: makin tinggi (per window).
    /// Left/Right: pindah ke lajur itu (lihat `snap_side`).
    pub fn snap(&mut self, dir: Dir) {
        let Some(id) = self.focused else { return };
        let Some(i) = self.index_of(id) else { return };
        match dir {
            // tinggi diubah user -> ukur ulang minimum window ini dari nol
            Dir::Up => {
                self.min_heights.remove(&id);
                self.items[i].height = self.items[i].height.shrink();
            }
            Dir::Down => {
                self.min_heights.remove(&id);
                self.items[i].height = self.items[i].height.grow();
            }
            Dir::Left => self.snap_side(i, Span::Left),
            Dir::Right => self.snap_side(i, Span::Right),
        }
        self.ensure_visible();
    }

    /// Jadikan window `i` penuh lebar. Window di sampingnya (yang mulai di atas
    /// dasar window ini) tetap di atas; window ini turun di bawahnya.
    fn make_full(&mut self, i: usize) {
        let flow = self.flow();
        let w_bottom = flow[i].bottom();
        let mut w = self.items[i];
        w.span = Span::Full;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (j, it) in self.items.iter().enumerate() {
            if j == i {
                continue;
            }
            if flow[j].top < w_bottom {
                before.push(*it);
            } else {
                after.push(*it);
            }
        }
        before.push(w);
        before.extend(after);
        self.items = before;
    }

    /// Left/Right pada window `i`.
    ///
    ///  - sudah di sisi itu            -> tidak ada yang terjadi
    ///  - di lajur seberang            -> jadi penuh lebar dulu (tinggi tetap);
    ///                                    tekan lagi untuk pindah ke sisi itu
    ///  - penuh lebar                  -> masuk lajur, dengan aturan penghuni (X)
    ///    yang terdekat di sekitarnya:
    ///      * seberang X kosong        -> SWAP: X pindah ke seberang, window ambil posisi X
    ///      * seberang X berisi window yang tidak lebih tinggi dari X
    ///        (baris kuarter sudah penuh) -> window masuk DI BAWAH X
    ///      * seberang X lebih tinggi dari X (mis. setengah layar)
    ///                                 -> window di ATAS X, X tergeser ke bawah
    ///      * tidak ada X              -> cukup pindah lajur
    fn snap_side(&mut self, i: usize, target: Span) {
        let current = self.items[i].span;
        if current == target {
            return;
        }
        let opposite = target.opposite();
        if current == opposite {
            self.make_full(i);
            return;
        }

        let flow = self.flow();
        let run_start = self.items[..i]
            .iter()
            .rposition(|it| it.span == Span::Full)
            .map_or(0, |p| p + 1);
        let run_end = self.items[i + 1..]
            .iter()
            .position(|it| it.span == Span::Full)
            .map_or(self.items.len(), |p| i + 1 + p);

        // masuk ke blok yang sudah punya pembagi: pakai rasio blok itu
        if let Some(r) = (run_start..run_end)
            .filter(|&k| k != i && matches!(self.items[k].span, Span::Left | Span::Right))
            .map(|k| self.items[k].split)
            .next()
        {
            self.items[i].split = r;
        }

        let x = (run_start..i)
            .rev()
            .find(|&j| self.items[j].span == target)
            .or_else(|| (i + 1..run_end).find(|&j| self.items[j].span == target));

        let Some(j) = x else {
            self.items[i].span = target;
            return;
        };

        let (tj, bj) = (flow[j].top, flow[j].bottom());
        let beside: Vec<Flow> = flow
            .iter()
            .enumerate()
            .filter(|(k, f)| {
                *k != i && *k != j && f.span == opposite && f.top < bj && f.bottom() > tj
            })
            .map(|(_, f)| *f)
            .collect();

        let above = if beside.is_empty() {
            self.items[j].span = opposite; // swap
            true
        } else {
            // baris kuarter penuh -> di bawah X; ada yang lebih tinggi -> di atas X
            !beside.iter().all(|f| f.top >= tj && f.bottom() <= bj)
        };

        self.items[i].span = target;
        let w = self.items.remove(i);
        let jj = if j > i { j - 1 } else { j };
        self.items.insert(if above { jj } else { jj + 1 }, w);
    }

    /// Kembalikan window yang fokus jadi penuh (lebar + tinggi), di bawah
    /// window-window yang ada di sampingnya, dan reset rasio lajurnya ke 1/2.
    pub fn maximize(&mut self) {
        let Some(id) = self.focused else { return };
        let Some(i) = self.index_of(id) else { return };
        self.make_full(i);
        if let Some(k) = self.index_of(id) {
            self.items[k].height = Height::Full;
            // keadaan awal: rasio lajur yang pernah diatur tidak ikut terbawa saat window
            // ini nanti masuk lajur lagi (Win+Kiri/Kanan jadi 1/2). Kalau ia masuk ke
            // blok yang sudah punya pembagi, `snap_side` tetap memakai rasio blok itu.
            self.items[k].split = 500;
        }
        self.ensure_visible();
    }

    /// Pindahkan fokus berdasarkan posisi sebenarnya di layar.
    pub fn focus(&mut self, dir: Dir) {
        if let Some(n) = self.neighbor(dir) {
            self.focused = Some(n);
            self.ensure_visible();
        }
    }

    /// Tukar POSISI window yang fokus dengan tetangga di arah `dir`; ukuran
    /// (lebar/tinggi) tetap milik masing-masing window.
    ///  - satu lajur, atau salah satunya penuh lebar -> tukar urutan
    ///  - beda lajur                                  -> tukar sisi
    pub fn move_window(&mut self, dir: Dir) {
        let Some(id) = self.focused else { return };
        let Some(n) = self.neighbor(dir) else { return };
        let (Some(a), Some(b)) = (self.index_of(id), self.index_of(n)) else {
            return;
        };
        let (sa, sb) = (self.items[a].span, self.items[b].span);
        if sa != Span::Full && sb != Span::Full && sa != sb {
            self.items[a].span = sb;
            self.items[b].span = sa;
        } else {
            self.items.swap(a, b);
        }
        // window penuh lebar yang ditukar tetap tampak di tempat yang sama (viewport
        // mengikuti fokus): beri tahu app supaya bisa memberi isyarat gerak
        self.cue = (sa == Span::Full).then_some((id, dir));
        self.ensure_visible();
    }

    /// Tukar posisi dua window tertentu (drag mouse). Aturannya sama dengan `move_window`:
    /// beda lajur -> tukar sisi, selain itu tukar urutan; ukuran tetap milik masing-masing
    /// window. Fokus tidak berubah. False kalau salah satu tidak dikenal / tersembunyi.
    pub fn swap_windows(&mut self, a: WindowId, b: WindowId) -> bool {
        if a == b {
            return false;
        }
        let (Some(i), Some(j)) = (self.index_of(a), self.index_of(b)) else {
            return false;
        };
        let (sa, sb) = (self.items[i].span, self.items[j].span);
        if sa == Span::Hidden || sb == Span::Hidden {
            return false;
        }
        if sa != Span::Full && sb != Span::Full && sa != sb {
            self.items[i].span = sb;
            self.items[j].span = sa;
        } else {
            self.items.swap(i, j);
        }
        self.ensure_visible();
        true
    }

    /// Ambil (dan hapus) isyarat tukar posisi terakhir: window penuh lebar yang baru
    /// dipindah dan arahnya. None kalau tukar terakhir sudah kelihatan di layar (lajur)
    /// atau tidak ada.
    pub fn take_move_cue(&mut self) -> Option<(WindowId, Dir)> {
        self.cue.take()
    }
}
