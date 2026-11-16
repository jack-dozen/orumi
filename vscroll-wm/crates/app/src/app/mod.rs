//! Status aplikasi: window yang dikelola, sinkronisasi, dan penataan.

mod actions;
mod anim;
mod events;
mod mouse;
mod overview_ctl;
mod persist;
pub(crate) mod workspace;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use layout::{WindowId, Workspaces};

use windows::Win32::Foundation::{HWND, LPARAM};

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) use self::actions::show_config_problems;
use self::anim::Anim;
use crate::consts::DEFAULT_ANIM_MS;
use crate::hook::OVERVIEW_VK;
use crate::winutil::{
    animations_enabled, bounds, collect, ext_bounds, place, set_border, title, wid, work_area,
    Bounds,
};
use crate::{bindings, config, overview, safety, taskbar};
use workspace::{slide_dir, Slide};

/// Overview yang sedang terbuka.
pub(crate) struct OverviewSession {
    /// Kursor overview (belum tentu window yang fokus).
    pub(crate) selected: WindowId,
    /// Window yang foreground sebelum overview dibuka, untuk dikembalikan saat batal.
    pub(crate) prev_fg: HWND,
    /// Data untuk membangun ulang panel saat kolom tengah bergeser.
    pub(crate) model: overview::Model,
    pub(crate) titles: HashMap<u64, String>,
    pub(crate) sources: HashMap<u64, isize>,
    /// Window yang fokus saat overview dibuka (0 = tidak ada), untuk sorotan.
    pub(crate) focused: u64,
}

pub(crate) struct App {
    /// Semua workspace; yang aktif ditata, sisanya diparkir di luar layar.
    pub(crate) ws: Workspaces,
    pub(crate) map: HashMap<WindowId, HWND>,
    pub(crate) last_fg: Option<WindowId>,
    /// Batas frame tiap window hasil penempatan terakhir kita; kalau berubah
    /// tanpa kita, berarti window digeser dari luar (Win+Arrow, drag, dll).
    pub(crate) actual: HashMap<WindowId, Bounds>,
    pub(crate) anim: HashMap<WindowId, Anim>,
    /// Window yang masih ada tapi sedang tidak tampil (di-minimize, cloaked, dll).
    /// Slotnya tetap diingat di strip.
    pub(crate) hidden: HashSet<WindowId>,
    /// Window di workspace lain yang sedang diparkir di luar layar (tombol taskbar
    /// disembunyikan). Dibuka lagi oleh apply() saat workspacenya aktif.
    pub(crate) parked: HashSet<WindowId>,
    /// Window yang sedang meluncur keluar layar menuju parkir; diparkir sungguhan saat
    /// animasinya selesai (tick_anim). Selalu juga ada di `parked`.
    pub(crate) leaving: HashSet<WindowId>,
    /// Arah geser untuk apply() berikutnya (pindah / kirim workspace); dipakai sekali.
    pub(crate) slide: Option<Slide>,
    /// Window yang sengaja dibiarkan mengambang (tidak ditata) lewat Win+Alt+F.
    pub(crate) floating: HashMap<WindowId, HWND>,
    /// Window yang menolak dikendalikan (hak akses lebih tinggi). Dibiarkan mengambang.
    pub(crate) blocked: HashMap<WindowId, HWND>,
    /// 0 = timer animasi tidak jalan (tidak ada beban saat idle).
    pub(crate) anim_timer: usize,
    pub(crate) anim_ms: f32,
    /// Warna border window fokus (COLORREF), dari konfigurasi.
    pub(crate) focus_border: Option<u32>,
    /// Window yang sedang diwarnai bordernya, supaya bisa dikembalikan.
    pub(crate) border_applied: Option<(HWND, u32)>,
    pub(crate) overview: Option<OverviewSession>,
    pub(crate) cfg: config::Config,
    /// Timer sekali jalan untuk menata ulang setelah event (0 = tidak ada).
    pub(crate) sync_timer: usize,
    /// Ada window baru: jadwalkan satu pengecekan ulang.
    pub(crate) recheck: bool,
    /// Susunan yang terakhir ditulis ke layout.state (persist.rs).
    pub(crate) saved_layout: Option<layout::SavedLayout>,
    /// Identitas (exe|class) window untuk layout.state, dihitung sekali per window.
    pub(crate) idents: HashMap<WindowId, String>,
}

impl App {
    pub(crate) unsafe fn new() -> Self {
        Self {
            ws: Workspaces::new(work_area()),
            map: HashMap::new(),
            last_fg: None,
            actual: HashMap::new(),
            anim: HashMap::new(),
            hidden: HashSet::new(),
            parked: HashSet::new(),
            leaving: HashSet::new(),
            slide: None,
            floating: HashMap::new(),
            blocked: HashMap::new(),
            anim_timer: 0,
            sync_timer: 0,
            recheck: false,
            saved_layout: None,
            idents: HashMap::new(),
            anim_ms: DEFAULT_ANIM_MS,
            focus_border: None,
            border_applied: None,
            overview: None,
            cfg: config::Config::default(),
        }
    }

