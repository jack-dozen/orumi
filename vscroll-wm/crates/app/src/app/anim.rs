//! Animasi posisi window.

use std::time::Instant;

use layout::{Dir, Rect, WindowId};

use windows::Win32::Foundation::HWND;

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use super::App;
use crate::winutil::{bounds, ext_bounds, hits, insets};

/// Jarak awal isyarat tukar posisi: seperlima area kerja.
const CUE_FRACTION: i32 = 5;

/// Selisih awal isyarat tukar posisi: window mulai bergeser berlawanan dengan arah
/// perpindahan lalu meluncur ke tempatnya, jadi tampak bergerak searah `dir`.
pub(crate) fn cue_offset(dir: Dir, work: Rect) -> (i32, i32) {
    match dir {
        Dir::Down => (0, -work.h / CUE_FRACTION),
        Dir::Up => (0, work.h / CUE_FRACTION),
        Dir::Right => (-work.w / CUE_FRACTION, 0),
        Dir::Left => (work.w / CUE_FRACTION, 0),
    }
}

/// Animasi posisi satu window (ukuran tidak ikut dianimasikan).
pub(crate) struct Anim {
    pub(crate) hwnd: HWND,
    pub(crate) from: (i32, i32),
    pub(crate) to: (i32, i32),
    /// Selisih rect window dengan frame yang terlihat (invisible border).
    pub(crate) inset: (i32, i32),
    pub(crate) start: Instant,
    pub(crate) dur_ms: f32,
}

impl Anim {
    /// Posisi frame pada waktu `now` dan apakah animasi sudah selesai.
    /// Kurva ease-out cubic: cepat di awal, melambat di akhir.
    pub(crate) fn pos_at(&self, now: Instant) -> ((i32, i32), bool) {
        let t = if self.dur_ms <= 0.0 {
            1.0
        } else {
            (now.duration_since(self.start).as_secs_f32() * 1000.0 / self.dur_ms).min(1.0)
        };
        let e = 1.0 - (1.0 - t).powi(3);
        let f = |a: i32, b: i32| (a as f32 + (b - a) as f32 * e).round() as i32;
        (
            (f(self.from.0, self.to.0), f(self.from.1, self.to.1)),
            t >= 1.0,
        )
    }
}

