//! Activities: overlay layar penuh berisi workspace dan app grid (pengganti start menu).
//!
//! Seluruh logikanya hidup di folder ini supaya tidak bercampur dengan kode panel lain.
//! File lama hanya perlu tahu tiga hal (lihat CHANGES.md): `mod activities;`, daftar command
//! di `generate_handler!`, dan satu panggilan `activities::init(...)` di `setup`.
//!
//! Window "activities" TIDAK masuk `PANELS`/edge.rs. Ia dikelola sendiri di sini: tidak ada
//! pemicu tepi layar, hanya Win key (winkey.rs) atau command `activities_toggle`.

mod thumbs;
mod winkey;
mod workspaces;

use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

use crate::winlist;
use workspaces::{Workspace, WorkspaceState};

/// Label window di tauri.conf.json.
const LABEL: &str = "activities";
/// Lamanya animasi tutup di sisi React sebelum window benar-benar disembunyikan.
const CLOSE_ANIM: Duration = Duration::from_millis(220);

/// true = Activities sedang terbuka (atau sedang menyiapkan snapshot untuk dibuka).
static OPEN: AtomicBool = AtomicBool::new(false);

/// Dipanggil sekali dari `setup` di lib.rs.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    app.manage(WorkspaceState::new());
    if app.get_webview_window(LABEL).is_none() {
        eprintln!("[activities] window '{LABEL}' tidak ada di tauri.conf.json; fitur dimatikan");
        return Ok(());
    }
    winkey::start(app.clone());
    Ok(())
}

/// Nyalakan/matikan pemicu Win key (untuk dihubungkan ke config nanti).
#[allow(dead_code)]
pub fn set_win_key_enabled(on: bool) {
    winkey::set_enabled(on);
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// Tutup SEKARANG tanpa animasi: dipakai saat Win ditekan selagi Activities terbuka, agar
/// fokus sudah pindah sebelum Win dilepas (lihat winkey.rs).
pub fn dismiss(app: &AppHandle) {
    if !OPEN.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit_to(LABEL, "activities-close", ());
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if OPEN.load(Ordering::SeqCst) {
            return;
        }
        if let Some(window) = a.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

pub fn toggle(app: &AppHandle) {
    if OPEN.load(Ordering::SeqCst) {
        hide(app);
    } else {
        show(app);
    }
}

/// Ambil snapshot window (dengan thumbnail) DULU, baru tampilkan window Activities.
/// Dengan begitu Activities tidak ikut tertangkap dan tidak ada kedipan isi lama.
pub fn show(app: &AppHandle) {
    if OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || {
        let windows = thumbs::snapshot(winlist::collect_windows());
        let payload = serde_json::to_value(&windows).unwrap_or_default();

        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            // Sudah ditutup lagi selama snapshot diambil.
            if !OPEN.load(Ordering::SeqCst) {
                return;
            }
            let Some(window) = a.get_webview_window(LABEL) else {
                OPEN.store(false, Ordering::SeqCst);
                return;
            };
            fit_to_monitor(&window);
            let _ = window.show();
            let _ = window.set_always_on_top(true);
            let _ = window.set_focus();
            let _ = a.emit_to(LABEL, "activities-open", payload);
        });
    });
}

pub fn hide(app: &AppHandle) {
    if !OPEN.swap(false, Ordering::SeqCst) {
        return;
    }
    // React memainkan animasi tutup; window disembunyikan setelahnya.
    let _ = app.emit_to(LABEL, "activities-close", ());
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(CLOSE_ANIM);
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            // Dibuka lagi selama animasi tutup: biarkan.
            if OPEN.load(Ordering::SeqCst) {
                return;
            }
            if let Some(window) = a.get_webview_window(LABEL) {
                let _ = window.hide();
            }
        });
    });
}

/// Window memenuhi monitor utama. Dihitung ulang tiap dibuka (resolusi/dock bisa berubah).
fn fit_to_monitor(window: &WebviewWindow) {
    if let Ok(Some(m)) = window.primary_monitor() {
        let _ = window.set_position(PhysicalPosition::new(m.position().x, m.position().y));
        let _ = window.set_size(PhysicalSize::new(m.size().width, m.size().height));
    }
}

// ---------- Command untuk React ----------

#[tauri::command]
pub fn activities_toggle(app: AppHandle) {
    toggle(&app);
}

#[tauri::command]
pub fn activities_close(app: AppHandle) {
    hide(&app);
}

#[tauri::command]
pub fn ws_list(state: State<'_, WorkspaceState>) -> Vec<Workspace> {
    state.0.list()
}

#[tauri::command]
pub fn ws_switch(id: String, state: State<'_, WorkspaceState>) -> Result<(), String> {
    state.0.switch(&id)
}

#[tauri::command]
pub fn ws_move_window(hwnd: i64, id: String, state: State<'_, WorkspaceState>) -> Result<(), String> {
    state.0.move_window(hwnd, &id)
}
