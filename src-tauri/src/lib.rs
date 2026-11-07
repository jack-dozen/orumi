mod actions;
mod apps;
mod battery;
mod brightness;
mod config;
mod edge;
mod gpu;
mod media;
mod osd;
mod overlay;
mod stats;
mod theme;
mod wifi;
mod winlist;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use edge::{Edge, PanelSpec, Panels};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

/// Daftar semua panel.
const PANELS: [PanelSpec; 4] = [
    PanelSpec {
        label: "bar",
        edge: Edge::Left,
        size: (64.0, 0.0),
        trigger_len: None,
        hover: true,
        focus: false,
    },
    PanelSpec {
        label: "notch",
        edge: Edge::Top,
        size: (820.0, 380.0),
        trigger_len: Some(240.0),
        hover: true,
        focus: false,
    },
    PanelSpec {
        label: "launcher",
        edge: Edge::Bottom,
        size: (640.0, 520.0),
        trigger_len: None,
        hover: false,
        focus: true,
    },
    PanelSpec {
        label: "overlay",
        edge: Edge::Right,
        size: (150.0, 440.0),
        trigger_len: None,
        hover: false,
        focus: false,
    },
];

/// Terapkan ukuran, posisi, dan zoom semua panel. Dipanggil saat startup dan saat skala berubah.
pub fn apply_layout(app: &AppHandle) {
    for spec in PANELS {
        if let Some(window) = app.get_webview_window(spec.label) {
            let _ = edge::place(&window, &spec);
            let _ = window.set_zoom(edge::ui_scale());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Harus plugin pertama. Instance kedua langsung keluar; instance pertama diam saja.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .invoke_handler(tauri::generate_handler![
            edge::toggle_panel,
            edge::set_panel,
            edge::panel_activation,
            config::get_config,
            config::set_config,
            stats::get_stats,
            stats::get_top_processes,
            gpu::get_gpu,
            wifi::get_wifi,
            wifi::open_settings_page,
            battery::get_battery,
            theme::wallpaper_stamp,
            theme::wallpaper_image,
            theme::set_theme,
            theme::get_theme,
            media::get_media,
            media::media_control,
            media::media_seek,
            apps::list_apps,
            apps::launch_app,
            actions::run_action,
            winlist::list_windows,
            winlist::focus_window,
            winlist::minimize_window,
            overlay::overlay_toggle,
            overlay::overlay_close,
            overlay::get_volume,
            overlay::set_volume,
            overlay::toggle_mute,
            brightness::get_brightness,
            brightness::set_brightness
        ])
        .setup(|app| {
            let cfg = config::load(app.handle());
            edge::set_ui_scale(cfg.ui_scale);
            app.manage(config::ConfigState::new(cfg.clone()));

            app.manage(stats::StatsState::new());
            app.manage(stats::ProcState::new());
            app.manage(gpu::GpuState::new());
            app.manage(apps::AppsState::new());

            // --- Semua panel ---
            let mut flags = HashMap::new();
            let mut watched = Vec::new();
            for spec in PANELS {
                let window = app
                    .get_webview_window(spec.label)
                    .unwrap_or_else(|| panic!("window '{}' tidak ada di tauri.conf.json", spec.label));
                let pinned = Arc::new(AtomicBool::new(false));
                flags.insert(spec.label, pinned.clone());

                edge::place(&window, &spec)?;
                window.set_zoom(edge::ui_scale())?;
                if spec.hover && !spec.focus {
                    edge::set_activatable(&window, false); // bar dan notch
                }
                window.set_ignore_cursor_events(true)?; // awalnya click-through
                watched.push((window, spec, pinned));
            }
            edge::start_all(app.handle().clone(), watched);
            app.manage(Panels(flags));

            overlay::start_volume_watcher(app.handle().clone());
            brightness::start_watcher(app.handle().clone());
            osd::start();
            wifi::start_notifications(app.handle().clone());
            winlist::start_events(app.handle().clone());

            // --- Hotkey global (dari config) ---
            config::init_hotkeys(app.handle(), &cfg.hotkeys)?;

            // --- Tray icon ---
            let bar_item =
                MenuItem::with_id(app, "toggle_bar", "Tampilkan / sembunyikan bar", true, None::<&str>)?;
            let notch_item =
                MenuItem::with_id(app, "toggle_notch", "Tampilkan / sembunyikan notch", true, None::<&str>)?;
            let launcher_item =
                MenuItem::with_id(app, "toggle_launcher", "Buka launcher", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Pengaturan", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Keluar", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&bar_item, &notch_item, &launcher_item, &settings_item, &quit_item],
            )?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("ikon app tidak ada").clone())
                .tooltip("edgeshell")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle_bar" => app.state::<Panels>().toggle("bar"),
                    "toggle_notch" => app.state::<Panels>().toggle("notch"),
                    "toggle_launcher" => app.state::<Panels>().toggle("launcher"),
                    "settings" => {
                        let _ = app.emit_to("notch", "notch-tab", "settings/app");
                        app.state::<Panels>().set_transient("notch");
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Kembalikan flyout Windows di jalur keluar mana pun (termasuk menu tray).
            if let tauri::RunEvent::Exit = event {
                osd::restore();
            }
        });
}