impl App {
    /// Pindahkan window yang ukurannya sudah benar ke `t`: dianimasikan kalau
    /// terlihat di layar, langsung kalau di luar layar (tanpa kerja sia-sia).
    pub(crate) unsafe fn move_only(
        &mut self,
        id: WindowId,
        hwnd: HWND,
        cur: (i32, i32),
        t: Rect,
        work: Rect,
        now: Instant,
    ) {
        let to = (t.x, t.y);
        if self.anim.get(&id).map_or(false, |a| a.to == to) {
            return; // sudah menuju ke sana
        }
        // animasi yang sedang berjalan dilanjutkan dari posisi saat ini (mulus)
        let from = self.anim.get(&id).map_or(cur, |a| a.pos_at(now).0);
        self.actual.insert(id, (t.x, t.y, t.x + t.w, t.y + t.h));
        if from == to {
            self.anim.remove(&id);
            return;
        }
        let inset = insets(hwnd);
        let visible = hits(work, from.0, from.1, t.w, t.h) || hits(work, t.x, t.y, t.w, t.h);
        if self.anim_ms > 0.0 && visible {
            self.anim.insert(
                id,
                Anim {
                    hwnd,
                    from,
                    to,
                    inset,
                    start: now,
                    dur_ms: self.anim_ms,
                },
            );
        } else {
            self.anim.remove(&id);
            let _ = SetWindowPos(
                hwnd,
                None,
                to.0 - inset.0,
                to.1 - inset.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub(crate) unsafe fn stop_anim_timer(&mut self) {
        if self.anim_timer != 0 {
            let _ = KillTimer(None, self.anim_timer);
            self.anim_timer = 0;
        }
    }

    /// Loncat ke `t` digeser `off`, lalu meluncur ke `t`. Tanpa animasi (mati / tidak
    /// terlihat) window langsung ke `t`.
    pub(crate) unsafe fn slide_from(
        &mut self,
        id: WindowId,
        hwnd: HWND,
        t: Rect,
        off: (i32, i32),
        work: Rect,
        now: Instant,
    ) {
        let start = (t.x + off.0, t.y + off.1);
        if self.anim_ms > 0.0 {
            let inset = insets(hwnd);
            let _ = SetWindowPos(
                hwnd,
                None,
                start.0 - inset.0,
                start.1 - inset.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            self.anim.remove(&id); // mulai dari `start`, bukan dari animasi lama
        }
        self.move_only(id, hwnd, start, t, work, now);
    }

    /// Meluncur keluar layar ke sisi `exit_to` (+1 kanan, -1 kiri) selebar area kerja,
    /// dari posisi tujuannya saat ini. True kalau animasi dimulai (window masih tampak);
    /// kalau tidak, window sudah langsung dipindah dan pemanggil boleh memarkirnya.
    pub(crate) unsafe fn slide_out(
        &mut self,
        id: WindowId,
        hwnd: HWND,
        exit_to: i32,
        work: Rect,
        now: Instant,
    ) -> bool {
        let Some(&(l, t, r, b)) = self.actual.get(&id) else {
            return false;
        };
        let target = Rect {
            x: l + exit_to * work.w,
            y: t,
            w: r - l,
            h: b - t,
        };
        let cur = ext_bounds(hwnd);
        self.move_only(id, hwnd, (cur.left, cur.top), target, work, now);
        self.anim.contains_key(&id)
    }

    /// Isyarat tukar posisi window penuh lebar (lihat `Strip::take_move_cue`): window
    /// yang tetap di tempat luncur masuk dari arah perpindahannya. Tidak dilakukan kalau
    /// animasi mati atau window itu sudah bergerak sendiri (sudah kelihatan).
    pub(crate) unsafe fn cue_slide(&mut self, id: WindowId, dir: Dir, work: Rect, now: Instant) {
        if self.anim_ms <= 0.0 || self.anim.contains_key(&id) {
            return;
        }
        let (Some(&hwnd), Some(&(l, t, r, b))) = (self.map.get(&id), self.actual.get(&id)) else {
            return;
        };
        let rect = Rect {
            x: l,
            y: t,
            w: r - l,
            h: b - t,
        };
        self.slide_from(id, hwnd, rect, cue_offset(dir, work), work, now);
    }

    /// Satu frame animasi: semua window yang bergerak dipindah dalam satu batch.
    pub(crate) unsafe fn tick_anim(&mut self) {
        if self.anim.is_empty() {
            self.stop_anim_timer();
            return;
        }
        let now = Instant::now();
        let Ok(mut hdwp) = BeginDeferWindowPos(self.anim.len() as i32) else {
            self.finish_all_anims();
            return;
        };
        let mut done = Vec::new();
        let mut ok = true;
        for (id, a) in &self.anim {
            let ((x, y), finished) = a.pos_at(now);
            match DeferWindowPos(
                hdwp,
                a.hwnd,
                None,
                x - a.inset.0,
                y - a.inset.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) {
                Ok(next) => hdwp = next,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
            if finished {
                done.push(*id);
            }
        }
        if !ok {
            self.finish_all_anims(); // mis. window sudah ditutup: langsung ke akhir
            return;
        }
        let _ = EndDeferWindowPos(hdwp);
        for id in done {
            if let Some(a) = self.anim.remove(&id) {
                self.actual.insert(id, bounds(ext_bounds(a.hwnd)));
            }
            if self.leaving.remove(&id) {
                self.finish_park(id); // selesai meluncur keluar: parkir sungguhan
            }
        }
        if self.anim.is_empty() {
            self.stop_anim_timer();
        }
    }

    /// Lompat semua animasi ke posisi akhir (jalur darurat).
    pub(crate) unsafe fn finish_all_anims(&mut self) {
        for (id, a) in std::mem::take(&mut self.anim) {
            let _ = SetWindowPos(
                a.hwnd,
                None,
                a.to.0 - a.inset.0,
                a.to.1 - a.inset.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            self.actual.insert(id, bounds(ext_bounds(a.hwnd)));
        }
        for id in std::mem::take(&mut self.leaving) {
            self.finish_park(id);
        }
        self.stop_anim_timer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn cue_starts_against_the_direction_of_travel() {
        assert_eq!(cue_offset(Dir::Down, WORK), (0, -216)); // dipindah ke bawah: datang dari atas
        assert_eq!(cue_offset(Dir::Up, WORK), (0, 216));
        assert_eq!(cue_offset(Dir::Right, WORK), (-384, 0));
        assert_eq!(cue_offset(Dir::Left, WORK), (384, 0));
    }
}
