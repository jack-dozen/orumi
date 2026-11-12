//! Activities: a panel from the bottom edge containing workspaces and an app grid (a Start menu replacement).
//!
//! All of its logic lives in this folder to keep it separate from other panel code.
//! The existing files only need three changes (see CHANGES.md): `mod activities;`, the command list
//! in `generate_handler!`, and a call to `activities::init(...)` in `setup`.
//!
//! The "activities" window is NOT part of `PANELS`/edge.rs. It is managed here: there is no
//! screen-edge trigger, only the Win key (winkey.rs) or the `activities_toggle` command.

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
/// Duration of the close animation on the React side before the window is actually hidden.
const CLOSE_ANIM: Duration = Duration::from_millis(320);
/// Panel size (design units, same as other panels' PanelSpec.size): capped at this size
/// and at the following screen dimensions (width, height).
const PANEL_MAX: (f64, f64) = (1600.0, 860.0);
const PANEL_FRACTION: (f64, f64) = (0.92, 0.84);

/// true = Activities is open (or preparing a snapshot to open).
static OPEN: AtomicBool = AtomicBool::new(false);

/// Called once from `setup` in lib.rs.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    app.manage(WorkspaceState::new());
    if app.get_webview_window(LABEL).is_none() {
        eprintln!("[activities] window '{LABEL}' tidak ada di tauri.conf.json; fitur dimatikan");
        return Ok(());
    }
    winkey::start(app.clone());
    Ok(())
}

/// Enable/disable the Win key trigger (to be connected to config later).
#[allow(dead_code)]
pub fn set_win_key_enabled(on: bool) {
    winkey::set_enabled(on);
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// Close when Win is pressed while Activities is open (see winkey.rs). Uses the same animation
/// as `hide`. If the Start menu appears again during repeated Win presses, replace its contents
/// with an immediate hide instead of waiting for `CLOSE_ANIM`.
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

/// Take a snapshot of windows (with thumbnails) BEFORE showing the Activities window.
/// This prevents Activities from being captured and avoids flashing stale contents.
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
            // It was closed again while the snapshot was being taken.
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
    // React plays the close animation; hide the window afterward.
    let _ = app.emit_to(LABEL, "activities-close", ());
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(CLOSE_ANIM);
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            // Reopened during the close animation: leave it open.
            if OPEN.load(Ordering::SeqCst) {
                return;
            }
            if let Some(window) = a.get_webview_window(LABEL) {
                let _ = window.hide();
            }
        });
    });
}

/// Place the window as a panel centered on the bottom edge, like other panels (`edge::place`),
/// and use the main UI scale. Recalculate each time it opens (resolution, DPI, or scale may
/// change) so scale changes take effect the next time it opens.
fn place_panel(window: &WebviewWindow) {
    let scale = crate::edge::ui_scale();
    let _ = window.set_zoom(scale);
    let Ok(Some(m)) = window.primary_monitor() else {
        return;
    };
    // 1 design unit = u physical pixels.
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

// ---------- Commands for React ----------

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
