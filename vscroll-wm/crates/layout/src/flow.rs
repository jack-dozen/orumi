//! Perhitungan posisi: aliran window per blok, scroll, dan query baca-saja.

use crate::types::*;

/// Posisi window di koordinat strip (belum dikurangi scroll).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Flow {
    pub(crate) id: WindowId,
    pub(crate) span: Span,
    /// Posisi dan lebar horizontal di area konten.
    pub(crate) x: i32,
    pub(crate) w: i32,
    pub(crate) top: i32,
    pub(crate) h: i32,
}

impl Flow {
    pub(crate) fn bottom(&self) -> i32 {
        self.top + self.h
    }
    pub(crate) fn x_range(&self) -> (i32, i32) {
        (self.x, self.x + self.w)
    }
}

impl Strip {
    /// Area efektif: work area dikurangi setengah gap di tiap sisi. Tiap window
    /// lalu dikecilkan setengah gap lagi, jadi jarak antar window = gap penuh.
    pub(crate) fn content(&self) -> Rect {
        let g = self.gap / 2;
        Rect {
            x: self.viewport.x + g,
            y: self.viewport.y + g,
            w: (self.viewport.w - 2 * g).max(1),
            h: (self.viewport.h - 2 * g).max(1),
        }
    }

    /// Batas blok (run) yang memuat posisi `i`: indeks pertama setelah window penuh
    /// sebelumnya sampai window penuh berikutnya.
    pub(crate) fn run_bounds(&self, i: usize) -> (usize, usize) {
        let lo = self.items[..i]
            .iter()
            .rposition(|it| it.span == Span::Full)
            .map_or(0, |p| p + 1);
        let hi = self.items[i + 1..]
            .iter()
            .position(|it| it.span == Span::Full)
            .map_or(self.items.len(), |p| i + 1 + p);
        (lo, hi)
    }

    /// Rasio pembagi (permil) sebuah blok: nilai item lajur pertama di blok itu.
    pub(crate) fn run_split(&self, left: &[usize], right: &[usize]) -> i32 {
        let first = match (left.first(), right.first()) {
            (Some(&a), Some(&b)) => Some(a.min(b)),
            (Some(&a), None) => Some(a),
            (None, Some(&b)) => Some(b),
            (None, None) => None,
        };
        first.map_or(500, |k| self.items[k].split)
    }

    /// Lebar lajur kiri (piksel konten) untuk satu blok. Dibatasi 15%-85%, lalu oleh
    /// lebar minimum window di tiap lajur yang berisi.
    pub(crate) fn left_width(&self, cw: i32, split: i32, left: &[usize], right: &[usize]) -> i32 {
        let gap = 2 * (self.gap / 2);
        let lane_min = |idxs: &[usize]| {
            idxs.iter()
                .filter_map(|&k| self.min_widths.get(&self.items[k].id))
                .max()
                .map_or(0, |m| m + gap)
        };
        let mut lw = (cw as i64 * split as i64 / 1000) as i32;
        lw = lw.clamp(cw * 15 / 100, cw * 85 / 100);
        if !left.is_empty() {
            lw = lw.max(lane_min(left));
        }
        if !right.is_empty() {
            lw = lw.min(cw - lane_min(right));
        }
        lw.clamp(0, cw)
    }

    /// Item lajur (kiri/kanan, tidak tersembunyi) dalam satu blok.
    pub(crate) fn lanes_in(&self, lo: usize, hi: usize) -> (Vec<usize>, Vec<usize>) {
        let left = (lo..hi)
            .filter(|&k| self.items[k].span == Span::Left)
            .collect();
        let right = (lo..hi)
            .filter(|&k| self.items[k].span == Span::Right)
            .collect();
        (left, right)
    }

    pub(crate) fn index_of(&self, id: WindowId) -> Option<usize> {
        self.items.iter().position(|it| it.id == id)
    }

