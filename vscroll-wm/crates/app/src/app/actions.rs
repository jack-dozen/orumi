//! Aksi atas window aktif: tutup, minimize, float, jeda, dan info.

use std::sync::atomic::Ordering;

use layout::WindowId;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use super::App;
use crate::winutil::{title, unmanaged_reason, wid};
use crate::{bindings, cheatsheet, osd, rules, safety, taskbar, tray};

/// Tampilkan masalah konfigurasi di kotak pesan (build release tidak punya console).
/// Jalan di thread terpisah supaya message loop tidak terblokir. Tidak melakukan
/// apa-apa kalau tidak ada masalah.
pub(crate) fn show_config_problems(report: &crate::config::LoadReport) {
    if report.problems.is_empty() {
        return;
    }
    let file = report
        .path
        .as_ref()
        .map_or_else(|| "configuration".to_string(), |p| p.display().to_string());
    let list: Vec<String> = report.problems.iter().map(|p| format!("  - {p}")).collect();
    let text = format!(
        "{} problem(s) in {file}:\n\n{}\n\nThe affected settings keep their defaults.\nFix the file, then use the tray menu: Reload configuration.",
        report.problems.len(),
        list.join("\n")
    );
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    std::thread::spawn(move || unsafe {
        MessageBoxW(
            None,
            PCWSTR(wide.as_ptr()),
            w!("vscroll-wm: configuration problems"),
            MB_OK | MB_ICONWARNING,
        );
    });
}

impl App {
    /// Win+Q: tutup window aktif (setara Alt+F4). Hanya window yang kita kenal,
    /// supaya desktop / taskbar tidak ikut terkirim perintah tutup.
    pub(crate) unsafe fn close_active(&self) {
        let fg = GetForegroundWindow();
        let id = wid(fg);
        if self.map.contains_key(&id) || self.floating.contains_key(&id) {
            let _ = PostMessageW(
                Some(fg),
                WM_SYSCOMMAND,
                WPARAM(SC_CLOSE as usize),
                LPARAM(0),
            );
        }
    }

    /// Win+M: minimize window aktif saja. Fokus pindah ke tetangganya di strip,
    /// bukan ke window pilihan Windows.
    pub(crate) unsafe fn minimize_active(&mut self) {
        let fg = GetForegroundWindow();
        let id = wid(fg);
        let tiled = self.map.contains_key(&id);
        if !tiled && !self.floating.contains_key(&id) {
            return;
        }
        if tiled {
            self.ws.set_hidden(id, true); // fokus pindah ke tetangga
            self.hidden.insert(id);
            self.anim.remove(&id);
        }
        let _ = ShowWindow(fg, SW_MINIMIZE);
        if tiled {
            if let Some(h) = self.ws.cur().focused().and_then(|f| self.map.get(&f).copied()) {
                let _ = SetForegroundWindow(h);
                self.last_fg = Some(wid(h));
            }
            self.apply();
        }
    }

    /// Jeda / lanjutkan (menu tray). Saat dijeda semua window dikembalikan ke
    /// posisi asli dan hotkey diteruskan ke Windows.
    pub(crate) unsafe fn toggle_pause(&mut self) {
        let paused = !tray::PAUSED.load(Ordering::Relaxed);
        tray::PAUSED.store(paused, Ordering::Relaxed);
        tray::set_tip(if paused {
            "vscroll-wm (paused)"
        } else {
            "vscroll-wm"
        });
        if paused {
            self.close_overview(false);
            cheatsheet::hide();
            osd::hide();
            self.finish_all_anims();
            self.stop_sync_timer();
            safety::restore_all(); // juga mengembalikan warna border
            taskbar::show_all();
            self.parked.clear();
            self.leaving.clear();
            self.border_applied = None;
            self.publish_ipc();
        } else {
            self.sync();
            self.apply();
            self.update_workspace_tip();
        }
    }

    /// Win+Alt+I: tampilkan class, exe, dan title window yang sedang aktif, plus
    /// alasan ditata atau tidaknya. Berguna untuk menulis aturan di vscroll-wm.ignore.
    pub(crate) unsafe fn inspect(&self) {
        let h = GetForegroundWindow();
        let id = wid(h);
        let class = rules::class_name(h);
        let title = rules::window_title(h);
        let exe = rules::exe_name(h).unwrap_or_else(|| "(unreadable)".into());
        let status = if self.floating.contains_key(&id) {
            format!("floating (toggle with {})", bindings::active_label("float"))
        } else if self.blocked.contains_key(&id) {
            "cannot be controlled: probably running as administrator \
             (run vscroll-wm as administrator too)"
                .to_string()
        } else if self.map.contains_key(&id) {
            "tiled".to_string()
        } else {
            match unmanaged_reason(h) {
                Some(why) => format!("not tiled: {why}"),
                None => "not tiled yet (waiting for sync)".to_string(),
            }
        };
        let text = format!(
            "title : {title}\nclass : {class}\nexe   : {exe}\nstatus: {status}\n\nLines for vscroll-wm.ignore:\n  class:{class}\n  exe:{exe}\n  title:{title}"
        );
        println!("---------- window aktif ----------\n{text}\n----------------------------------");
        if !cfg!(debug_assertions) {
            // Build release tidak punya console: tampilkan di kotak pesan, di thread
            // terpisah supaya message loop tidak terblokir.
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            std::thread::spawn(move || {
                MessageBoxW(
                    None,
                    PCWSTR(wide.as_ptr()),
                    w!("vscroll-wm: window info"),
                    MB_OK | MB_ICONINFORMATION,
                );
            });
        }
    }

    /// Window menolak dipindahkan karena hak aksesnya lebih tinggi dari kita (biasanya
    /// berjalan sebagai administrator; UIPI). Keluarkan dari strip supaya tidak
    /// memegang slot kosong yang tidak pernah bisa diisi.
    pub(crate) unsafe fn mark_blocked(&mut self, id: WindowId, h: HWND) {
        self.ws.remove_window(id);
        self.map.remove(&id);
        self.hidden.remove(&id);
        self.actual.remove(&id);
        self.anim.remove(&id);
        safety::forget(h);
        self.parked.remove(&id);
        self.leaving.remove(&id);
        taskbar::show(h);
        self.blocked.insert(id, h);
        eprintln!(
            "[akses ditolak] \"{}\" tidak bisa dikendalikan (kemungkinan berjalan sebagai administrator)",
            title(h)
        );
    }

    /// Win+Alt+F: window yang sedang di foreground dikeluarkan dari strip
    /// (mengambang, bebas digeser/di-resize), atau dimasukkan kembali.
    pub(crate) unsafe fn toggle_float(&mut self) {
        let fg = GetForegroundWindow();
        let id = wid(fg);
        if self.floating.remove(&id).is_some() {
            println!("tile: {}", title(fg)); // masuk strip lagi lewat sync berikutnya
            return;
        }
        let Some(h) = self.map.remove(&id) else {
            return;
        };
        self.ws.remove_window(id);
        self.hidden.remove(&id);
        self.actual.remove(&id);
        if let Some(a) = self.anim.remove(&id) {
            // selesaikan gerakan dulu supaya berhenti di posisi akhirnya
            let _ = SetWindowPos(
                h,
                None,
                a.to.0 - a.inset.0,
                a.to.1 - a.inset.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        safety::forget(h); // tidak ikut dipindahkan saat app keluar
        self.floating.insert(id, h);
        println!("float: {}", title(h));
    }
}
