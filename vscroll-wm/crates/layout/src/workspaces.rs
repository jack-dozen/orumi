//! Beberapa strip (workspace) ala GNOME. Murni logika, tanpa Win32.
//!
//! Aturan: selalu ada tepat satu workspace kosong di ujung kanan. Workspace kosong
//! di tengah dihapus begitu ditinggalkan (nomor di kanannya bergeser). Workspace
//! aktif boleh kosong selama masih ditempati. "Kosong" berarti tidak ada item di
//! strip; window yang di-minimize tetap dihitung sebagai isi.
//!
//! Tambah/hapus window dan pindah workspace HARUS lewat method di sini (bukan
//! `cur_mut().add_window()` dsb.), supaya aturan di atas selalu dirapikan.

use crate::types::*;

/// Susunan satu workspace yang disimpan: item berurutan (lebar, tinggi, pembagi) dan
/// window yang fokus.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedStrip {
    pub items: Vec<Item>,
    pub focused: Option<WindowId>,
}

/// Susunan seluruh workspace yang disimpan, dan nomor workspace yang aktif.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedLayout {
    pub active: usize,
    pub strips: Vec<SavedStrip>,
}

pub struct Workspaces {
    strips: Vec<Strip>,
    active: usize,
    viewport: Rect,
    gap: i32,
}

impl Workspaces {
    pub fn new(viewport: Rect) -> Self {
        Self {
            strips: vec![Strip::new(viewport)],
            active: 0,
            viewport,
            gap: 0,
        }
    }

