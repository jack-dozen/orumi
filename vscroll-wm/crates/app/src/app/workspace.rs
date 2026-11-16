//! Workspace: pindah, kirim window, dan parkir window workspace lain.
//!
//! Window workspace yang tidak aktif diparkir di x = PARK_X (ukuran tetap, jadi saat
//! dibuka lagi hanya posisinya yang berubah) dan tombol taskbar-nya disembunyikan
//! (lihat taskbar.rs). Window tetap "visible" bagi EnumWindows, jadi sync() tidak
//! menganggapnya minimize.
//!
//! Pindah / kirim workspace dianimasikan geser horizontal (`Slide`): window yang keluar
//! meluncur ke luar layar dan baru diparkir sungguhan saat animasinya selesai
//! (`leaving`, lihat tick_anim); window yang masuk dimulai dari luar layar di sisi
//! berlawanan lalu meluncur ke tempatnya. Animasi mati -> langsung, seperti biasa.

use std::sync::atomic::Ordering;
use std::time::Instant;

use layout::{Rect, WindowId};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetShellWindow, SetForegroundWindow, SwitchToThisWindow,
};

use super::App;
use crate::consts::PARK_X;
use crate::winutil::{bounds, ext_bounds, place, wid, work_area};
use crate::{osd, safety, taskbar, tray};

/// Arah geser animasi pindah workspace: `enter_from` = sisi asal window yang masuk
/// (+1 kanan, -1 kiri), `exit_to` = sisi tujuan window yang keluar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slide {
    pub(crate) enter_from: i32,
    pub(crate) exit_to: i32,
}

impl Slide {
    /// Pindah workspace ke arah `dir`: isi layar bergeser berlawanan (ke kanan = isi ke kiri).
    pub(crate) fn switching(dir: i32) -> Self {
        Slide {
            enter_from: dir,
            exit_to: -dir,
        }
    }

    /// Kirim window ke workspace ke arah `dir`: window itu meluncur ke sisi tersebut.
    pub(crate) fn sending(dir: i32) -> Self {
        Slide {
            enter_from: -dir,
            exit_to: dir,
        }
    }
}

/// Arah dari workspace `old` ke `new` (nomor sesudah dirapikan): -1 kiri, +1 kanan.
/// Nomor sama berarti workspace lama kosong lalu dihapus, jadi kita pasti ke kanan.
pub(crate) fn slide_dir(old: usize, new: usize) -> i32 {
    if new < old {
        -1
    } else {
        1
    }
}

impl App {
    /// Pindah ke workspace nomor `n` (0 = pertama).
    pub(crate) unsafe fn switch_workspace(&mut self, n: usize) {
        self.switch_workspace_with_focus(n, true);
    }

    /// Switch to workspace `n` (0-based). `focus = false` keeps keyboard focus on the caller
    /// (e.g. ORUMI Activities panel) while sliding/parking windows in the background.
    pub(crate) unsafe fn switch_workspace_with_focus(&mut self, n: usize, focus: bool) -> bool {
        let old = self.ws.active();
        if self.ws.switch_to(n) {
            self.slide = Some(Slide::switching(slide_dir(old, self.ws.active())));
            self.after_workspace_change(focus);
            true
        } else {
            false
        }
    }

    /// Workspace sebelumnya / berikutnya (tidak berputar).
    pub(crate) unsafe fn switch_workspace_rel(&mut self, delta: i32) {
        let old = self.ws.active();
        if self.ws.switch_rel(delta) {
            self.slide = Some(Slide::switching(slide_dir(old, self.ws.active())));
            self.after_workspace_change(true);
        }
    }

    /// Kirim window yang fokus ke workspace `n`; kita tetap di workspace ini.
    pub(crate) unsafe fn send_to_workspace(&mut self, n: usize) {
        let dir = slide_dir(self.ws.active(), n);
        if self.ws.move_focused_to(n).is_some() {
            self.slide = Some(Slide::sending(dir));
            self.after_workspace_change(true);
        }
    }

    pub(crate) unsafe fn send_to_workspace_rel(&mut self, delta: i32) {
        if self.ws.move_focused_rel(delta).is_some() {
            self.slide = Some(Slide::sending(delta.signum()));
            self.after_workspace_change(true);
        }
    }

    unsafe fn after_workspace_change(&mut self, focus: bool) {
        self.apply(); // buka parkir workspace aktif, parkir yang lain
        if focus {
            self.focus_current();
        } else {
            self.last_fg = Some(wid(GetForegroundWindow()));
        }
        self.update_workspace_tip();
    }

    /// Bring `h` to foreground (with `SwitchToThisWindow` fallback) and sync `last_fg`.
    pub(crate) unsafe fn focus_hwnd(&mut self, h: HWND) {
        if !SetForegroundWindow(h).as_bool() {
            SwitchToThisWindow(h, true);
        }
        // Always record the actual foreground window so a rejected focus change never causes
        // sync() to snap back to a newly parked workspace.
        self.last_fg = Some(wid(GetForegroundWindow()));
    }

