//! Snap layout: preset susunan beberapa zona (seperti Snap Layouts di Windows 11).
//! Murni logika, tanpa Win32.
//!
//! Sebuah preset adalah daftar zona berurutan, tiap zona = lebar (`Span`: kiri, kanan,
//! penuh) + tinggi. Urutannya sama dengan urutan item di strip, jadi preset selalu
//! bisa dinyatakan oleh engine yang ada (dua lajur + rasio pembagi + tinggi per window).
//!
//! Memasang preset ke strip (`Strip::apply_snap`):
//!  - window yang dijatuhkan mengisi satu zona; zona lain menjadi SLOT: item dengan id
//!    palsu (`WindowId::slot`) yang memesan ruangnya di layout. App tidak menggerakkan
//!    slot (id-nya tidak punya window), dan menampilkan daftar window di atasnya;
//!  - blok preset menggantikan susunan layar tempat window itu berada: window lain
//!    di susunan yang sama dijadikan penuh lebar di bawah blok (supaya tidak ikut
//!    tercampur ke lajur preset, dan tetap bisa dipilih untuk mengisi slot);
//!  - `fill_slot` mengisi slot dengan window yang sudah ada; `clear_slots` membuang
//!    slot yang tersisa (zona yang tidak diisi tetap kosong).

use crate::types::*;

/// Batas bawah id slot. Id window asli berasal dari HWND (32 bit bermakna), jadi
/// tidak pernah sebesar ini.
const SLOT_BASE: u64 = 1 << 60;

impl WindowId {
    /// Id palsu untuk zona ke-`n` dari preset yang belum terisi.
    pub fn slot(n: usize) -> WindowId {
        WindowId(SLOT_BASE + n as u64)
    }

    /// True kalau ini id slot (bukan window asli).
    pub fn is_slot(self) -> bool {
        self.0 >= SLOT_BASE
    }
}

/// Jumlah zona maksimum satu preset.
pub const MAX_ZONES: usize = 8;