    pub fn count(&self) -> usize {
        self.strips.len()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn cur(&self) -> &Strip {
        &self.strips[self.active]
    }

    /// Untuk operasi yang tidak mengubah isi (snap, fokus, geser, ...).
    pub fn cur_mut(&mut self) -> &mut Strip {
        &mut self.strips[self.active]
    }

    pub fn strip(&self, n: usize) -> Option<&Strip> {
        self.strips.get(n)
    }

    pub fn set_viewport(&mut self, viewport: Rect) {
        self.viewport = viewport;
        for s in &mut self.strips {
            s.set_viewport(viewport);
        }
    }

    pub fn set_gap(&mut self, gap: i32) {
        self.gap = gap.max(0);
        for s in &mut self.strips {
            s.set_gap(gap);
        }
    }

    /// Hapus workspace kosong selain yang aktif, lalu pastikan yang terakhir kosong.
    fn normalize(&mut self) {
        let old = std::mem::take(&mut self.strips);
        let mut new_active = 0;
        for (k, s) in old.into_iter().enumerate() {
            if k == self.active {
                new_active = self.strips.len();
                self.strips.push(s);
            } else if !s.items().is_empty() {
                self.strips.push(s);
            }
        }
        self.active = new_active;
        if self.strips.last().map_or(true, |s| !s.items().is_empty()) {
            let mut s = Strip::new(self.viewport);
            s.set_gap(self.gap);
            self.strips.push(s);
        }
    }

    /// Salinan susunan sekarang, untuk disimpan ke file.
    pub fn snapshot(&self) -> SavedLayout {
        SavedLayout {
            active: self.active,
            strips: self
                .strips
                .iter()
                .map(|s| SavedStrip {
                    items: s.items().to_vec(),
                    focused: s.focused(),
                })
                .collect(),
        }
    }

    /// Pulihkan susunan tersimpan untuk window yang masih ada (`live`). Hanya berlaku
    /// selama semua workspace masih kosong (saat start). Item yang tidak `live` dibuang
    /// (workspace yang jadi kosong ikut hilang); window yang tadinya tersembunyi
    /// (minimize) kembali ke lebar semula.
    pub fn restore(&mut self, saved: &SavedLayout, live: impl Fn(WindowId) -> bool) {
        if self.strips.iter().any(|s| !s.items().is_empty()) {
            return;
        }
        let mut strips: Vec<Strip> = Vec::new();
        let mut active = 0;
        let mut seen: Vec<WindowId> = Vec::new();
        for (k, ss) in saved.strips.iter().enumerate() {
            let mut s = Strip::new(self.viewport);
            s.set_gap(self.gap);
            for it in &ss.items {
                if seen.contains(&it.id) || !live(it.id) {
                    continue;
                }
                seen.push(it.id);
                let mut item = *it;
                if item.span == Span::Hidden {
                    item.span = if item.restore == Span::Hidden {
                        Span::Full
                    } else {
                        item.restore
                    };
                }
                s.items.push(item);
            }
            if s.items.is_empty() {
                continue;
            }
            s.focused = ss
                .focused
                .filter(|f| s.index_of(*f).is_some())
                .or_else(|| s.items.first().map(|it| it.id));
            s.ensure_visible();
            if k == saved.active {
                active = strips.len();
            }
            strips.push(s);
        }
        if strips.is_empty() {
            return;
        }
        self.strips = strips;
        self.active = active;
        self.normalize();
    }

    /// Nomor workspace yang memuat `id`.
    pub fn workspace_of(&self, id: WindowId) -> Option<usize> {
        self.strips
            .iter()
            .position(|s| s.items().iter().any(|it| it.id == id))
    }

    /// Semua window (termasuk yang tersembunyi) di workspace `n`.
    pub fn window_ids(&self, n: usize) -> Vec<WindowId> {
        self.strips
            .get(n)
            .map_or_else(Vec::new, |s| s.items().iter().map(|it| it.id).collect())
    }

    /// Window baru masuk ke workspace aktif.
    pub fn add_window(&mut self, id: WindowId) {
        if self.workspace_of(id).is_some() {
            return;
        }
        self.cur_mut().add_window(id);
        self.normalize();
    }

    /// Hapus window dari workspace mana pun.
    pub fn remove_window(&mut self, id: WindowId) {
        if let Some(n) = self.workspace_of(id) {
            self.strips[n].remove_window(id);
            self.normalize();
        }
    }

    /// Minimize / cloaked: slot diingat di workspace pemiliknya.
    pub fn set_hidden(&mut self, id: WindowId, hide: bool) -> bool {
        match self.workspace_of(id) {
            Some(n) => self.strips[n].set_hidden(id, hide),
            None => false,
        }
    }

    /// Pindah ke workspace `n`. False kalau `n` di luar jangkauan atau sudah aktif.
    /// Nomor workspace lain bisa bergeser kalau yang ditinggalkan kosong.
    pub fn switch_to(&mut self, n: usize) -> bool {
        if n == self.active || n >= self.strips.len() {
            return false;
        }
        self.active = n;
        self.normalize();
        true
    }

    /// Workspace sebelumnya (`delta` < 0) / berikutnya (> 0). Tidak berputar.
    pub fn switch_rel(&mut self, delta: i32) -> bool {
        let n = self.active as i64 + delta as i64;
        usize::try_from(n).map_or(false, |n| self.switch_to(n))
    }

    /// Kirim window yang fokus ke workspace `n` (boleh workspace kosong di ujung).
    /// Pengguna tetap di workspace aktif; fokus pindah ke tetangga window itu.
    /// Window masuk sebagai penuh lebar & tinggi di tujuan; ukuran minimum yang
    /// sudah terukur ikut dibawa.
    pub fn move_focused_to(&mut self, n: usize) -> Option<WindowId> {
        let a = self.active;
        if n == a || n >= self.strips.len() {
            return None;
        }
        let id = self.strips[a].focused()?;
        let min_h = self.strips[a].min_heights.get(&id).copied();
        let min_w = self.strips[a].min_widths.get(&id).copied();
        self.strips[a].remove_window(id);
        let dst = &mut self.strips[n];
        dst.add_window(id);
        if let Some(h) = min_h {
            dst.min_heights.insert(id, h);
        }
        if let Some(w) = min_w {
            dst.min_widths.insert(id, w);
        }
        dst.ensure_visible();
        self.normalize();
        Some(id)
    }

    /// Pindahkan window `id` (bukan hanya yang fokus) ke workspace `n`; workspace aktif
    /// tidak berubah. Aturannya sama dengan `move_focused_to`. False kalau `id` tidak
    /// dikenal, sudah di `n`, atau `n` di luar jangkauan.
    pub fn move_window_to(&mut self, id: WindowId, n: usize) -> bool {
        self.move_window_with(id, n, None)
    }

    /// Seperti `move_window_to`, tapi window disisipkan pada `at` di workspace tujuan
    /// (lihat `Strip::insert_window`); kalau `at` tidak berlaku, masuk seperti biasa.
    pub fn move_window_at(&mut self, id: WindowId, n: usize, at: InsertPosition) -> bool {
        self.move_window_with(id, n, Some(at))
    }

    fn move_window_with(&mut self, id: WindowId, n: usize, at: Option<InsertPosition>) -> bool {
        let Some(from) = self.workspace_of(id) else {
            return false;
        };
        if from == n || n >= self.strips.len() {
            return false;
        }
        let min_h = self.strips[from].min_heights.get(&id).copied();
        let min_w = self.strips[from].min_widths.get(&id).copied();
        self.strips[from].remove_window(id);
        let dst = &mut self.strips[n];
        if !at.is_some_and(|p| dst.insert_window(id, p)) {
            dst.add_window(id);
        }
        if let Some(h) = min_h {
            dst.min_heights.insert(id, h);
        }
        if let Some(w) = min_w {
            dst.min_widths.insert(id, w);
        }
        dst.ensure_visible();
        self.normalize();
        true
    }

    /// Pindahkan window ke tempat `at` di workspace-nya sendiri (drag ke ruang kosong).
    pub fn reposition_window(&mut self, id: WindowId, at: InsertPosition) -> bool {
        let Some(n) = self.workspace_of(id) else {
            return false;
        };
        let ok = self.strips[n].reinsert_window(id, at);
        if ok {
            self.normalize();
        }
        ok
    }

    /// Tukar posisi dua window yang berada di workspace yang sama (drag mouse).
    pub fn swap_windows(&mut self, a: WindowId, b: WindowId) -> bool {
        match (self.workspace_of(a), self.workspace_of(b)) {
            (Some(x), Some(y)) if x == y => self.strips[x].swap_windows(a, b),
            _ => false,
        }
    }

    /// Seperti `move_focused_to`, ke workspace sebelumnya / berikutnya (tidak berputar).
    pub fn move_focused_rel(&mut self, delta: i32) -> Option<WindowId> {
        let n = self.active as i64 + delta as i64;
        usize::try_from(n).ok().and_then(|n| self.move_focused_to(n))
    }

    /// Peta overview tiap workspace yang punya window terlihat, urut dari kiri, berikut
    /// nomornya (0 = pertama). Workspace kosong / hanya berisi window tersembunyi dilewati.
    pub fn overviews(&self) -> Vec<(usize, Overview)> {
        self.strips
            .iter()
            .enumerate()
            .map(|(n, s)| (n, s.overview()))
            .filter(|(_, o)| !o.items.is_empty())
            .collect()
    }

    /// Workspace terdekat dari `from` ke arah `forward` yang punya window terlihat.
    pub fn adjacent_workspace(&self, from: usize, forward: bool) -> Option<usize> {
        let has = |n: usize| {
            self.strips
                .get(n)
                .is_some_and(|s| !s.overview().items.is_empty())
        };
        if forward {
            (from + 1..self.strips.len()).find(|&n| has(n))
        } else {
            (0..from.min(self.strips.len())).rev().find(|&n| has(n))
        }
    }

    /// Seperti `adjacent_workspace`, tapi berputar: dari ujung terakhir (`forward`) lanjut
    /// ke workspace pertama yang punya window terlihat, dan sebaliknya. None kalau tidak
    /// ada workspace lain yang punya window terlihat.
    pub fn wrapped_workspace(&self, from: usize, forward: bool) -> Option<usize> {
        let has = |n: usize| {
            self.strips
                .get(n)
                .is_some_and(|s| !s.overview().items.is_empty())
        };
        let target = if forward {
            (0..self.strips.len()).find(|&n| has(n))
        } else {
            (0..self.strips.len()).rev().find(|&n| has(n))
        };
        target.filter(|&n| n != from)
    }

    /// Window wakil workspace `n`: yang fokus, atau yang pertama terlihat. None kalau
    /// workspace itu tidak ada / tidak punya window terlihat.
    pub fn representative(&self, n: usize) -> Option<WindowId> {
        let s = self.strips.get(n)?;
        let ov = s.overview();
        s.focused()
            .filter(|f| ov.items.iter().any(|it| it.id == *f))
            .or_else(|| ov.items.first().map(|it| it.id))
    }

    /// Window tempat kursor masuk saat menyeberang ke workspace `n` dari samping.
    /// `from_left`: masuk dari tepi kiri (menekan Kanan). Dipilih window di tepi itu
    /// yang paling dekat secara vertikal dengan `y` (koordinat strip).
    pub fn entry_window(&self, n: usize, from_left: bool, y: i32) -> Option<WindowId> {
        const SLACK: i32 = 2; // lebar lajur dibulatkan: toleransi beberapa piksel
        let ov = self.strips.get(n)?.overview();
        let edge = if from_left {
            ov.items.iter().map(|i| i.rect.x).min()?
        } else {
            ov.items.iter().map(|i| i.rect.x + i.rect.w).max()?
        };
        ov.items
            .iter()
            .filter(|i| {
                if from_left {
                    i.rect.x - edge <= SLACK
                } else {
                    edge - (i.rect.x + i.rect.w) <= SLACK
                }
            })
            .min_by_key(|i| ((i.rect.y + i.rect.h / 2 - y).abs(), i.rect.y))
            .map(|i| i.id)
    }

    /// Fokuskan `id` di workspace pemiliknya; kalau bukan yang aktif, pindah ke sana.
    /// True kalau workspace aktif berubah.
    pub fn focus_window(&mut self, id: WindowId) -> bool {
        let Some(n) = self.workspace_of(id) else {
            return false;
        };
        let switched = self.switch_to(n);
        self.cur_mut().set_focus(id);
        switched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: WindowId = WindowId(1);
    const B: WindowId = WindowId(2);
    const C: WindowId = WindowId(3);
    const X: WindowId = WindowId(4);

    fn rect(w: i32, h: i32) -> Rect {
        Rect { x: 0, y: 0, w, h }
    }

    fn ws() -> Workspaces {
        Workspaces::new(rect(1920, 1080))
    }

    #[test]
    fn starts_with_a_single_empty_workspace() {
        let w = ws();
        assert_eq!((w.count(), w.active()), (1, 0));
        assert!(w.cur().items().is_empty());
    }

    #[test]
    fn first_window_opens_an_empty_workspace_next_to_it() {
        let mut w = ws();
        w.add_window(A);
        assert_eq!((w.count(), w.active()), (2, 0));
        assert_eq!(w.workspace_of(A), Some(0));
        assert!(w.strip(1).unwrap().items().is_empty());
    }

    #[test]
    fn workspaces_keep_growing_to_the_right() {
        let mut w = ws();
        w.add_window(A);
        assert!(w.switch_to(1));
        w.add_window(B);
        assert_eq!(w.count(), 3);
        assert!(w.switch_to(2));
        w.add_window(C);
        assert_eq!(w.count(), 4);
        assert_eq!(
            (w.workspace_of(A), w.workspace_of(B), w.workspace_of(C)),
            (Some(0), Some(1), Some(2))
        );
    }

    #[test]
    fn empty_workspace_in_the_middle_goes_away_when_left() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B); // [A] [B] []
        w.remove_window(B);
        // yang aktif boleh kosong selama masih ditempati; dia jadi kosong di ujung
        assert_eq!((w.count(), w.active()), (2, 1));
        assert!(w.switch_to(0));
        assert_eq!((w.count(), w.active()), (2, 0)); // [A] []
    }