    /// Tinggi window: target atau minimum aslinya, mana yang lebih besar.
    fn item_px(&self, it: &Item) -> i32 {
        // tinggi minimum adalah tinggi window; slot perlu ditambah gap
        let min = self
            .min_heights
            .get(&it.id)
            .map_or(0, |m| m + 2 * (self.gap / 2));
        it.height.pixels(self.content().h).max(min)
    }

    /// Susun semua window di koordinat strip.
    ///
    /// Window penuh lebar memisahkan strip jadi "run" (blok berisi dua lajur).
    /// Di tiap run, lihat batas-batas NOMINAL (tinggi target, tanpa tinggi
    /// minimum) di kedua lajur: kalau sebuah batas seharusnya sejajar tapi jadi
    /// tidak rata karena tinggi minimum sebuah app, window di lajur yang lebih
    /// pendek diregangkan supaya batas itu rata. Dengan begitu susunan seperti
    /// 2x2 kuarter tetap sejajar baris demi baris, dan gap seragam.
    pub(crate) fn flow(&self) -> Vec<Flow> {
        let content_h = self.content().h;
        let content_w = self.content().w;
        let dummy = Flow {
            id: WindowId(0),
            span: Span::Full,
            x: 0,
            w: 0,
            top: 0,
            h: 0,
        };
        let mut out = vec![dummy; self.items.len()];
        let mut base = 0i32;
        let (mut left, mut right): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        for (i, it) in self.items.iter().enumerate() {
            match it.span {
                Span::Left => left.push(i),
                Span::Right => right.push(i),
                Span::Hidden => {
                    out[i] = Flow {
                        id: it.id,
                        span: Span::Hidden,
                        x: 0,
                        w: 0,
                        top: base,
                        h: 0,
                    };
                }
                Span::Full => {
                    let top = self.place_run(&mut out, base, &left, &right, content_h, content_w);
                    left.clear();
                    right.clear();
                    let h = self.item_px(it);
                    out[i] = Flow {
                        id: it.id,
                        span: Span::Full,
                        x: 0,
                        w: content_w,
                        top,
                        h,
                    };
                    base = top + h;
                }
            }
        }
        self.place_run(&mut out, base, &left, &right, content_h, content_w);
        out
    }

    /// Tata satu run mulai dari `base`. Mengembalikan dasar run (yang terendah).
    fn place_run(
        &self,
        out: &mut [Flow],
        base: i32,
        left: &[usize],
        right: &[usize],
        content_h: i32,
        content_w: i32,
    ) -> i32 {
        let mut hl: Vec<i32> = left.iter().map(|&k| self.item_px(&self.items[k])).collect();
        let mut hr: Vec<i32> = right
            .iter()
            .map(|&k| self.item_px(&self.items[k]))
            .collect();

        let nominal = |k: usize| self.items[k].height.pixels(content_h);
        let cum = |idxs: &[usize]| -> Vec<i32> {
            let mut c = 0;
            idxs.iter()
                .map(|&k| {
                    c += nominal(k);
                    c
                })
                .collect()
        };
        let (cl, cr) = (cum(left), cum(right));

        let (mut i, mut j) = (0, 0);
        while i < cl.len() && j < cr.len() {
            let d = cl[i] - cr[j];
            if d.abs() <= 3 {
                let al: i32 = hl[..=i].iter().sum();
                let ar: i32 = hr[..=j].iter().sum();
                if al < ar {
                    hl[i] += ar - al;
                } else if ar < al {
                    hr[j] += al - ar;
                }
                i += 1;
                j += 1;
            } else if d < 0 {
                i += 1;
            } else {
                j += 1;
            }
        }

        let lw = self.left_width(content_w, self.run_split(left, right), left, right);
        let mut yl = base;
        for (n, &k) in left.iter().enumerate() {
            out[k] = Flow {
                id: self.items[k].id,
                span: Span::Left,
                x: 0,
                w: lw,
                top: yl,
                h: hl[n],
            };
            yl += hl[n];
        }
        let mut yr = base;
        for (n, &k) in right.iter().enumerate() {
            out[k] = Flow {
                id: self.items[k].id,
                span: Span::Right,
                x: lw,
                w: content_w - lw,
                top: yr,
                h: hr[n],
            };
            yr += hr[n];
        }
        yl.max(yr)
    }