/// Tinggi terkecil sebuah zona (permil), sama dengan batas `Height::Custom`.
const MIN_PERMIL: i32 = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapZone {
    /// `Full`, `Left` atau `Right`.
    pub span: Span,
    pub height: Height,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapPreset {
    pub name: String,
    /// Lebar lajur kiri dalam permil (500 = 50%), seperti `Item::split`.
    pub split: i32,
    /// Zona berurutan (urutan baca: atas ke bawah, kiri ke kanan).
    pub zones: Vec<SnapZone>,
}

/// Preset bawaan: (nama, spesifikasi dalam format yang sama dengan file konfigurasi).
pub const BUILTIN: &[(&str, &str)] = &[
    ("full", "F"),
    ("halves", "L, R"),
    ("wide-left", "split=67, L, R"),
    ("wide-right", "split=33, L, R"),
    ("main-stack", "L, R:1/2, R:1/2"),
    ("stack-main", "L:1/2, L:1/2, R"),
    ("quarters", "L:1/2, R:1/2, L:1/2, R:1/2"),
    ("top-split", "F:1/2, L:1/2, R:1/2"),
    ("rows", "F:1/2, F:1/2"),
    ("thirds", "F:1/3, F:1/3, F:1/3"),
];

/// Semua preset bawaan, urut seperti `BUILTIN`.
pub fn builtin_presets() -> Vec<SnapPreset> {
    BUILTIN
        .iter()
        .filter_map(|(name, spec)| SnapPreset::parse(name, spec).ok())
        .collect()
}

fn height_from_permil(p: i32) -> Height {
    match p {
        1000 => Height::Full,
        500 => Height::Half,
        333 => Height::Third,
        p => Height::Custom(p),
    }
}

/// "full", "1/2", "40%" atau "0.4" -> permil tinggi area konten.
fn parse_height(s: &str) -> Result<Height, String> {
    let s = s.trim();
    let bad = || format!("bad height '{s}' (use full, 1/2, 1/3, 2/3 or 40%)");
    let frac: f64 = if s == "full" {
        1.0
    } else if let Some(p) = s.strip_suffix('%') {
        p.trim().parse::<f64>().map_err(|_| bad())? / 100.0
    } else if let Some((a, b)) = s.split_once('/') {
        let a = a.trim().parse::<f64>().map_err(|_| bad())?;
        let b = b.trim().parse::<f64>().map_err(|_| bad())?;
        if b <= 0.0 {
            return Err(bad());
        }
        a / b
    } else {
        s.parse::<f64>().map_err(|_| bad())?
    };
    if !frac.is_finite() {
        return Err(bad());
    }
    let permil = (frac * 1000.0).round() as i32;
    if !(MIN_PERMIL..=1000).contains(&permil) {
        return Err(format!("height '{s}' must be between 15% and 100%"));
    }
    Ok(height_from_permil(permil))
}

/// "L", "r:1/2", "full:1/3" -> zona.
fn parse_zone(tok: &str) -> Result<SnapZone, String> {
    let (side, height) = match tok.split_once(':') {
        Some((s, h)) => (s.trim(), Some(h)),
        None => (tok.trim(), None),
    };
    let span = match side {
        "l" | "left" => Span::Left,
        "r" | "right" => Span::Right,
        "f" | "full" => Span::Full,
        other => {
            return Err(format!(
                "unknown zone '{other}' (use L, R or F, optionally with :height)"
            ))
        }
    };
    let height = match height {
        Some(h) => parse_height(h)?,
        None => Height::Full,
    };
    Ok(SnapZone { span, height })
}

/// "67" / "66.7%" -> permil lebar lajur kiri (15%-85%).
fn parse_split(v: &str) -> Result<i32, String> {
    let v = v.trim().trim_end_matches('%').trim();
    let pct: f64 = v
        .parse()
        .map_err(|_| format!("bad split '{v}' (use a percentage such as split=67)"))?;
    if !pct.is_finite() || !(15.0..=85.0).contains(&pct) {
        return Err(format!("split '{v}' must be between 15 and 85"));
    }
    Ok((pct * 10.0).round() as i32)
}

impl SnapPreset {
    /// Baca spesifikasi teks: daftar dipisah koma, urut seperti di strip.
    ///   `split=67`        lebar lajur kiri (persen, 15-85; bawaan 50)
    ///   `L` `R` `F`       zona di lajur kiri / kanan / penuh lebar
    ///   `L:1/2`           tinggi zona: full (bawaan), 1/2, 1/3, 2/3, 40%
    /// Contoh: `split=60, L, R:1/2, R:1/2`.
    pub fn parse(name: &str, spec: &str) -> Result<SnapPreset, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a layout needs a name".to_string());
        }
        let mut split = 500;
        let mut zones = Vec::new();
        for tok in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let lower = tok.to_lowercase();
            if let Some(rest) = lower.strip_prefix("split") {
                let value = rest
                    .trim_start()
                    .strip_prefix('=')
                    .ok_or_else(|| "expected split=NN".to_string())?;
                split = parse_split(value)?;
                continue;
            }
            zones.push(parse_zone(&lower)?);
        }
        if zones.is_empty() {
            return Err("no zones (write e.g. L, R)".to_string());
        }
        if zones.len() > MAX_ZONES {
            return Err(format!("at most {MAX_ZONES} zones per layout"));
        }
        Ok(SnapPreset {
            name: name.to_string(),
            split,
            zones,
        })
    }

    fn item(&self, id: WindowId, z: SnapZone) -> Item {
        Item {
            id,
            span: z.span,
            height: z.height,
            restore: z.span,
            split: self.split,
        }
    }

    /// Gambaran zona di kotak `w` x `h` (tanpa gap), dihitung oleh engine layout yang
    /// sama dengan yang dipakai saat dipasang sungguhan. Sebuah rect per zona, urut
    /// seperti `zones`. Kalau zona-zonanya lebih tinggi dari satu layar, gambarnya
    /// diperkecil supaya semuanya muat.
    pub fn preview(&self, w: i32, h: i32) -> Vec<Rect> {
        let (w, h) = (w.max(1), h.max(1));
        let mut s = Strip::new(Rect { x: 0, y: 0, w, h });
        s.items = self
            .zones
            .iter()
            .enumerate()
            .map(|(k, z)| self.item(WindowId::slot(k), *z))
            .collect();
        let flow = s.flow();
        let bottom = flow.iter().map(|f| f.bottom()).max().unwrap_or(0).max(1);
        let scale = |y: i32| {
            if bottom > h {
                (y as i64 * h as i64 / bottom as i64) as i32
            } else {
                y
            }
        };
        flow.iter()
            .map(|f| {
                let (y0, y1) = (scale(f.top), scale(f.bottom()));
                Rect {
                    x: f.x,
                    y: y0,
                    w: f.w,
                    h: (y1 - y0).max(1),
                }
            })
            .collect()
    }
}

