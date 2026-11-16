//! Ingat layout: simpan susunan workspace ke file, dan pulihkan saat vscroll-wm
//! dijalankan ulang selagi window-window itu masih terbuka (lihat layout_state.rs).
//!
//! Disimpan otomatis dari timer poll (setiap perubahan tersimpan paling lambat sekitar
//! satu detik kemudian) dan sekali lagi saat keluar. Dimatikan lewat `remember_layout = off`.

use std::collections::HashMap;

use layout::WindowId;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::EnumWindows;

use super::App;
use crate::winutil::{collect, wid};
use crate::{layout_state, rules};

/// Identitas window untuk mencocokkan entri tersimpan: `exe|class` (huruf kecil).
unsafe fn identity(h: HWND) -> String {
    format!(
        "{}|{}",
        rules::exe_name(h).unwrap_or_default().to_lowercase(),
        rules::class_name(h)
    )
}

impl App {
    /// Pulihkan susunan tersimpan untuk window yang masih terbuka dan identitasnya cocok.
    /// Dipanggil sekali saat start, sebelum sync() pertama (yang menambahkan sisanya).
    pub(crate) unsafe fn restore_layout(&mut self) {
        if !self.cfg.remember_layout {
            return;
        }
        let Some(path) = layout_state::path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Some((saved, idents)) = layout_state::decode(&text) else {
            eprintln!("layout.state tidak dikenali, diabaikan");
            return;
        };
        let mut list: Vec<HWND> = Vec::new();
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut Vec<HWND> as isize));
        let live: HashMap<WindowId, String> =
            list.iter().map(|&h| (wid(h), identity(h))).collect();
        self.ws.restore(&saved, |id| {
            matches!((live.get(&id), idents.get(&id.0)), (Some(a), Some(b)) if a == b)
        });
        // yang baru dipulihkan belum perlu ditulis lagi
        self.saved_layout = Some(self.ws.snapshot());
    }

    /// Tulis susunan ke file kalau berubah sejak penulisan terakhir.
    pub(crate) unsafe fn save_layout(&mut self) {
        if !self.cfg.remember_layout {
            return;
        }
        let snap = self.ws.snapshot();
        if self.saved_layout.as_ref() == Some(&snap) {
            return;
        }
        let Some(path) = layout_state::path() else {
            return;
        };
        // identitas dihitung sekali per window (membuka proses itu tidak murah)
        let ids: Vec<WindowId> = snap
            .strips
            .iter()
            .flat_map(|s| s.items.iter().map(|it| it.id))
            .collect();
        for id in &ids {
            if !self.idents.contains_key(id) {
                if let Some(&h) = self.map.get(id) {
                    self.idents.insert(*id, identity(h));
                }
            }
        }
        self.idents.retain(|id, _| ids.contains(id));
        let text = layout_state::encode(&snap, |id| self.idents.get(&id).cloned());
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.saved_layout = Some(snap);
        }
    }
}
