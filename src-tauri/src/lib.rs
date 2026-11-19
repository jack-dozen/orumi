mod actions;
mod apps;
mod battery;
mod bluetooth;
mod brightness;
mod config;
mod edge;
mod gpu;
mod media;
mod notifications;
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
mod quick;
mod taskbar;
mod icons;
mod previews;
mod capture;
mod visualizer;
mod wm_ipc;

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

/// List of all panels.
const PANELS: [PanelSpec; 7] = [
    PanelSpec {
        label: "bar",
        edge: Edge::Left,
        size: (64.0, 0.0),
        trigger_len: None,
        hover: true,
        focus: false,
    },
    PanelSpec {
        label: "overview",
        edge: Edge::Left,
        size: (480.0, 0.0),
        trigger_len: None,
        hover: false,
        focus: true,
    },
    PanelSpec {
        label: "notch",
        edge: Edge::Top,
        size: (860.0, 460.0),
        trigger_len: Some(240.0),
        hover: true,
        focus: false,
    },
    PanelSpec {
        label: "launcher",
        edge: Edge::Bottom,
        size: (1080.0, 520.0),
        trigger_len: Some(360.0),
        hover: false,
        focus: true,
    },
    PanelSpec {
        label: "overlay",
        edge: Edge::Right,
        size: (160.0, 440.0),
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
    PanelSpec {
        label: "preview",
        edge: Edge::Free,
        size: (0.0, 0.0),
        trigger_len: None,
        hover: true, // use auto-close behavior for panels opened by clicking
        focus: false,
    },
];

/// Apply size, position, and zoom to all panels. Called at startup and when the scale changes.
pub fn apply_layout(app: &AppHandle) {
    for spec in PANELS {
        if let Some(window) = app.get_webview_window(spec.label) {
            let _ = edge::place(&window, &spec);
            let _ = window.set_zoom(edge::ui_scale());
        }
    }
}

/// Reposition one panel (e.g. after its size changes).
pub fn place_panel(app: &AppHandle, label: &str) {
    if let Some(spec) = PANELS.iter().find(|s| s.label == label) {
        if let Some(window) = app.get_webview_window(label) {
            let _ = edge::place(&window, spec);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Launch as the recovery watchdog (see taskbar.rs): no UI and no Tauri.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--guard") {
        taskbar::run_guard(&args);
        return;
    }

    tauri::Builder::default()
        // Must be the first plugin. A second instance exits immediately; the first stays idle.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("orumi-state")
                .setup(|app, _api| {
                    let cfg = config::load(app);
                    app.manage(config::ConfigState::new(cfg));
                    app.manage(stats::StatsState::new());
                    app.manage(stats::ProcState::new());
                    app.manage(gpu::GpuState::new());
                    app.manage(apps::AppsState::new());
                    app.manage(notifications::NotifState::new());
                    app.manage(bluetooth::BtState::new());
                    let flags: HashMap<_, _> = PANELS
                        .iter()
                        .map(|s| (s.label, Arc::new(AtomicBool::new(false))))
                        .collect();
                    app.manage(Panels(flags));
                    Ok(())
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            edge::toggle_panel,
            edge::set_panel,
            edge::panel_activation,
            edge::launcher_view,
            edge::launcher_set_size,
            edge::notch_set_height,
            edge::notch_set_toast_rect,
            edge::notch_set_lyrics_pin,
            notifications::notif_list,
            notifications::notif_mark_read,
            notifications::notif_clear_all,
            notifications::notif_open,
            activities::activities_startup_begin,
            activities::activities_startup_end,
            activities::activities_toggle,
            activities::activities_close,
            activities::ws_list,
            activities::ws_switch,
            activities::ws_move_window,
            altswitch::switcher_pick,
            altswitch::switcher_open,
            altswitch::switcher_set_previews,
            altswitch::switcher_get_previews,
            altswitch::dev_log,
            tray_flyout::tray_toggle,
            icons::app_icon,
            previews::preview_open,
            previews::preview_keep,
            previews::preview_close_soon,
            previews::preview_close,
            config::get_config,
            config::set_config,
            stats::get_stats,
            stats::get_top_processes,
            gpu::get_gpu,
            wifi::get_wifi,
            wifi::wifi_list,
            wifi::wifi_scan,
            wifi::wifi_connect,
            wifi::wifi_disconnect,
            wifi::wifi_forget,
            wifi::open_settings_page,
            bluetooth::bt_list,
            bluetooth::bt_connect,
            bluetooth::bt_disconnect,
            bluetooth::bt_unpair,
            bluetooth::bt_scan_start,
            bluetooth::bt_scan_stop,
            bluetooth::bt_pair,
            bluetooth::bt_pair_respond,
            quick::quick_get,
            quick::quick_set,
            quick::cycle_power_mode,
            battery::get_battery,
            theme::wallpaper_stamp,
            theme::wallpaper_pixels,
            theme::set_theme,
            theme::get_theme,
            theme::mica_backdrop,
            theme::get_mica,
            wallpaper::list_wallpapers,
            wallpaper::set_wallpaper,
            wallpaper::run_wallpaper_command,
            media::get_media,
            media::media_control,
            media::media_seek,
            visualizer::set_visualizer_active,
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
            overlay::list_audio_outputs,
            overlay::set_audio_output,
            brightness::get_brightness,
            brightness::set_brightness,
            wm_ipc::wm_cheatsheet_toggle,
            wm_ipc::wm_cheatsheet_groups,
            wm_ipc::wm_overview_toggle,
            wm_ipc::wm_overview_state,
            wm_ipc::wm_overview_drop,
            wm_ipc::wm_overview_pick,
            wm_ipc::wm_config_get,
            wm_ipc::wm_config_save,
            wm_ipc::wm_toggle_pause_cmd,
            wm_ipc::wm_reapply_cmd,
            wm_ipc::wm_setup_elevated
        ])
        .setup(|app| {
            let cfg = config::current(app.handle());
            edge::set_ui_scale(cfg.ui_scale);
            edge::set_panel_prefs(cfg.panels.clone());
            capture::clear_cache(app.handle());
            taskbar::spawn_guard(app.handle());
            taskbar::apply(app.handle(), cfg.taskbar_autohide, cfg.taskbar_hidden);
            if !cfg!(dev) && cfg.autostart {
                let _ = config::apply_autostart(true);
            }

            // --- All panels ---
            let mut watched = Vec::new();
            for spec in PANELS {
                let window = app
                    .get_webview_window(spec.label)
                    .unwrap_or_else(|| panic!("window '{}' tidak ada di tauri.conf.json", spec.label));
                let pinned = app.state::<Panels>().0[spec.label].clone();

                edge::place(&window, &spec)?;
                window.set_zoom(edge::ui_scale())?;
                if (spec.hover && !spec.focus) || spec.label == "switcher" {
                    edge::set_activatable(&window, false); // bar, notch, and switcher
                }
                window.set_ignore_cursor_events(true)?; // initially click-through
                watched.push((window, spec, pinned));
            }
            edge::start_all(app.handle().clone(), watched);

            overlay::start_volume_watcher(app.handle().clone());
            brightness::start_watcher(app.handle().clone());
            osd::start();
            notifications::start(app.handle().clone());
            visualizer::start(app.handle().clone());
            wifi::start_notifications(app.handle().clone());
            winlist::start_events(app.handle().clone());
            wallpaper::warm(app.handle().clone());
            wm_ipc::start(app.handle().clone());
            activities::init(app.handle())?;
            activities::set_win_key_enabled(cfg.panels.activities);
            // Install the swallowing Alt+Tab hook last so it is first in the low-level hook chain.
            altswitch::set_enabled(cfg.alt_tab && cfg.panels.switcher);
            altswitch::start(app.handle().clone());

            // --- Global hotkeys (from config) ---
            config::init_hotkeys(app.handle(), &cfg.hotkeys)?;

            // --- Tray icon ---
            let bar_item =
                MenuItem::with_id(app, "toggle_bar", "Show / hide bar", true, None::<&str>)?;
            let notch_item =
                MenuItem::with_id(app, "toggle_notch", "Show / hide notch", true, None::<&str>)?;
            let launcher_item =
                MenuItem::with_id(app, "toggle_launcher", "Open launcher", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let alt_item = MenuItem::with_id(
                app,
                "toggle_alt_tab",
                "Toggle custom Alt+Tab",
                true,
                None::<&str>,
            )?;
            let wm_pause_item = MenuItem::with_id(
                app,
                "wm_toggle_pause",
                "Pause / resume tiling WM",
                true,
                None::<&str>,
            )?;
            let wm_reload_item = MenuItem::with_id(
                app,
                "wm_reload_config",
                "Reload WM config",
                true,
                None::<&str>,
            )?;
            let wm_cheatsheet_item = MenuItem::with_id(
                app,
                "wm_cheatsheet",
                "WM keyboard shortcuts",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &bar_item,
                    &notch_item,
                    &launcher_item,
                    &settings_item,
                    &alt_item,
                    &wm_pause_item,
                    &wm_reload_item,
                    &wm_cheatsheet_item,
                    &quit_item,
                ],
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
                        // App settings now live in the launcher (the notch settings tab is quick settings).
                        let _ = app.emit_to("launcher", "launcher-view", "settings");
                        app.state::<Panels>().set_transient("launcher");
                    }
                    "toggle_alt_tab" => config::toggle_alt_tab(app),
                    "wm_toggle_pause" => wm_ipc::toggle_pause(),
                    "wm_reload_config" => wm_ipc::reload_config(),
                    "wm_cheatsheet" => wm_ipc::toggle_cheatsheet_ui(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Restore tiled windows (if spawned by ORUMI), Windows flyout, and taskbar on normal exit.
            if let tauri::RunEvent::Exit = event {
                wm_ipc::shutdown();
                notifications::restore(app);
                osd::restore();
                taskbar::restore(app);
            }
        });
}
