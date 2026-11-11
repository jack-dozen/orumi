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
mod wallpaper;
mod altswitch;
mod tray_flyout;
mod activities;
mod taskbar;
mod icons;

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
const PANELS: [PanelSpec; 5] = [
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
        size: (1040.0, 520.0),
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
    PanelSpec {
        label: "switcher",
        edge: Edge::Bottom,
        size: (1200.0, 260.0),
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

/// Taruh ulang satu panel (misalnya setelah ukurannya berubah).
pub fn place_panel(app: &AppHandle, label: &str) {
    if let Some(spec) = PANELS.iter().find(|s| s.label == label) {
        if let Some(window) = app.get_webview_window(label) {
            let _ = edge::place(&window, spec);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Peluncuran sebagai penjaga pemulih (lihat taskbar.rs): tanpa UI, tanpa Tauri.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--guard") {
        taskbar::run_guard(&args);
        return;
    }

    tauri::Builder::default()
        // Harus plugin pertama. Instance kedua langsung keluar; instance pertama diam saja.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .invoke_handler(tauri::generate_handler![
            edge::toggle_panel,
            edge::set_panel,
            edge::panel_activation,
            edge::launcher_view,
            activities::activities_toggle,
            activities::activities_close,
            activities::ws_list,
            activities::ws_switch,
            activities::ws_move_window,
            altswitch::switcher_pick,
            tray_flyout::tray_toggle,
            icons::app_icon,
            config::get_config,
            config::set_config,
            stats::get_stats,
            stats::get_top_processes,
            gpu::get_gpu,
            wifi::get_wifi,
            wifi::open_settings_page,
            battery::get_battery,
            theme::wallpaper_stamp,
            theme::wallpaper_pixels,
            theme::set_theme,
            theme::get_theme,
            wallpaper::list_wallpapers,
            wallpaper::set_wallpaper,
            wallpaper::run_wallpaper_command,
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
            taskbar::spawn_guard(app.handle());
            taskbar::apply(app.handle(), cfg.taskbar_autohide);
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
                if (spec.hover && !spec.focus) || spec.label == "switcher" {
                    edge::set_activatable(&window, false); // bar, notch, dan switcher
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
            wallpaper::warm(app.handle().clone());
            altswitch::set_enabled(cfg.alt_tab);
            altswitch::start(app.handle().clone());
            activities::init(app.handle())?;

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
            let alt_item = MenuItem::with_id(
                app,
                "toggle_alt_tab",
                "Nyalakan/matikan Alt+Tab kustom",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Keluar", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&bar_item, &notch_item, &launcher_item, &settings_item, &alt_item, &quit_item],
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
                    "toggle_alt_tab" => config::toggle_alt_tab(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Pulihkan flyout Windows dan taskbar di jalur keluar normal (termasuk menu tray).
            if let tauri::RunEvent::Exit = event {
                osd::restore();
                taskbar::restore(app);
            }
        });
}
