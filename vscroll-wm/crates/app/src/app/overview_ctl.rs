//! Pengendali overview (sisi aplikasi; gambarnya ada di overview.rs).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use layout::{Dir, InsertPosition, WindowId};

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use super::workspace::{slide_dir, Slide};
use super::{App, OverviewSession};
use crate::consts::{DIRS, EVENT_DEBOUNCE_MS};
use crate::hook::{OV_MOVED, OV_OPEN};
use crate::winutil::{title, wid, work_area};
use crate::{config, overview, rules, safety};

impl App {
    /// Win+Alt+Space: buka / tutup overview.
    pub(crate) unsafe fn toggle_overview(&mut self) {
        if self.overview.is_some() {
            self.close_overview(false);
        } else {
            self.open_overview();
        }
    }

    pub(crate) unsafe fn open_overview(&mut self) {
        if !self.try_open_overview() {
            // gagal membuka: batalkan status "terbuka" yang mungkin sudah diset hook
            OV_OPEN.store(false, Ordering::Relaxed);
            OV_MOVED.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) unsafe fn try_open_overview(&mut self) -> bool {
        if !overview::available() {
            return false;
        }
        let maps = self.ws.overviews();
        if maps.is_empty() {
            return false;
        }
        // Kolom tengah = workspace aktif (atau yang terdekat kalau aktif kosong).
        let active = self.ws.active();
        let center = maps
            .iter()
            .enumerate()
            .min_by_key(|(_, (n, _))| (n.abs_diff(active), *n))
            .map_or(0, |(i, _)| i);
        let Some(selected) = self.ws.representative(maps[center].0) else {
            return false;
        };
        let mode = self.cfg.overview_label;
        let all = || maps.iter().flat_map(|(_, o)| o.items.iter());
        let titles: HashMap<u64, String> = all()
            .filter_map(|it| {
                self.map.get(&it.id).map(|&h| {
                    let t = title(h);
                    let exe = if mode == config::LabelMode::App {
                        rules::exe_name(h)
                    } else {
                        None
                    };
                    (
                        it.id.0,
                        config::label(mode, &t, exe.as_deref(), &self.cfg.aliases),
                    )
                })
            })
            .collect();
        let sources: HashMap<u64, isize> = all()
            .filter_map(|it| self.map.get(&it.id).map(|&h| (it.id.0, h.0 as isize)))
            .collect();
        let focused = self.ws.cur().focused().map_or(0, |f| f.0);
        let (first, visible) = overview::window_for(maps.len(), center);
        let model = overview::Model {
            maps,
            first,
            visible,
            big: center,
            active_ws: active,
        };
        let draw = overview::build(
            &model,
            &titles,
            &sources,
            selected.0,
            focused,
            work_area(),
            &self.overview_options(),
        );
        self.overview = Some(OverviewSession {
            selected,
            prev_fg: GetForegroundWindow(),
            model,
            titles,
            sources,
            focused,
        });
        overview::show(draw, self.cfg.overview_thumbnails);
        OV_MOVED.store(false, Ordering::Relaxed);
        OV_OPEN.store(true, Ordering::Relaxed);
        true
    }

    fn overview_options(&self) -> overview::Options {
        overview::Options {
            hint: self.cfg.overview_hint,
            size_pct: self.cfg.overview_size,
            colors: self.cfg.overview_colors,
        }
    }

    /// Pindahkan kursor ke `n`. Kalau workspace-nya berbeda, kolom itu jadi yang besar
    /// dan jendela kolom digeser seperlunya (panel digambar ulang); selain itu cukup
    /// menyorot.
    unsafe fn overview_select(&mut self, n: WindowId) {
        let opts = self.overview_options();
        let thumbs = self.cfg.overview_thumbnails;
        let Some(s) = self.overview.as_mut() else {
            return;
        };
        s.selected = n;
        let col = s
            .model
            .maps
            .iter()
            .position(|(_, o)| o.items.iter().any(|it| it.id == n));
        let Some(col) = col else {
            overview::set_selected(n.0);
            return;
        };
        let first = overview::scroll(s.model.first, s.model.visible, col);
        if col == s.model.big && first == s.model.first {
            overview::set_selected(n.0);
            return;
        }
        s.model.big = col;
        s.model.first = first;
        let draw = overview::build(
            &s.model,
            &s.titles,
            &s.sources,
            n.0,
            s.focused,
            work_area(),
            &opts,
        );
        overview::show(draw, thumbs);
    }

    /// Tab di overview: kursor ke window berikutnya / sebelumnya, hanya di dalam
    /// workspace window yang sedang dipilih (urutan baca).
    pub(crate) unsafe fn overview_cycle(&mut self, forward: bool) {
        let Some(sel) = self.overview.as_ref().map(|s| s.selected) else {
            return;
        };
        let next = self
            .ws
            .workspace_of(sel)
            .and_then(|n| self.ws.strip(n))
            .and_then(|st| st.cycle_from(sel, forward));
        if let Some(n) = next {
            self.overview_select(n);
        }
    }

    /// Panah di overview: geser kursor ke tetangga di workspace yang sama (window
    /// belum berpindah fokus). Kiri/Kanan di tepi workspace lanjut ke workspace
    /// tetangga, masuk dari tepi yang berhadapan, ke window yang setinggi kursor.
    /// Kalau sudah di ujung dan `overview_wrap` aktif, kursor berputar ke ujung seberang
    /// (workspace pertama / terakhir untuk Kiri/Kanan; baris terbawah / teratas untuk
    /// Atas/Bawah; kalau hanya ada satu workspace, Kiri/Kanan berputar di dalamnya).
    pub(crate) unsafe fn overview_move(&mut self, dir_index: usize) {
        let Some(dir) = DIRS.get(dir_index).map(|d| d.1) else {
            return;
        };
        let Some(sel) = self.overview.as_ref().map(|s| s.selected) else {
            return;
        };
        let Some(from) = self.ws.workspace_of(sel) else {
            return;
        };
        let mut next = self
            .ws
            .strip(from)
            .and_then(|st| st.neighbor_of(sel, dir))
            .or_else(|| self.overview_cross(from, sel, dir, false));
        if next.is_none() && self.cfg.overview_wrap {
            next = self.overview_cross(from, sel, dir, true).or_else(|| {
                self.ws
                    .strip(from)
                    .and_then(|st| st.wrap_neighbor_of(sel, dir))
            });
        }
        if let Some(n) = next {
            self.overview_select(n);
        }
    }

    /// Kiri/Kanan melewati tepi workspace: window tujuan di workspace tetangga (`wrap` =
    /// berputar ke workspace di ujung seberang), setinggi kursor. None untuk Atas/Bawah.
    fn overview_cross(&self, from: usize, sel: WindowId, dir: Dir, wrap: bool) -> Option<WindowId> {
        if !matches!(dir, Dir::Left | Dir::Right) {
            return None;
        }
        let forward = dir == Dir::Right;
        let target = if wrap {
            self.ws.wrapped_workspace(from, forward)?
        } else {
            self.ws.adjacent_workspace(from, forward)?
        };
        let y = self
            .ws
            .strip(from)?
            .overview()
            .items
            .iter()
            .find(|it| it.id == sel)
            .map(|it| it.rect.y + it.rect.h / 2)?;
        self.ws.entry_window(target, forward, y)
    }

    /// Win+Ctrl+Kiri/Kanan atau angka di overview: kursor lompat ke workspace lain.
    /// `code`: 0 sebelumnya, 1 berikutnya, n >= 2 workspace nomor n - 2 (mulai 0).
    pub(crate) unsafe fn overview_workspace(&mut self, code: usize) {
        let Some(sel) = self.overview.as_ref().map(|s| s.selected) else {
            return;
        };
        let Some(from) = self.ws.workspace_of(sel) else {
            return;
        };
        let target = match code {
            0 => self.ws.adjacent_workspace(from, false),
            1 => self.ws.adjacent_workspace(from, true),
            n => Some(n - 2),
        };
        if let Some(w) = target.and_then(|t| self.ws.representative(t)) {
            self.overview_select(w);
        }
    }

    /// Window diseret lalu dilepas di overview (lihat `overview::take_drop`):
    ///  - di workspace mana pun (termasuk asalnya): pindah ke tempat yang ditunjuk penanda:
    ///    membagi dua window di bawah kursor (penuh lebar -> berdampingan, setengah lebar ->
    ///    bertumpuk), mengisi lajur kosong di sebelah window setengah lebar, atau penuh
    ///    lebar di atas / bawah blok (pemisah blok baru)
    /// Tiling nyata ikut diperbarui; panel digambar ulang dan tetap terbuka.
    pub(crate) unsafe fn overview_drop(&mut self) {
        let Some((src, slot)) = overview::take_drop() else {
            return;
        };
        if self.overview.is_none() {
            return;
        }
        let src = WindowId(src);
        let Some(from) = self.ws.workspace_of(src) else {
            return;
        };
        let to = slot.number.saturating_sub(1);
        let at = match slot.kind {
            overview::SlotKind::Beside(t) => InsertPosition::Beside(WindowId(t)),
            overview::SlotKind::Before(t) => InsertPosition::FullBefore(WindowId(t)),
            overview::SlotKind::After(t) => InsertPosition::FullAfter(WindowId(t)),
            overview::SlotKind::Split(t, first) => InsertPosition::Split(WindowId(t), first),
        };
        let changed = if to != from {
            self.ws.move_window_at(src, to, at)
        } else {
            self.ws.reposition_window(src, at)
        };
        if changed {
            self.apply(); // parkir / penataan mengikuti susunan baru
            self.overview_rebuild(src);
        }
    }

    /// Susun ulang panel dari data layout sekarang (setelah window dipindah), kursor di
    /// `selected`.
    unsafe fn overview_rebuild(&mut self, selected: WindowId) {
        let opts = self.overview_options();
        let thumbs = self.cfg.overview_thumbnails;
        let active = self.ws.active();
        let maps = self.ws.overviews();
        if maps.is_empty() {
            return;
        }
        let col = maps
            .iter()
            .position(|(_, o)| o.items.iter().any(|it| it.id == selected))
            .unwrap_or(0);
        let (first, visible) = overview::window_for(maps.len(), col);
        let Some(s) = self.overview.as_mut() else {
            return;
        };
        s.selected = selected;
        s.model = overview::Model {
            maps,
            first,
            visible,
            big: col,
            active_ws: active,
        };
        let draw = overview::build(
            &s.model,
            &s.titles,
            &s.sources,
            selected.0,
            s.focused,
            work_area(),
            &opts,
        );
        overview::show(draw, thumbs);
    }

    pub(crate) unsafe fn overview_pick(&mut self, id: u64) {
        if let Some(s) = self.overview.as_mut() {
            s.selected = WindowId(id);
        }
        self.close_overview(true);
    }

    /// Tutup overview. `accept` = loncat ke window yang dipilih; kalau tidak,
    /// fokus dikembalikan ke window sebelumnya.
    pub(crate) unsafe fn close_overview(&mut self, accept: bool) {
        OV_OPEN.store(false, Ordering::Relaxed);
        OV_MOVED.store(false, Ordering::Relaxed);
        let Some(s) = self.overview.take() else {
            return;
        };
        overview::hide();
        if accept {
            let old = self.ws.active();
            if self.ws.focus_window(s.selected) {
                // window itu ada di workspace lain: pindah dengan animasi geser
                self.slide = Some(Slide::switching(slide_dir(old, self.ws.active())));
                self.update_workspace_tip();
            }
            self.apply();
            if let Some(&h) = self.map.get(&s.selected) {
                let _ = SetForegroundWindow(h);
                self.last_fg = Some(wid(h));
            }
            self.schedule_sync(EVENT_DEBOUNCE_MS); // border dan penataan setelah aktivasi
        } else if safety::alive(s.prev_fg) {
            let _ = SetForegroundWindow(s.prev_fg);
        }
    }
}
