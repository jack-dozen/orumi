//! Activities: panel dari tepi bawah berisi workspace dan app grid (pengganti start menu).
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

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::{
    edge::{Edge, PanelSpec},
    winlist,
};
use workspaces::{Workspace, WorkspaceState};

/// Label window di tauri.conf.json.
const LABEL: &str = "activities";
/// Lamanya animasi tutup di sisi React sebelum window benar-benar disembunyikan.
const CLOSE_ANIM: Duration = Duration::from_millis(320);
/// Ukuran panel (satuan desain, sama dengan PanelSpec.size panel lain): paling besar segini,
/// dan tidak melebihi bagian layar berikut (lebar, tinggi).
const PANEL_MAX: (f64, f64) = (1600.0, 860.0);
const PANEL_FRACTION: (f64, f64) = (0.92, 0.84);

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

/// Tutup karena Win ditekan selagi Activities terbuka (lihat winkey.rs). Memakai animasi yang
/// sama dengan `hide`. Kalau Start menu kembali lolos saat Win ditekan beruntun, ganti isinya
/// dengan penyembunyian langsung tanpa menunggu `CLOSE_ANIM`.
pub fn dismiss(app: &AppHandle) {
    hide(app);
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
            place_panel(&window);
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

/// Taruh window sebagai panel di tepi bawah, di tengah, dengan cara yang sama seperti panel
/// lain (`edge::place`) dan mengikuti skala UI utama. Dihitung ulang tiap dibuka (resolusi,
/// DPI, atau skala bisa berubah), jadi perubahan skala berlaku pada pembukaan berikutnya.
fn place_panel(window: &WebviewWindow) {
    let scale = crate::edge::ui_scale();
    let _ = window.set_zoom(scale);
    let Ok(Some(m)) = window.primary_monitor() else {
        return;
    };
    // 1 satuan desain = u pixel fisik.
    let u = m.scale_factor() * scale;
    let (mw, mh) = (m.size().width as f64 / u, m.size().height as f64 / u);
    let spec = PanelSpec {
        label: LABEL,
        edge: Edge::Bottom,
        size: (
            (mw * PANEL_FRACTION.0).min(PANEL_MAX.0),
            (mh * PANEL_FRACTION.1).min(PANEL_MAX.1),
        ),
        trigger_len: None,
        hover: false,
        focus: true,
    };
    let _ = crate::edge::place(window, &spec);
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