    /// Fokus ke window yang fokus di workspace aktif; kalau workspace kosong, ke desktop
    /// supaya foreground tidak tertinggal di window yang baru diparkir.
    unsafe fn focus_current(&mut self) {
        let target = self
            .ws
            .cur()
            .focused()
            .and_then(|f| self.map.get(&f).copied());
        if let Some(h) = target {
            self.focus_hwnd(h);
        } else {
            let shell = GetShellWindow();
            if !shell.0.is_null() && !SetForegroundWindow(shell).as_bool() {
                SwitchToThisWindow(shell, true);
            }
            self.last_fg = Some(wid(GetForegroundWindow()));
        }
    }

    /// Tooltip tray menunjukkan workspace aktif; kalau `workspace_osd` aktif, panel kecil
    /// muncul sebentar setiap workspace berubah (osd.rs mengabaikan panggilan yang sama).
    pub(crate) unsafe fn update_workspace_tip(&self) {
        if tray::PAUSED.load(Ordering::Relaxed) {
            return;
        }
        let active = self.ws.active();
        tray::set_tip(&format!("vscroll-wm - workspace {}", active + 1));
        if self.cfg.workspace_osd {
            let total = self
                .ws
                .overviews()
                .iter()
                .map(|(n, _)| n + 1)
                .max()
                .unwrap_or(0)
                .max(active + 1);
            osd::notify(active, total, self.cfg.overview_colors, work_area());
        }
    }

    /// Saat start: kembalikan tombol taskbar yang mungkin tertinggal terhapus oleh
    /// proses sebelumnya yang mati paksa.
    pub(crate) unsafe fn restore_all_tabs(&self) {
        for &h in self.map.values() {
            taskbar::restore(h);
        }
    }

    /// Parkir semua window di workspace selain yang aktif. Idempoten. Dengan `slide`,
    /// window yang masih tampak meluncur keluar dulu.
    pub(crate) unsafe fn park_inactive(&mut self, slide: Option<Slide>, work: Rect, now: Instant) {
        let active = self.ws.active();
        let ids: Vec<WindowId> = (0..self.ws.count())
            .filter(|&n| n != active)
            .flat_map(|n| self.ws.window_ids(n))
            .collect();
        for id in ids {
            self.park(id, slide, work, now);
        }
    }

    unsafe fn park(&mut self, id: WindowId, slide: Option<Slide>, work: Rect, now: Instant) {
        let Some(&h) = self.map.get(&id) else {
            return;
        };
        if !self.parked.insert(id) {
            return; // sudah diparkir, atau masih meluncur keluar (leaving)
        }
        taskbar::hide(h);
        if self.hidden.contains(&id) {
            self.anim.remove(&id);
            return; // minimize / cloaked: cukup tombolnya yang disembunyikan
        }
        if let Some(s) = slide {
            // meluncur keluar dulu; parkir sungguhan setelah animasinya selesai
            if self.slide_out(id, h, s.exit_to, work, now) {
                self.leaving.insert(id);
                return;
            }
        }
        self.anim.remove(&id);
        self.finish_park(id);
    }

    /// Pindahkan window ke posisi parkir (di luar layar, ukuran tetap).
    pub(crate) unsafe fn finish_park(&mut self, id: WindowId) {
        let Some(&h) = self.map.get(&id) else {
            return;
        };
        if !safety::alive(h) {
            return;
        }
        let cur = ext_bounds(h);
        let target = Rect {
            x: PARK_X,
            y: cur.top,
            w: cur.right - cur.left,
            h: cur.bottom - cur.top,
        };
        let (actual, denied) = place(h, target);
        if denied {
            self.mark_blocked(id, h); // hak akses lebih tinggi: tidak bisa diparkir
            return;
        }
        // yang dicatat posisi sebenarnya (USER32 bisa menjepit), supaya bukan "drift"
        self.actual.insert(id, bounds(actual));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slide_direction_follows_workspace_order() {
        assert_eq!(slide_dir(0, 1), 1);
        assert_eq!(slide_dir(3, 1), -1);
        // workspace lama (kosong, di tengah) dihapus: nomor baru bisa sama -> ke kanan
        assert_eq!(slide_dir(1, 1), 1);
    }

    #[test]
    fn switching_moves_content_against_travel_and_sending_goes_toward_the_target() {
        assert_eq!(
            Slide::switching(1),
            Slide {
                enter_from: 1,
                exit_to: -1
            }
        );
        assert_eq!(
            Slide::switching(-1),
            Slide {
                enter_from: -1,
                exit_to: 1
            }
        );
        assert_eq!(Slide::sending(1).exit_to, 1);
        assert_eq!(Slide::sending(-1).exit_to, -1);
    }
}
