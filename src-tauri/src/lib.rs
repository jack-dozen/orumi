mod apps;
mod edge;
mod media;
mod stats;
mod actions;
mod winlist;
mod overlay;
mod brightness;
mod gpu;
mod osd;
mod wifi;
mod battery;
mod theme;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use edge::{Edge, PanelSpec, Panels};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Daftar semua panel. Overlay nanti tinggal ditambah di sini.
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
    // Harus plugin pertama. Instance kedua langsung keluar; instance pertama diam saja.
    .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .invoke_handler(tauri::generate_handler![
            edge::toggle_panel,
            edge::set_panel,
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
            actions::run_action,
            winlist::list_windows,
            winlist::focus_window,
            winlist::focus_window,
            winlist::minimize_window,
            overlay::overlay_toggle,
            overlay::overlay_close,
            overlay::get_volume,
            overlay::set_volume,
            overlay::set_volume,
            overlay::toggle_mute,
            brightness::get_brightness,
            brightness::set_brightness
        ])
        .setup(|app| {
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
                window.set_zoom(edge::UI_SCALE)?;
                if spec.hover && !spec.focus {
                    edge::set_no_activate(&window); // bar dan notch
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

            // --- Hotkey global ---
            let ctrl_alt = Modifiers::CONTROL | Modifiers::ALT;
            let bar_key = Shortcut::new(Some(ctrl_alt), Code::KeyB);
            let notch_key = Shortcut::new(Some(ctrl_alt), Code::KeyN);
            let launcher_key = Shortcut::new(Some(ctrl_alt), Code::KeyJ);
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        let panels = app.state::<Panels>();
                        if shortcut == &bar_key {
                            panels.toggle("bar");
                        } else if shortcut == &notch_key {
                            panels.toggle("notch");
                        } else if shortcut == &launcher_key {
                            panels.toggle("launcher");
                        }
                    })
                    .build(),
            )?;
            for (key, name) in [
                (bar_key, "Ctrl+Alt+B"),
                (notch_key, "Ctrl+Alt+N"),
                (launcher_key, "Ctrl+Alt+J"),
            ] {
                if let Err(e) = app.global_shortcut().register(key) {
                    eprintln!("Hotkey {name} gagal didaftarkan (mungkin dipakai app lain): {e}");
                }
            }

            // --- Tray icon ---
            let bar_item =
                MenuItem::with_id(app, "toggle_bar", "Tampilkan / sembunyikan bar", true, None::<&str>)?;
            let notch_item =
                MenuItem::with_id(app, "toggle_notch", "Tampilkan / sembunyikan notch", true, None::<&str>)?;
            let launcher_item =
                MenuItem::with_id(app, "toggle_launcher", "Buka launcher", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Keluar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&bar_item, &notch_item, &launcher_item, &quit_item])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("ikon app tidak ada").clone())
                .tooltip("edgeshell")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle_bar" => app.state::<Panels>().toggle("bar"),
                    "toggle_notch" => app.state::<Panels>().toggle("notch"),
                    "toggle_launcher" => app.state::<Panels>().toggle("launcher"),
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