    pub(crate) fn ensure_visible(&mut self) {
        let flow = self.flow();
        let vh = self.content().h;
        let content = flow.iter().map(|f| f.bottom()).max().unwrap_or(0);
        self.scroll_y = self.scroll_y.min((content - vh).max(0));

        let Some(f) = self.focused.and_then(|id| flow.iter().find(|f| f.id == id)) else {
            self.scroll_y = 0;
            return;
        };
        let (top, bottom) = (f.top, f.bottom());
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + vh {
            self.scroll_y = bottom - vh;
        }
        self.scroll_y = self.scroll_y.min(top).max(0);
    }

    /// Window tetangga terdekat di arah `dir`, berdasarkan posisi di layar.
    pub(crate) fn neighbor(&self, dir: Dir) -> Option<WindowId> {
        self.neighbor_of(self.focused?, dir)
    }

    /// Tetangga terdekat dari `id` di arah `dir`, tanpa mengubah fokus.
    pub fn neighbor_of(&self, id: WindowId, dir: Dir) -> Option<WindowId> {
        let flow = self.flow();
        let f = flow.iter().find(|c| c.id == id)?;
        let (fx0, fx1) = f.x_range();
        let (fy0, fy1) = (f.top, f.bottom());

        let mut best: Option<((bool, i32, i32, i32), WindowId)> = None;
        for c in flow.iter().filter(|c| c.id != id && c.span != Span::Hidden) {
            let (cx0, cx1) = c.x_range();
            let (cy0, cy1) = (c.top, c.bottom());
            let h_ov = (fx1.min(cx1) - fx0.max(cx0)).max(0);
            let v_ov = (fy1.min(cy1) - fy0.max(cy0)).max(0);
            let key = match dir {
                Dir::Up if cy1 <= fy0 => Some((h_ov == 0, fy0 - cy1, -h_ov, c.top)),
                Dir::Down if cy0 >= fy1 => Some((h_ov == 0, cy0 - fy1, -h_ov, c.top)),
                Dir::Left if cx1 <= fx0 && v_ov > 0 => Some((false, fx0 - cx1, -v_ov, c.top)),
                Dir::Right if cx0 >= fx1 && v_ov > 0 => Some((false, cx0 - fx1, -v_ov, c.top)),
                _ => None,
            };
            if let Some(k) = key {
                if best.map_or(true, |(bk, _)| k < bk) {
                    best = Some((k, c.id));
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// Seperti `neighbor_of`, tapi untuk saat tidak ada window lagi di arah `dir`:
    /// lompat ke ujung seberang strip (Atas di baris teratas -> baris terbawah, Kiri
    /// di tepi kiri -> tepi kanan, dst.), sebisanya di lajur / baris yang sama.
    /// Dipakai panah di overview supaya pilihan berputar. None kalau tidak ada window
    /// lain di sisi seberang, atau `id` tidak dikenal / tersembunyi.
    pub fn wrap_neighbor_of(&self, id: WindowId, dir: Dir) -> Option<WindowId> {
        let flow = self.flow();
        let f = flow.iter().find(|c| c.id == id && c.span != Span::Hidden)?;
        let (fx0, fx1) = f.x_range();
        let (fy0, fy1) = (f.top, f.bottom());
        let f_mid = (fy0 + fy1) / 2;
        let shown = || flow.iter().filter(|c| c.span != Span::Hidden);
        let first_top = shown().map(|c| c.top).min()?;
        let last_bottom = shown().map(|c| c.bottom()).max()?;
        let first_x = shown().map(|c| c.x_range().0).min()?;
        let last_x = shown().map(|c| c.x_range().1).max()?;

        let mut best: Option<((bool, i32, i32, i32), WindowId)> = None;
        for c in shown().filter(|c| c.id != id) {
            let (cx0, cx1) = c.x_range();
            let (cy0, cy1) = (c.top, c.bottom());
            let h_ov = (fx1.min(cx1) - fx0.max(cx0)).max(0);
            let v_ov = (fy1.min(cy1) - fy0.max(cy0)).max(0);
            let dy = ((cy0 + cy1) / 2 - f_mid).abs();
            // Hanya window di sisi "belakang" `id` yang jadi kandidat (Atas: yang ada di
            // bawahnya, dst.); yang terdekat ke ujung seberang menang.
            let key = match dir {
                Dir::Up if cy0 >= fy1 => Some((h_ov == 0, last_bottom - cy1, -h_ov, cy0)),
                Dir::Down if cy1 <= fy0 => Some((h_ov == 0, cy0 - first_top, -h_ov, cy0)),
                Dir::Left if cx0 >= fx1 => Some((v_ov == 0, last_x - cx1, dy, cy0)),
                Dir::Right if cx1 <= fx0 => Some((v_ov == 0, cx0 - first_x, dy, cy0)),
                _ => None,
            };
            if let Some(k) = key {
                if best.map_or(true, |(bk, _)| k < bk) {
                    best = Some((k, c.id));
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// Window berikutnya (atau sebelumnya) menurut urutan baca: atas ke bawah,
    /// kiri ke kanan, berputar. Dipakai untuk Tab di overview. None kalau hanya
    /// ada satu window atau `id` tidak dikenal / tersembunyi.
    pub fn cycle_from(&self, id: WindowId, forward: bool) -> Option<WindowId> {
        let mut order: Vec<(i32, i32, WindowId)> = self
            .flow()
            .iter()
            .filter(|f| f.span != Span::Hidden)
            .map(|f| (f.top, f.x_range().0, f.id))
            .collect();
        order.sort_by_key(|&(top, x, _)| (top, x));
        let n = order.len();
        let pos = order.iter().position(|&(_, _, w)| w == id)?;
        if n < 2 {
            return None;
        }
        let next = if forward {
            (pos + 1) % n
        } else {
            (pos + n - 1) % n
        };
        Some(order[next].2)
    }

    /// Peta seluruh strip (termasuk bagian di luar layar) untuk overview.
    pub fn overview(&self) -> Overview {
        let c = self.content();
        let items: Vec<OverviewItem> = self
            .flow()
            .iter()
            .filter(|f| f.span != Span::Hidden)
            .map(|f| {
                let (x0, x1) = f.x_range();
                OverviewItem {
                    id: f.id,
                    rect: Rect {
                        x: x0,
                        y: f.top,
                        w: x1 - x0,
                        h: f.h,
                    },
                }
            })
            .collect();
        let height = items
            .iter()
            .map(|i| i.rect.y + i.rect.h)
            .max()
            .unwrap_or(0)
            .max(c.h);
        Overview {
            items,
            width: c.w,
            height,
            view_y: self.scroll_y,
            view_h: c.h,
        }
    }

    pub fn layout(&self) -> Vec<Placement> {
        let vp = self.viewport;
        let c = self.content();
        let g = self.gap / 2;
        self.flow()
            .into_iter()
            .filter(|f| f.span != Span::Hidden)
            .map(|f| {
                let top = c.y + f.top - self.scroll_y;
                let (x, w) = (c.x + f.x, f.w);
                Placement {
                    id: f.id,
                    rect: Rect {
                        x: x + g,
                        y: top + g,
                        w: (w - 2 * g).max(1),
                        h: (f.h - 2 * g).max(1),
                    },
                    visible: top + f.h > vp.y && top < vp.y + vp.h,
                }
            })
            .collect()
    }
}
