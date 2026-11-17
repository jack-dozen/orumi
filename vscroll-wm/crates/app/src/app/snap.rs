//! Snap layouts: seret window ke tepi atas layar -> menu preset muncul; lepas di sebuah
//! zona -> window mengisi zona itu. Zona lain di preset yang sama menampilkan daftar
//! window yang sedang terbuka (Snap Assist); klik sebuah window untuk mengisinya.
//!
//! Alur:
//!  1. EVENT_SYSTEM_MOVESIZESTART -> `snap_drag_start`: mulai memantau kursor dengan
//!     timer (`tick_snap_drag`). Seretan tepi window (resize) dikenali dari ukuran yang
//!     berubah dan diabaikan.
//!  2. Kursor masuk pita pemicu di tepi atas -> menu tampil (snap_menu.rs); zona di
//!     bawah kursor disorot. Menu hilang lagi kalau kursor menjauh.
//!  3. EVENT_SYSTEM_MOVESIZEEND -> `snap_drag_end`: kalau menu tampil dan kursor ada di
//!     sebuah zona, preset dipasang (`Workspaces::apply_snap`); selain itu seretan
//!     diproses seperti biasa (tukar posisi / geser pembagi, mouse.rs).
//!  4. Zona kosong menjadi slot di strip. Kalau `snap_assist` aktif, daftar window
//!     (snap_slots.rs) muncul di atas tiap slot. Memilih window memanggil
//!     `snap_assist_pick`. Sesi berakhir kalau semua slot terisi, atau dibatalkan
//!     (Esc, tombol tutup, klik di luar, pintasan keyboard, overview, seretan baru);
//!     slot yang tersisa dibuang sehingga zona itu tetap kosong.

use std::sync::atomic::Ordering;

use layout::{Rect, WindowId};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use super::App;
use crate::consts::{EVENT_DEBOUNCE_MS, SNAP_TICK_MS};
use crate::hook::SNAP_OPEN;
use crate::winutil::{bounds, ext_bounds, title, wid, work_area, Bounds};
use crate::{config, rules, safety, snap_menu, snap_slots, tray};

/// Jarak (piksel) dari panel menu yang masih dianggap "di menu" supaya menu tidak
/// menghilang saat kursor sedikit keluar.
const MENU_MARGIN: i32 = 24;
/// Gerak minimum (piksel) sebelum sebuah seretan dianggap benar-benar memindahkan
/// window, dan selisih ukuran maksimum sebelum dianggap seretan tepi (resize).
const MOVE_TOL: i32 = 6;

/// Seretan window yang sedang dipantau untuk menu snap layouts.
pub(crate) struct SnapDrag {
    id: WindowId,
    hwnd: HWND,
    /// Batas frame saat seretan mulai.
    start: Bounds,
    /// Susunan menu kalau sedang tampil.
    menu: Option<snap_menu::MenuLayout>,
    /// Zona (preset, zona) di bawah kursor.
    hover: Option<(usize, usize)>,
    /// Esc ditekan selama seretan: seretan dibatalkan, jangan dipasang.
    cancelled: bool,
}

/// Sesi daftar window untuk zona kosong.
pub(crate) struct SnapAssist {
    /// Window yang sudah menempati zona preset ini (untuk pelacak fokus).
    members: Vec<WindowId>,
    /// Slot yang belum terisi.
    slots: Vec<WindowId>,
}

impl App {
    /// Seretan window dimulai.
    pub(crate) unsafe fn snap_drag_start(&mut self, hwnd: HWND) {
        if !self.cfg.snap_layouts_enabled
            || self.cfg.snap_layouts.is_empty()
            || self.overview.is_some()
            || tray::PAUSED.load(Ordering::Relaxed)
        {
            return;
        }
        let id = wid(hwnd);
        if self.ws.workspace_of(id) != Some(self.ws.active())
            || self.hidden.contains(&id)
            || self.parked.contains(&id)
        {
            return;
        }
        // seretan baru membatalkan sesi sebelumnya; tanpa apply(): window yang sedang
        // diseret tidak boleh ditarik balik ke tempatnya
        self.snap_reset(false);
        self.snap_drag = Some(SnapDrag {
            id,
            hwnd,
            start: bounds(ext_bounds(hwnd)),
            menu: None,
            hover: None,
            cancelled: false,
        });
        if self.snap_timer == 0 {
            self.snap_timer = SetTimer(None, 0, SNAP_TICK_MS, None);
        }
    }

    /// Berhenti memantau seretan: sembunyikan menu dan matikan timer.
    unsafe fn snap_stop_drag(&mut self) {
        self.snap_drag = None;
        snap_menu::hide();
        if self.snap_timer != 0 {
            let _ = KillTimer(None, self.snap_timer);
            self.snap_timer = 0;
        }
    }