impl Strip {
    /// Id slot yang masih ada di strip ini, urut menurut posisi.
    pub fn slots(&self) -> Vec<WindowId> {
        self.items
            .iter()
            .filter(|it| it.id.is_slot())
            .map(|it| it.id)
            .collect()
    }

    /// Buang semua slot (zona yang tidak diisi). True kalau ada yang dibuang.
    pub fn clear_slots(&mut self) -> bool {
        let before = self.items.len();
        self.items.retain(|it| !it.id.is_slot());
        if self.items.len() == before {
            return false;
        }
        if self.focused.is_some_and(|f| f.is_slot()) {
            self.focused = self
                .items
                .iter()
                .find(|it| it.span != Span::Hidden)
                .map(|it| it.id);
        }
        self.ensure_visible();
        true
    }

    /// Tempat sisip window baru: di bawah seluruh blok window yang fokus.
    fn after_focused(&self) -> usize {
        self.focused
            .and_then(|f| self.index_of(f))
            .map_or(self.items.len(), |f| {
                if self.items[f].span == Span::Full {
                    f + 1
                } else {
                    (f + 1..self.items.len())
                        .find(|&k| self.items[k].span == Span::Full)
                        .unwrap_or(self.items.len())
                }
            })
    }

    /// Pasang `preset` dengan `id` di zona `zone`; zona lainnya menjadi slot kosong.
    /// Mengembalikan id slot tersebut, urut menurut zona (kosong kalau `zone` di luar
    /// jangkauan atau `id` tidak valid).
    ///
    /// Blok preset menggantikan susunan layar tempat `id` berada: kalau `id` berbagi
    /// lajur dengan window lain, window-window itu dijadikan penuh lebar & tinggi tepat
    /// di bawah blok. Blok bisa menyatu dengan lajur di atas / bawahnya kalau yang
    /// bersebelahan bukan window penuh lebar; rasio pembagi preset berlaku untuk
    /// seluruh lajur itu (pembagi memang milik satu lajur). `id` menjadi fokus dan
    /// viewport menggulung ke awal blok.
    pub fn apply_snap(&mut self, id: WindowId, preset: &SnapPreset, zone: usize) -> Vec<WindowId> {
        if id.is_slot() || zone >= preset.zones.len() {
            return Vec::new();
        }
        self.clear_slots();
        let at = match self.index_of(id) {
            Some(i) => {
                // window penuh lebar berdiri sendiri; selain itu seluruh lajurnya (blok)
                let (lo, hi) = if self.items[i].span == Span::Full {
                    (i, i + 1)
                } else {
                    self.run_bounds(i)
                };
                let mut rest: Vec<Item> = self
                    .items
                    .drain(lo..hi)
                    .filter(|it| it.id != id)
                    .collect();
                for it in rest.iter_mut() {
                    if matches!(it.span, Span::Left | Span::Right) {
                        it.span = Span::Full;
                        it.restore = Span::Full;
                        it.height = Height::Full;
                        it.split = 500;
                    }
                }
                for (k, it) in rest.into_iter().enumerate() {
                    self.items.insert(lo + k, it);
                }
                lo
            }
            None => self.after_focused(),
        };
        self.min_heights.remove(&id);
        self.min_widths.remove(&id);

        let mut slots = Vec::new();
        for (k, z) in preset.zones.iter().enumerate() {
            let item_id = if k == zone {
                id
            } else {
                let s = WindowId::slot(k);
                slots.push(s);
                s
            };
            self.items.insert(at + k, preset.item(item_id, *z));
        }
        // pembagi dimiliki lajur: seluruh lajur yang memuat blok memakai rasio preset
        for k in 0..preset.zones.len() {
            let idx = at + k;
            if self.items[idx].span == Span::Full {
                continue;
            }
            let (lo, hi) = self.run_bounds(idx);
            for j in lo..hi {
                if self.items[j].span != Span::Full {
                    self.items[j].split = preset.split;
                }
            }
        }
        self.focused = Some(id);
        if let Some(top) = self.flow().get(at).map(|f| f.top) {
            self.scroll_y = top;
        }
        self.ensure_visible();
        slots
    }