    /// Samakan state strip dengan window yang ada sekarang. True kalau ada perubahan.
    pub(crate) unsafe fn sync(&mut self) -> bool {
        let mut list: Vec<HWND> = Vec::new();
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut Vec<HWND> as isize));
        let live: HashMap<WindowId, HWND> = list.iter().map(|&h| (wid(h), h)).collect();
        let mut changed = false;
        self.floating.retain(|_, h| safety::alive(*h)); // yang ditutup tidak perlu diingat
        self.blocked.retain(|_, h| safety::alive(*h));

        let gone: Vec<WindowId> = self
            .map
            .keys()
            .filter(|k| !live.contains_key(k))
            .copied()
            .collect();
        for id in gone {
            let h = self.map[&id];
            if safety::alive(h) {
                // di-minimize / disembunyikan / pindah virtual desktop:
                // window masih ada, jadi ingat slotnya, jangan hapus dari strip
                if self.hidden.insert(id) {
                    self.ws.set_hidden(id, true);
                    self.anim.remove(&id);
                    changed = true;
                }
                continue;
            }
            // benar-benar ditutup
            self.ws.remove_window(id);
            self.map.remove(&id);
            self.hidden.remove(&id);
            self.parked.remove(&id);
            self.leaving.remove(&id);
            taskbar::forget(h);
            safety::forget(h);
            self.actual.remove(&id);
            self.anim.remove(&id);
            changed = true;
        }
        let mut added = false;
        let work = work_area();
        for &h in &list {
            let id = wid(h);
            if self.floating.contains_key(&id) || self.blocked.contains_key(&id) {
                continue;
            }
            if !self.map.contains_key(&id) {
                safety::remember(h, work); // posisi asli, untuk dipulihkan saat keluar
                self.map.insert(id, h);
                self.ws.add_window(id); // masuk ke workspace aktif
                added = true;
            } else if self.hidden.remove(&id) {
                // muncul lagi (mis. setelah Win+D): kembali ke slotnya semula
                self.parked.remove(&id); // kalau di workspace lain, diparkir ulang oleh apply()
                self.ws.set_hidden(id, false);
                added = true;
            }
        }
        changed |= added;
        if added {
            self.recheck = true;
        }

        // ikuti window yang difokuskan user (klik, alt-tab, tombol taskbar). Window di
        // workspace lain membawa kita ke workspace itu. Foreground lama yang masih
        // tertinggal di window yang baru diparkir tidak dihitung.
        let fg = wid(GetForegroundWindow());
        if (added && !self.parked.contains(&fg)) || self.last_fg != Some(fg) {
            self.last_fg = Some(fg);
            if self.map.contains_key(&fg) && self.ws.cur().focused() != Some(fg) {
                let old = self.ws.active();
                if self.ws.focus_window(fg) {
                    self.slide = Some(Slide::switching(slide_dir(old, self.ws.active())));
                    self.update_workspace_tip();
                }
                changed = true;
            }
        }
        // foreground bisa berubah tanpa mengubah layout (mis. setelah Win+Shift+Arrow)
        self.refresh_border();
        self.publish_ipc();
        changed || self.drifted()
    }

    pub(crate) fn publish_ipc(&self) {
        crate::ipc::publish(crate::ipc::snapshot_json(self));
    }

    /// True kalau ada window yang posisinya bukan hasil penempatan kita.
    /// Diabaikan saat tombol kiri mouse ditekan (user sedang drag).
    pub(crate) unsafe fn drifted(&self) -> bool {
        if !self.anim.is_empty() {
            return false; // posisi sedang bergerak karena animasi kita
        }
        if GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 {
            return false;
        }
        self.map
            .iter()
            .filter(|(id, _)| !self.hidden.contains(id))
            .any(|(id, &h)| self.actual.get(id) != Some(&bounds(ext_bounds(h))))
    }

    pub(crate) unsafe fn apply(&mut self) {
        let work = work_area();
        self.ws.set_viewport(work);
        let now = Instant::now();
        let slide = self.slide.take(); // arah geser pindah workspace, sekali pakai
        // Pass kedua hanya kalau ada window yang menolak dikecilkan
        // (tinggi minimum) sehingga layout perlu dihitung ulang.
        for _ in 0..2 {
            let mut grew = false;
            for p in self.ws.cur().layout() {
                let Some(&h) = self.map.get(&p.id) else {
                    continue;
                };
                let t = p.rect;
                // baru keluar dari parkir: mulai dari luar layar di sisi masuk lalu meluncur
                // (jangan dianimasikan dari -32768). Yang masih meluncur keluar (pindah balik
                // di tengah animasi) cukup dilanjutkan ke posisi barunya.
                let was_parked = self.parked.remove(&p.id);
                let was_leaving = self.leaving.remove(&p.id);
                if was_parked {
                    taskbar::show(h);
                }
                let at_park = was_parked && !was_leaving;
                let cur = ext_bounds(h);
                let same_size = cur.right - cur.left == t.w
                    && cur.bottom - cur.top == t.h
                    && !IsZoomed(h).as_bool();
                if same_size && !at_park {
                    // hanya posisi yang berubah -> boleh dianimasikan
                    self.move_only(p.id, h, (cur.left, cur.top), t, work, now);
                    continue;
                }
                if same_size && self.anim_ms > 0.0 {
                    if let Some(s) = slide {
                        self.slide_from(p.id, h, t, (s.enter_from * work.w, 0), work, now);
                        continue;
                    }
                }
                // ukuran berubah -> langsung ke ukuran akhir, tanpa animasi
                self.anim.remove(&p.id);
                let (mut a, denied) = place(h, t);
                if denied {
                    self.mark_blocked(p.id, h);
                    grew = true; // hitung ulang layout tanpa window ini
                    continue;
                }
                let mut got = a.bottom - a.top;
                let mut got_w = a.right - a.left;
                if got > t.h + 2 || got_w > t.w + 2 {
                    // Sebagian app (Electron/UWP) menerapkan ukuran dengan jeda;
                    // ukur ulang sebelum menyimpulkan ada tinggi minimum.
                    std::thread::sleep(Duration::from_millis(40));
                    a = ext_bounds(h);
                    got = a.bottom - a.top;
                    got_w = a.right - a.left;
                }
                self.actual.insert(p.id, bounds(a));
                if got > t.h + 2 && self.ws.cur_mut().set_min_height(p.id, got) {
                    eprintln!(
                        "[min-height] \"{}\" menolak lebih pendek dari {} px (diminta {})",
                        title(h),
                        got,
                        t.h
                    );
                    grew = true;
                }
                if got_w > t.w + 2 && self.ws.cur_mut().set_min_width(p.id, got_w) {
                    eprintln!(
                        "[min-width] \"{}\" menolak lebih sempit dari {} px (diminta {})",
                        title(h),
                        got_w,
                        t.w
                    );
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        // window workspace aktif yang tidak ikut ditata (di-minimize) tapi masih diparkir
        for id in self.ws.window_ids(self.ws.active()) {
            self.leaving.remove(&id);
            if self.parked.remove(&id) {
                if let Some(&h) = self.map.get(&id) {
                    taskbar::show(h);
                }
            }
        }
        self.park_inactive(slide, work, now);
        // isyarat tukar posisi window penuh lebar (lihat Strip::take_move_cue)
        if let Some((id, dir)) = self.ws.cur_mut().take_move_cue() {
            self.cue_slide(id, dir, work, now);
        }
        if !self.anim.is_empty() && self.anim_timer == 0 {
            // 10 ms = minimum Windows; praktisnya berdetak tiap ~15,6 ms (~64 fps)
            self.anim_timer = SetTimer(None, 0, 10, None);
        }
        self.refresh_border();
        self.publish_ipc();
    }

    /// Warnai border window yang SEDANG foreground (kalau ditata dan focus_border
    /// diatur), dan kembalikan border window sebelumnya.
    ///
    /// Sengaja berdasar foreground sebenarnya, bukan tujuan fokus di strip: warna
    /// yang dipasang sebelum window selesai diaktifkan bisa hilang saat aktivasi.
    pub(crate) unsafe fn refresh_border(&mut self) {
        let want = match self.focus_border {
            Some(c) => {
                let fg = GetForegroundWindow();
                self.map.contains_key(&wid(fg)).then_some((fg, c))
            }
            None => None,
        };
        if want == self.border_applied {
            return;
        }
        if let Some((old, _)) = self.border_applied.take() {
            if want.map_or(true, |(h, _)| h != old) && safety::alive(old) {
                set_border(old, safety::DWM_COLOR_DEFAULT);
            }
        }
        if let Some((h, c)) = want {
            set_border(h, c);
            self.border_applied = Some((h, c));
        }
    }

    /// Terapkan konfigurasi (gap, animasi, warna border).
    pub(crate) unsafe fn apply_config(&mut self, cfg: &config::Config) {
        self.cfg = cfg.clone();
        bindings::install(&cfg.bindings);
        OVERVIEW_VK.store(cfg.bindings.overview_cycle_vk(), Ordering::Relaxed);
        self.ws.set_gap(cfg.gap);
        self.focus_border = cfg.focus_border;
        self.refresh_border();
        self.anim_ms = cfg.anim_ms.unwrap_or(if animations_enabled() {
            DEFAULT_ANIM_MS
        } else {
            0.0
        });
    }

    /// Muat ulang file konfigurasi (menu tray). Aturan ignore butuh restart.
    pub(crate) unsafe fn reload_config(&mut self) {
        let report = config::load();
        self.apply_config(&report.cfg);
        self.apply();
        actions::show_config_problems(&report);
    }
}