    #[test]
    fn removing_in_an_inactive_workspace_shifts_the_active_index() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B); // aktif = 1
        w.remove_window(A); // workspace 0 kosong -> dihapus
        assert_eq!((w.count(), w.active()), (2, 0));
        assert_eq!(w.workspace_of(B), Some(0));
    }

    #[test]
    fn moving_does_not_wrap_and_stops_at_the_ends() {
        let mut w = ws();
        w.add_window(A);
        assert!(!w.switch_rel(-1));
        assert!(w.switch_rel(1)); // ke workspace kosong di ujung
        assert_eq!((w.count(), w.active()), (2, 1));
        assert!(!w.switch_rel(1));
        assert!(w.switch_rel(-1));
        assert!(!w.switch_to(0)); // sudah aktif
        assert!(!w.switch_to(9));
    }

    #[test]
    fn leaving_the_trailing_empty_workspace_keeps_exactly_one_at_the_end() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        assert!(w.switch_to(0));
        assert_eq!(w.count(), 2); // [A] []
        assert!(w.strip(1).unwrap().items().is_empty());
    }

    #[test]
    fn hidden_windows_still_count_as_content() {
        let mut w = ws();
        w.add_window(A);
        assert!(w.set_hidden(A, true));
        w.switch_to(1);
        assert_eq!(w.workspace_of(A), Some(0)); // workspace 0 tidak dihapus
        assert_eq!(w.count(), 2);
    }

    #[test]
    fn move_focused_sends_the_window_and_stays_put() {
        let mut w = ws();
        w.add_window(A);
        w.add_window(B); // B fokus
        assert_eq!(w.move_focused_to(1), Some(B));
        assert_eq!(w.active(), 0);
        assert_eq!(w.cur().focused(), Some(A));
        assert_eq!((w.workspace_of(A), w.workspace_of(B)), (Some(0), Some(1)));
        assert_eq!(w.count(), 3); // [A] [B] []
        assert_eq!(w.move_focused_to(0), None); // tujuan = workspace aktif
        assert_eq!(w.move_focused_to(7), None);
    }

    #[test]
    fn moving_the_last_window_leaves_an_empty_active_workspace_until_you_leave() {
        let mut w = ws();
        w.add_window(A);
        assert_eq!(w.move_focused_to(1), Some(A));
        assert_eq!(w.count(), 3); // [] [A] []
        assert!(w.cur().items().is_empty());
        assert!(w.switch_to(1));
        assert_eq!((w.count(), w.active()), (2, 0)); // [A] []
        assert_eq!(w.workspace_of(A), Some(0));
    }

    #[test]
    fn move_focused_rel_follows_neighbours_and_has_no_wrap() {
        let mut w = ws();
        w.add_window(A);
        assert_eq!(w.move_focused_rel(-1), None); // sudah paling kiri
        w.switch_to(1);
        w.add_window(B); // [A] [B] []
        assert_eq!(w.move_focused_rel(-1), Some(B));
        assert_eq!(w.workspace_of(B), Some(0));
        assert_eq!(w.move_focused_rel(1), None); // workspace aktif kini kosong
    }

    #[test]
    fn move_carries_measured_minimums() {
        let mut w = ws();
        w.add_window(A);
        w.add_window(B);
        w.cur_mut().set_min_height(B, 700);
        w.cur_mut().set_min_width(B, 500);
        assert_eq!(w.move_focused_to(1), Some(B));
        assert_eq!(w.strips[1].min_heights.get(&B), Some(&700));
        assert_eq!(w.strips[1].min_widths.get(&B), Some(&500));
        assert_eq!(w.strips[0].min_heights.get(&B), None);
    }

    #[test]
    fn focus_window_switches_to_the_owning_workspace() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B); // [A] [B] []
        assert!(w.focus_window(A));
        assert_eq!(w.active(), 0);
        assert_eq!(w.cur().focused(), Some(A));
        assert!(!w.focus_window(A)); // sudah di sana
        assert!(!w.focus_window(WindowId(99)));
    }

    #[test]
    fn viewport_and_gap_apply_to_every_strip_including_new_ones() {
        let mut w = ws();
        w.set_gap(20);
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B);
        let r = w.cur().layout()[0].rect;
        assert_eq!((r.x, r.w), (20, 1880));
        w.set_viewport(rect(1280, 720));
        w.switch_to(2); // workspace kosong di ujung
        w.add_window(C);
        assert_eq!(w.cur().layout()[0].rect.w, 1240);
        w.switch_to(1);
        assert_eq!(w.cur().layout()[0].rect.w, 1240);
    }

    #[test]
    fn overviews_list_only_workspaces_with_visible_windows() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B); // [A] [B] []
        let idx: Vec<usize> = w.overviews().iter().map(|(n, _)| *n).collect();
        assert_eq!(idx, vec![0, 1]);
        w.set_hidden(B, true); // hanya window tersembunyi -> tidak ada yang bisa dipilih
        let idx: Vec<usize> = w.overviews().iter().map(|(n, _)| *n).collect();
        assert_eq!(idx, vec![0]);
    }

    #[test]
    fn adjacent_workspace_skips_empty_ones_and_stops_at_the_ends() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B);
        w.switch_to(2);
        w.add_window(C); // [A] [B] [C] []
        assert_eq!(w.adjacent_workspace(0, true), Some(1));
        assert_eq!(w.adjacent_workspace(2, true), None); // yang kosong di ujung dilewati
        assert_eq!(w.adjacent_workspace(0, false), None);
        assert_eq!(w.adjacent_workspace(2, false), Some(1));
        // workspace aktif kosong di tengah: tetap bisa menyeberang
        w.switch_to(1);
        w.remove_window(B); // [A] [] [C] ...
        assert_eq!(w.adjacent_workspace(1, true), Some(2));
        assert_eq!(w.adjacent_workspace(1, false), Some(0));
    }

    #[test]
    fn representative_prefers_the_focused_window() {
        let mut w = ws();
        w.add_window(A);
        w.add_window(B); // B fokus
        assert_eq!(w.representative(0), Some(B));
        w.cur_mut().set_focus(A);
        assert_eq!(w.representative(0), Some(A));
        assert_eq!(w.representative(1), None); // kosong di ujung
        assert_eq!(w.representative(9), None);
    }

    #[test]
    fn entry_window_picks_the_edge_window_closest_in_height() {
        let mut w = ws();
        w.add_window(A);
        w.cur_mut().snap(Dir::Left);
        w.add_window(B);
        w.cur_mut().snap(Dir::Right); // [A kiri] [B kanan]
        assert_eq!(w.entry_window(0, true, 500), Some(A)); // masuk dari kiri
        assert_eq!(w.entry_window(0, false, 500), Some(B)); // masuk dari kanan
        assert_eq!(w.entry_window(5, true, 0), None); // workspace tidak ada
        assert_eq!(w.entry_window(1, true, 0), None); // kosong di ujung

        // dua window lebar penuh bertumpuk: yang setinggi kursor menang
        let mut w = ws();
        w.add_window(A);
        w.cur_mut().snap(Dir::Up); // setengah tinggi: pusat y = 270
        w.add_window(B); // di bawahnya: pusat y = 1080
        assert_eq!(w.entry_window(0, true, 100), Some(A));
        assert_eq!(w.entry_window(0, false, 1200), Some(B));
    }

    #[test]
    fn move_window_to_works_for_any_window_and_keeps_the_active_workspace() {
        let mut w = ws();
        w.add_window(A);
        w.add_window(B);
        w.switch_to(1);
        w.add_window(C); // [A B] [C] []
        assert!(w.move_window_to(A, 1)); // bukan window yang fokus
        assert_eq!(w.workspace_of(A), Some(1));
        assert_eq!(w.workspace_of(B), Some(0));
        assert_eq!(w.active(), 1);
        assert!(!w.move_window_to(A, 1)); // sudah di sana
        assert!(!w.move_window_to(A, 99)); // di luar jangkauan
        assert!(!w.move_window_to(X, 0)); // tidak dikenal
        // boleh ke workspace kosong di ujung; yang lama yang kosong dibuang
        let last = w.count() - 1;
        assert!(w.move_window_to(B, last));
        assert!(w.workspace_of(B).is_some());
        assert_ne!(w.workspace_of(B), w.workspace_of(C)); // workspace sendiri
    }

    #[test]
    fn move_window_at_places_the_window_in_the_target_block() {
        let mut w = ws();
        w.add_window(A);
        w.cur_mut().snap(Dir::Left);
        w.switch_to(1);
        w.add_window(X); // [A] [X] []
        assert!(w.move_window_at(X, 0, InsertPosition::Beside(A)));
        assert_eq!(w.workspace_of(X), Some(0));
        let items = w.strip(0).unwrap().items();
        assert_eq!((items[0].id, items[0].span), (A, Span::Left));
        assert_eq!((items[1].id, items[1].span), (X, Span::Right));

        // penempatan yang tidak berlaku (target tidak ada): tetap masuk ke workspace itu
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(X);
        assert!(w.move_window_at(X, 0, InsertPosition::Beside(C)));
        assert_eq!(w.workspace_of(X), Some(0));
    }

    #[test]
    fn swap_windows_only_within_one_workspace() {
        let mut w = ws();
        w.add_window(A);
        w.add_window(B);
        w.switch_to(1);
        w.add_window(C); // [A B] [C]
        assert!(w.swap_windows(A, B));
        assert_eq!(w.strip(0).unwrap().items()[0].id, B);
        assert!(!w.swap_windows(A, C)); // beda workspace
        assert!(!w.swap_windows(A, X)); // tidak dikenal
    }

    #[test]
    fn restore_brings_back_order_widths_focus_and_active_workspace() {
        let mut w = ws();
        w.add_window(A);
        w.cur_mut().snap(Dir::Left);
        w.add_window(B);
        w.cur_mut().snap(Dir::Up);
        w.switch_to(1);
        w.add_window(C); // [A B] [C] []
        let saved = w.snapshot();

        let mut fresh = ws();
        fresh.restore(&saved, |_| true);
        assert_eq!(fresh.snapshot(), saved);
        assert_eq!(fresh.active(), 1);

        // window yang sudah tidak ada dibuang; workspace yang jadi kosong ikut hilang
        let mut part = ws();
        part.restore(&saved, |id| id != C);
        assert_eq!(part.workspace_of(C), None);
        assert_eq!(part.workspace_of(A), Some(0));
        assert_eq!(part.active(), 0);

        // tidak menimpa susunan yang sudah ada
        let mut busy = ws();
        busy.add_window(X);
        busy.restore(&saved, |_| true);
        assert_eq!(busy.workspace_of(A), None);
        assert_eq!(busy.workspace_of(X), Some(0));

        // tidak ada yang masih hidup: tidak berubah
        let mut none = ws();
        none.restore(&saved, |_| false);
        assert!(none.overviews().is_empty());
    }

    #[test]
    fn wrapped_workspace_jumps_to_the_opposite_end() {
        let mut w = ws();
        w.add_window(A);
        w.switch_to(1);
        w.add_window(B);
        w.switch_to(2);
        w.add_window(C); // [A] [B] [C] []
        assert_eq!(w.wrapped_workspace(2, true), Some(0)); // setelah yang terakhir: pertama
        assert_eq!(w.wrapped_workspace(0, false), Some(2)); // sebelum yang pertama: terakhir
        assert_eq!(w.wrapped_workspace(1, true), Some(0)); // selalu ke ujung seberang
        // hanya satu workspace berisi: tidak ada tujuan lain
        let mut w = ws();
        w.add_window(A);
        assert_eq!(w.wrapped_workspace(0, true), None);
        assert_eq!(w.wrapped_workspace(0, false), None);
    }
}