    /// Hentikan seretan dan sesi daftar window (pause, reload config, seretan baru).
    pub(crate) unsafe fn snap_reset(&mut self, apply: bool) {
        self.snap_stop_drag();
        if self.snap_assist.is_some() {
            self.snap_assist_end(apply);
        }
    }

    unsafe fn snap_show_menu(&mut self, work: Rect) {
        let scale = GetDpiForSystem() as f32 / 96.0;
        let menu = snap_menu::layout_menu(work, &self.cfg.snap_layouts, scale);
        let names: Vec<String> = self
            .cfg
            .snap_layouts
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let anim_ms = (self.cfg.anim_ms.unwrap_or(180.0).round() as u32).clamp(140, 260);
        snap_menu::show(menu.clone(), names, self.cfg.overview_colors, anim_ms);
        if let Some(d) = self.snap_drag.as_mut() {
            d.menu = Some(menu);
            d.hover = None;
        }
    }

    unsafe fn snap_hide_menu(&mut self) {
        snap_menu::hide();
        if let Some(d) = self.snap_drag.as_mut() {
            d.menu = None;
            d.hover = None;
        }
    }

    /// Dipanggil timer selama seretan: tampilkan / sembunyikan menu dan sorot zona.
    pub(crate) unsafe fn tick_snap_drag(&mut self) {
        let Some(d) = self.snap_drag.as_ref() else {
            self.snap_stop_drag();
            return;
        };
        let (hwnd, start, cancelled) = (d.hwnd, d.start, d.cancelled);
        if tray::PAUSED.load(Ordering::Relaxed)
            || self.overview.is_some()
            || !safety::alive(hwnd)
        {
            self.snap_stop_drag();
            return;
        }
        if cancelled {
            return;
        }
        if GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0 {
            // Esc membatalkan seretan oleh Windows: jangan pasang preset saat dilepas
            self.snap_hide_menu();
            if let Some(d) = self.snap_drag.as_mut() {
                d.cancelled = true;
            }
            return;
        }
        let now = bounds(ext_bounds(hwnd));
        let size_changed = ((now.2 - now.0) - (start.2 - start.0)).abs() > MOVE_TOL
            || ((now.3 - now.1) - (start.3 - start.1)).abs() > MOVE_TOL;
        if size_changed {
            // tepi window yang ditarik (resize), bukan dipindah: bukan urusan menu ini
            self.snap_stop_drag();
            return;
        }
        if GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0 {
            return; // tombol kiri tidak ditekan (mis. pindah lewat keyboard)
        }
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return;
        }
        let work = work_area();
        let in_band = pt.y < work.y + self.cfg.snap_trigger
            && pt.x >= work.x
            && pt.x < work.x + work.w;
        let moved = (now.0 - start.0).abs() > MOVE_TOL || (now.1 - start.1).abs() > MOVE_TOL;