    /// Isi slot `slot` dengan window `id` yang tidak ada di strip ini (pemanggil
    /// mengeluarkannya dulu dari tempat lamanya). `id` menjadi fokus. False kalau
    /// `slot` tidak ada atau `id` sudah ada di strip.
    pub fn fill_slot(&mut self, slot: WindowId, id: WindowId) -> bool {
        if !slot.is_slot() || id.is_slot() || self.index_of(id).is_some() {
            return false;
        }
        let Some(i) = self.index_of(slot) else {
            return false;
        };
        self.items[i].id = id;
        // ukuran zona beda dari tempat lamanya: ukur ulang batas minimum window ini
        self.min_heights.remove(&id);
        self.min_widths.remove(&id);
        self.focused = Some(id);
        self.ensure_visible();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::Workspaces;

    const A: WindowId = WindowId(1);
    const B: WindowId = WindowId(2);
    const C: WindowId = WindowId(3);
    const W: WindowId = WindowId(9);

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn strip() -> Strip {
        Strip::new(rect(0, 0, 1000, 600))
    }

    fn full(id: WindowId) -> Item {
        Item {
            id,
            span: Span::Full,
            height: Height::Full,
            restore: Span::Full,
            split: 500,
        }
    }

    fn lane(id: WindowId, span: Span) -> Item {
        Item {
            id,
            span,
            height: Height::Full,
            restore: span,
            split: 500,
        }
    }

    fn preset(spec: &str) -> SnapPreset {
        SnapPreset::parse("t", spec).unwrap()
    }

    fn placed(s: &Strip, id: WindowId) -> Rect {
        s.layout().iter().find(|p| p.id == id).unwrap().rect
    }

    #[test]
    fn slot_ids_are_distinct_from_window_handles() {
        assert!(WindowId::slot(0).is_slot());
        assert!(WindowId::slot(7).is_slot());
        assert!(!WindowId(0x0004_05A2).is_slot());
        assert_ne!(WindowId::slot(0), WindowId::slot(1));
    }

    #[test]
    fn parses_zones_heights_and_split() {
        let p = SnapPreset::parse("x", "split=67, L, R:1/2, F:1/3, l:2/3, r:40%").unwrap();
        assert_eq!(p.name, "x");
        assert_eq!(p.split, 670);
        let z: Vec<(Span, Height)> = p.zones.iter().map(|z| (z.span, z.height)).collect();
        assert_eq!(
            z,
            vec![
                (Span::Left, Height::Full),
                (Span::Right, Height::Half),
                (Span::Full, Height::Third),
                (Span::Left, Height::Custom(667)),
                (Span::Right, Height::Custom(400)),
            ]
        );
        assert_eq!(SnapPreset::parse("x", "left, right").unwrap().split, 500);
        assert_eq!(SnapPreset::parse("x", "split = 66.7%, L").unwrap().split, 667);
    }

    #[test]
    fn rejects_bad_specs() {
        assert!(SnapPreset::parse("x", "").is_err());
        assert!(SnapPreset::parse("x", "split=60").is_err()); // tanpa zona
        assert!(SnapPreset::parse("x", "Q").is_err());
        assert!(SnapPreset::parse("x", "L:1/0").is_err());
        assert!(SnapPreset::parse("x", "L:5%").is_err()); // di bawah 15%
        assert!(SnapPreset::parse("x", "L:3/2").is_err()); // di atas 100%
        assert!(SnapPreset::parse("x", "split=99, L").is_err());
        assert!(SnapPreset::parse("x", "split, L").is_err());
        assert!(SnapPreset::parse("", "L").is_err());
        assert!(SnapPreset::parse("x", "L,L,L,L,L,L,L,L,L").is_err()); // 9 zona
    }

    #[test]
    fn every_builtin_parses_and_names_are_unique() {
        let all = builtin_presets();
        assert_eq!(all.len(), BUILTIN.len());
        for (i, p) in all.iter().enumerate() {
            assert!(!p.zones.is_empty());
            assert!(all[i + 1..].iter().all(|q| q.name != p.name), "{}", p.name);
        }
    }

    #[test]
    fn preview_uses_the_real_layout_math() {
        let halves = preset("L, R").preview(1000, 600);
        assert_eq!(halves, vec![rect(0, 0, 500, 600), rect(500, 0, 500, 600)]);

        let quarters = preset("L:1/2, R:1/2, L:1/2, R:1/2").preview(1000, 600);
        assert_eq!(
            quarters,
            vec![
                rect(0, 0, 500, 300),
                rect(500, 0, 500, 300),
                rect(0, 300, 500, 300),
                rect(500, 300, 500, 300),
            ]
        );

        let wide = preset("split=67, L, R").preview(1000, 600);
        assert_eq!(wide, vec![rect(0, 0, 670, 600), rect(670, 0, 330, 600)]);

        let thirds = preset("F:1/3, F:1/3, F:1/3").preview(1000, 600);
        assert_eq!(
            thirds,
            vec![
                rect(0, 0, 1000, 200),
                rect(0, 200, 1000, 200),
                rect(0, 400, 1000, 200),
            ]
        );
    }

    #[test]
    fn preview_shrinks_blocks_taller_than_the_screen() {
        let rows = preset("F, F").preview(1000, 600);
        assert_eq!(rows, vec![rect(0, 0, 1000, 300), rect(0, 300, 1000, 300)]);
    }

    #[test]
    fn snapping_the_only_window_reserves_the_other_zones() {
        let mut s = strip();
        s.add_window(W);
        let slots = s.apply_snap(W, &preset("L, R"), 0);
        assert_eq!(slots, vec![WindowId::slot(1)]);
        assert_eq!(s.slots(), slots);
        assert_eq!(s.focused(), Some(W));
        assert_eq!(placed(&s, W), rect(0, 0, 500, 600));
        assert_eq!(placed(&s, WindowId::slot(1)), rect(500, 0, 500, 600));
    }

    #[test]
    fn the_block_replaces_the_dropped_window_between_full_neighbours() {
        let mut s = strip();
        s.items = vec![full(A), full(W), full(B)];
        s.focused = Some(W);
        let slots = s.apply_snap(W, &preset("L:1/2, R:1/2, L:1/2, R:1/2"), 1);
        assert_eq!(
            slots,
            vec![WindowId::slot(0), WindowId::slot(2), WindowId::slot(3)]
        );
        let ids: Vec<WindowId> = s.items().iter().map(|it| it.id).collect();
        assert_eq!(
            ids,
            vec![
                A,
                WindowId::slot(0),
                W,
                WindowId::slot(2),
                WindowId::slot(3),
                B
            ]
        );
        // viewport menggulung ke awal blok: W (kanan atas) ada di pojok kanan atas layar
        assert_eq!(placed(&s, W), rect(500, 0, 500, 300));
        assert_eq!(placed(&s, WindowId::slot(3)), rect(500, 300, 500, 300));
    }

    #[test]
    fn other_windows_of_the_old_screen_become_full_below_the_block() {
        let mut s = strip();
        s.items = vec![lane(A, Span::Left), lane(W, Span::Right), full(C)];
        s.focused = Some(W);
        s.apply_snap(W, &preset("L, R"), 1);
        let got: Vec<(WindowId, Span)> = s.items().iter().map(|it| (it.id, it.span)).collect();
        assert_eq!(
            got,
            vec![
                (WindowId::slot(0), Span::Left),
                (W, Span::Right),
                (A, Span::Full),
                (C, Span::Full),
            ]
        );
    }

    #[test]
    fn a_block_next_to_a_lane_run_takes_over_its_divider_ratio() {
        let mut s = strip();
        s.items = vec![lane(A, Span::Left), lane(B, Span::Right), full(W)];
        s.focused = Some(W);
        s.apply_snap(W, &preset("split=67, L, R"), 0);
        assert!(s
            .items()
            .iter()
            .filter(|it| it.span != Span::Full)
            .all(|it| it.split == 670));
        // baris kedua dari lajur yang menyatu: W di kiri bawah, sejajar slot kanan
        assert_eq!(placed(&s, W), rect(0, 0, 670, 600));
        assert_eq!(placed(&s, WindowId::slot(1)), rect(670, 0, 330, 600));
    }

    #[test]
    fn snapping_again_replaces_leftover_slots() {
        let mut s = strip();
        s.add_window(W);
        s.apply_snap(W, &preset("L, R"), 0);
        let slots = s.apply_snap(W, &preset("F:1/3, F:1/3, F:1/3"), 0);
        assert_eq!(slots.len(), 2);
        assert_eq!(s.items().len(), 3);
        assert!(s.items().iter().all(|it| it.span == Span::Full));
    }

    #[test]
    fn invalid_requests_change_nothing() {
        let mut s = strip();
        s.add_window(W);
        assert!(s.apply_snap(W, &preset("L, R"), 5).is_empty());
        assert!(s.apply_snap(WindowId::slot(0), &preset("L, R"), 0).is_empty());
        assert_eq!(s.items().len(), 1);
    }

    #[test]
    fn clear_slots_keeps_the_filled_zones() {
        let mut s = strip();
        s.add_window(W);
        s.apply_snap(W, &preset("L, R:1/2, R:1/2"), 0);
        assert_eq!(s.items().len(), 3);
        assert!(s.clear_slots());
        assert!(!s.clear_slots());
        assert_eq!(s.items().len(), 1);
        assert_eq!(s.items()[0].id, W);
        assert_eq!(s.items()[0].span, Span::Left);
    }

    #[test]
    fn filling_a_slot_from_the_same_workspace_moves_the_window() {
        let mut w = Workspaces::new(rect(0, 0, 1000, 600));
        w.add_window(A);
        w.add_window(W); // [A, W]
        let slots = w.apply_snap(W, &preset("L, R"), 0);
        assert_eq!(slots, vec![WindowId::slot(1)]);
        assert!(w.has_slots());
        // slot tidak ikut tersimpan ke layout.state
        let saved = w.snapshot();
        assert!(saved.strips[0].items.iter().all(|it| !it.id.is_slot()));
        assert!(w.fill_slot(WindowId::slot(1), A));
        assert!(!w.has_slots());
        let got: Vec<(WindowId, Span)> =
            w.cur().items().iter().map(|it| (it.id, it.span)).collect();
        assert_eq!(got, vec![(W, Span::Left), (A, Span::Right)]);
        assert_eq!(w.cur().focused(), Some(A));
    }

    #[test]
    fn filling_a_slot_pulls_a_window_from_another_workspace() {
        let mut w = Workspaces::new(rect(0, 0, 1000, 600));
        w.add_window(A);
        assert!(w.switch_to(1));
        w.add_window(W);
        assert_eq!(w.apply_snap(W, &preset("L, R"), 0).len(), 1);
        assert!(w.fill_slot(WindowId::slot(1), A));
        // workspace lama jadi kosong dan dihapus; keduanya kini di workspace aktif
        assert_eq!(w.workspace_of(A), Some(w.active()));
        assert_eq!(w.workspace_of(W), Some(w.active()));
        assert!(!w.has_slots());
    }

    #[test]
    fn filling_rejects_unknown_slots_and_unknown_windows() {
        let mut w = Workspaces::new(rect(0, 0, 1000, 600));
        w.add_window(W);
        w.apply_snap(W, &preset("L, R"), 0);
        assert!(!w.fill_slot(WindowId::slot(5), W)); // slot tidak ada
        assert!(!w.fill_slot(WindowId::slot(1), C)); // window tidak dikenal
        assert!(!w.fill_slot(WindowId::slot(1), WindowId::slot(0))); // slot bukan window
        assert!(w.has_slots());
        assert!(w.clear_slots());
        assert!(!w.has_slots());
    }

    #[test]
    fn snapping_a_window_of_another_workspace_is_ignored() {
        let mut w = Workspaces::new(rect(0, 0, 1000, 600));
        w.add_window(A);
        assert!(w.switch_to(1));
        w.add_window(B);
        assert!(w.apply_snap(A, &preset("L, R"), 0).is_empty());
        assert!(!w.has_slots());
    }
}