        let shown = self
            .snap_drag
            .as_ref()
            .map_or(false, |d| d.menu.is_some());
        if !shown {
            if moved && in_band {
                self.snap_show_menu(work);
            }
            return;
        }
        // menu tampil: bertahan selama kursor di menu atau di pita pemicu
        let (hover, keep) = {
            let Some(m) = self.snap_drag.as_ref().and_then(|d| d.menu.as_ref()) else {
                return;
            };
            let hover = snap_menu::hit(m, pt.x, pt.y);
            let keep = hover.is_some()
                || snap_menu::inside(m, pt.x, pt.y, MENU_MARGIN)
                || pt.y < work.y + self.cfg.snap_trigger;
            (hover, keep)
        };
        if !keep {
            self.snap_hide_menu();
            return;
        }
        snap_menu::tick_anim();
        if let Some(d) = self.snap_drag.as_mut() {
            if d.hover != hover {
                d.hover = hover;
                snap_menu::set_hover(hover);
            }
        }
    }

    /// Seretan selesai. True kalau dipakai untuk memasang preset (pemanggil tidak perlu
    /// memprosesnya sebagai tukar posisi / geser pembagi).
    pub(crate) unsafe fn snap_drag_end(&mut self, hwnd: HWND) -> bool {
        let Some(d) = self.snap_drag.take() else {
            return false;
        };
        self.snap_stop_drag(); // menu hilang, timer mati
        if d.id != wid(hwnd) || d.cancelled {
            return false;
        }
        let Some(menu) = d.menu else {
            return false;
        };
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        let Some((pi, zi)) = snap_menu::hit(&menu, pt.x, pt.y) else {
            return false;
        };
        self.snap_apply(d.id, pi, zi);
        true
    }

    /// Pasang preset `pi` dengan `id` di zona `zi`.
    unsafe fn snap_apply(&mut self, id: WindowId, pi: usize, zi: usize) {
        let Some(preset) = self.cfg.snap_layouts.get(pi).cloned() else {
            return;
        };
        let slots = self.ws.apply_snap(id, &preset, zi);
        if !self.cfg.snap_assist {
            self.ws.clear_slots(); // zona lain dibiarkan kosong
        }
        self.apply();
        if let Some(&h) = self.map.get(&id) {
            self.focus_hwnd(h);
        }
        if self.cfg.snap_assist && !slots.is_empty() {
            self.snap_assist = Some(SnapAssist {
                members: vec![id],
                slots,
            });
            if !self.snap_assist_show() {
                self.snap_assist_end(true); // tidak ada window lain untuk dipilih
            }
        }
        self.schedule_sync(EVENT_DEBOUNCE_MS);
    }

    /// Candidate windows to fill empty slots: active, visible tiled windows in the current
    /// workspace (excluding `members`, hidden, and parked windows).
    unsafe fn snap_candidates(&self, members: &[WindowId]) -> Vec<snap_slots::Candidate> {
        let mode = self.cfg.overview_label;
        let active = self.ws.active();
        let mut out = Vec::new();
        for id in self.ws.window_ids(active) {
            if id.is_slot()
                || members.contains(&id)
                || self.hidden.contains(&id)
                || self.parked.contains(&id)
            {
                continue;
            }
            let Some(&h) = self.map.get(&id) else {
                continue;
            };
            let t = title(h);
            let exe = if mode == config::LabelMode::App {
                rules::exe_name(h)
            } else {
                None
            };
            out.push(snap_slots::Candidate {
                id: id.0,
                label: config::label(mode, &t, exe.as_deref(), &self.cfg.aliases),
                source: h.0 as isize,
            });
        }
        out
    }

    /// Tampilkan (ulang) daftar window di atas slot yang masih kosong. False kalau tidak
    /// ada yang bisa ditampilkan (tidak ada slot kelihatan atau tidak ada window).
    unsafe fn snap_assist_show(&mut self) -> bool {
        let Some(a) = self.snap_assist.as_ref() else {
            return false;
        };
        let places = self.ws.cur().layout();
        let views: Vec<snap_slots::SlotView> = a
            .slots
            .iter()
            .filter_map(|s| {
                places
                    .iter()
                    .find(|p| p.id == *s && p.visible) // zona di luar layar tidak bisa diklik
                    .map(|p| snap_slots::SlotView {
                        slot: s.0,
                        rect: p.rect,
                    })
            })
            .collect();
        if views.is_empty() {
            return false;
        }
        let cands = self.snap_candidates(&a.members);
        if cands.is_empty() {
            return false;
        }
        snap_slots::show(
            &views,
            &cands,
            self.cfg.overview_colors,
            self.cfg.overview_thumbnails,
        );
        SNAP_OPEN.store(true, Ordering::Relaxed);
        true
    }

    /// Sebuah window dipilih untuk mengisi slot (pesan WM_SNAP_PICK).
    pub(crate) unsafe fn snap_assist_pick(&mut self, win: u64, slot: u64) {
        if self.snap_assist.is_none() {
            return;
        }
        let (id, slot_id) = (WindowId(win), WindowId(slot));
        if self.ws.fill_slot(slot_id, id) {
            if let Some(a) = self.snap_assist.as_mut() {
                a.slots.retain(|s| *s != slot_id);
                a.members.push(id);
            }
            self.apply();
            if let Some(&h) = self.map.get(&id) {
                self.focus_hwnd(h);
            }
            self.schedule_sync(EVENT_DEBOUNCE_MS);
        }
        // gagal (window sudah hilang): cukup muat ulang daftarnya
        let more = self
            .snap_assist
            .as_ref()
            .map_or(false, |a| !a.slots.is_empty());
        if !more || !self.snap_assist_show() {
            self.snap_assist_end(true);
        }
    }

    /// Akhiri sesi: daftar window disembunyikan, slot yang tersisa dibuang (zonanya
    /// tetap kosong). `apply` = tata ulang window setelah slot dibuang.
    pub(crate) unsafe fn snap_assist_end(&mut self, apply: bool) {
        self.snap_assist = None;
        snap_slots::hide();
        SNAP_OPEN.store(false, Ordering::Relaxed);
        if self.ws.clear_slots() && apply {
            self.apply();
        }
    }

    /// Dipanggil sync(): sesi berakhir kalau fokus pindah ke window di luar preset ini
    /// (klik window lain, Alt+Tab).
    pub(crate) unsafe fn snap_assist_follow_focus(&mut self, fg: HWND) {
        let Some(a) = self.snap_assist.as_ref() else {
            return;
        };
        if fg.0.is_null() || snap_slots::is_ours(fg) || a.members.contains(&wid(fg)) {
            return; // foreground sedang transisi / milik kita / bagian dari preset
        }
        self.snap_assist_end(true);
    }
}
