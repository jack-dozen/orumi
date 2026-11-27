//! App configuration: a JSON file in the config folder, plus configurable global hotkeys.

use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::edge::Panels;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Hotkeys {
    pub bar: String,
    pub notch: String,
    pub launcher: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            bar: "Ctrl+Alt+B".into(),
            notch: "Ctrl+Alt+N".into(),
            launcher: "Ctrl+Alt+J".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ColorSlot {
    /// "auto" (from the wallpaper), "windows" (follow Windows theme/accent), or "custom".
    pub mode: String,
    /// Color used when mode = "custom", in #rrggbb format.
    pub value: String,
}

impl Default for ColorSlot {
    fn default() -> Self {
        slot("#000000")
    }
}

fn slot(value: &str) -> ColorSlot {
    ColorSlot { mode: "auto".into(), value: value.into() }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Colors {
    /// Panel background (bar, notch, launcher, overlay).
    pub bg0: ColorSlot,
    /// Card background.
    pub bg1: ColorSlot,
    /// Text and icons.
    pub fg: ColorSlot,
    pub accent: ColorSlot,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            bg0: slot("#101418"),
            bg1: slot("#272a2f"),
            fg: slot("#e0e2e8"),
            accent: slot("#a8c8ff"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PanelPref {
    pub enabled: bool,
    /// "left" | "right" | "top" | "bottom"
    pub edge: String,
    pub hotedge: bool,
}

impl Default for PanelPref {
    fn default() -> Self {
        Self { enabled: true, edge: "left".into(), hotedge: false }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PanelsConfig {
    pub bar: PanelPref,
    pub notch: PanelPref,
    pub launcher: PanelPref,
    pub overlay: PanelPref,
    pub overview: bool,
    pub switcher: bool,
    pub activities: bool,
}

impl Default for PanelsConfig {
    fn default() -> Self {
        Self {
            bar: PanelPref { enabled: true, edge: "left".into(), hotedge: true },
            notch: PanelPref { enabled: true, edge: "top".into(), hotedge: true },
            launcher: PanelPref { enabled: true, edge: "bottom".into(), hotedge: true },
            overlay: PanelPref { enabled: true, edge: "right".into(), hotedge: true },
            overview: true,
            switcher: true,
            activities: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub ui_scale: f64,
    pub weather_city: String,
    /// 0-100: how strongly the wallpaper color tints the background and text in "auto" mode.
    pub bg_tint: u8,
    pub colors: Colors,
    /// Wallpaper folder for the built-in picker. Empty = Pictures folder.
    pub wallpaper_dir: String,
    /// "builtin" (launcher picker) or "external" (run the command below).
    pub wallpaper_switcher: String,
    pub wallpaper_command: String,
    /// Most recently applied wallpaper from this app. Set by the backend only.
    pub last_wallpaper: String,
    /// true = replace Alt+Tab with the custom switcher; false = use the Windows default.
    pub alt_tab: bool,
    /// true = intercept Precision Touchpad 3/4-finger gestures via Raw Input HID.
    pub touchpad_gestures: bool,
    /// "activities" or "overview": what a 3-finger swipe up opens.
    pub touchpad_swipe_up: String,
    /// true = auto-hide the Windows taskbar while edgeshell runs (restored on exit).
    pub taskbar_autohide: bool,
    /// true = hide the Windows taskbar completely (takes precedence over `taskbar_autohide`).
    pub taskbar_hidden: bool,
    /// "icons" or "dots": how the open-app list appears in the bar.
    pub bar_app_style: String,
    /// "material" (Material You + Material Symbols Rounded) or "fluent" (Windows 11 Fluent + Segoe Fluent Icons).
    pub ui_style: String,
    /// Enable blurred wallpaper Mica backdrop behind ORUMI panels (works with both UI styles).
    pub mica: bool,
    /// Enable Caelestia-style screen border frame around all 4 edges (default off).
    pub screen_border: bool,
    /// Screen border thickness in design units (6-16 px, default 10).
    pub screen_border_size: u8,
    /// Custom search bar placeholder for Launcher & Activities. Empty = built-in default.
    pub search_placeholder: String,
    /// "manual" or "auto" (follows wallpaper luminance for Windows dark/light mode).
    pub windows_theme_mode: String,
    /// Launch ORUMI automatically when signing in to Windows (HKCU\...\Run).
    pub autostart: bool,
    /// Play the fullscreen water-drop welcome animation once at app startup.
    pub startup_animation: bool,
    /// Enable custom notification reader & Notch notification tab.
    pub notifications_enabled: bool,
    /// Show incoming notifications as a top-notch popup when Notch is closed.
    pub notifications_popup: bool,
    /// Suppress Windows' native bottom-right toast banners while ORUMI runs (restored on exit/crash).
    pub notifications_suppress_native: bool,
    /// "activities" or "launcher": what pressing the Win key by itself opens.
    pub win_key_action: String,
    pub panels: PanelsConfig,
    pub hotkeys: Hotkeys,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ui_scale: 0.8,
            weather_city: "Bandung".into(),
            bg_tint: 35,
            colors: Colors::default(),
            wallpaper_dir: String::new(),
            wallpaper_switcher: "builtin".into(),
            wallpaper_command: String::new(),
            last_wallpaper: String::new(),
            alt_tab: true,
            touchpad_gestures: true,
            touchpad_swipe_up: "activities".into(),
            taskbar_autohide: true,
            taskbar_hidden: false,
            bar_app_style: "icons".into(),
            ui_style: "material".into(),
            mica: true,
            screen_border: false,
            screen_border_size: 10,
            search_placeholder: String::new(),
            windows_theme_mode: "manual".into(),
            autostart: false,
            startup_animation: true,
            notifications_enabled: true,
            notifications_popup: true,
            notifications_suppress_native: true,
            win_key_action: "activities".into(),
            panels: PanelsConfig::default(),
            hotkeys: Hotkeys::default(),
        }
    }
}

pub struct ConfigState(Mutex<Config>);

impl ConfigState {
    pub fn new(cfg: Config) -> Self {
        Self(Mutex::new(cfg))
    }
}

#[derive(Serialize)]
pub struct SaveResult {
    config: Config,
    warnings: Vec<String>,
}

// ---------- File ----------

fn config_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("config.json"))
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn sanitize_slot(s: &mut ColorSlot, default: &ColorSlot) {
    if !matches!(s.mode.as_str(), "auto" | "windows" | "custom") {
        s.mode = "auto".into();
    }
    if is_hex_color(&s.value) {
        s.value = s.value.to_lowercase();
    } else {
        s.value = default.value.clone();
    }
}

fn sanitize_panels(p: &mut PanelsConfig) {
    // Launcher cannot be disabled because it hosts core commands (>settings, >switch, etc.).
    p.launcher.enabled = true;

    if !matches!(p.bar.edge.as_str(), "left" | "right" | "top" | "bottom") {
        p.bar.edge = "left".into();
    }
    if !matches!(p.notch.edge.as_str(), "top" | "bottom") {
        p.notch.edge = "top".into();
    }
    if !matches!(p.launcher.edge.as_str(), "top" | "bottom") {
        p.launcher.edge = "bottom".into();
    }
    if !matches!(p.overlay.edge.as_str(), "left" | "right") {
        p.overlay.edge = "right".into();
    }

    if !p.bar.enabled {
        p.bar.hotedge = false;
    }
    if !p.notch.enabled {
        p.notch.hotedge = false;
    }
    if !p.overlay.enabled {
        p.overlay.hotedge = false;
    }

    // At most one active panel may own the hotedge on any given screen edge.
    let mut used: Vec<String> = Vec::new();
    for pref in [&mut p.bar, &mut p.notch, &mut p.launcher, &mut p.overlay] {
        if pref.hotedge {
            if used.iter().any(|e| e == &pref.edge) {
                pref.hotedge = false;
            } else {
                used.push(pref.edge.clone());
            }
        }
    }
}

/// Normalize values so they are always valid, regardless of file contents or UI input.
fn sanitize(mut c: Config) -> Config {
    let d = Config::default();
    c.ui_scale = ((c.ui_scale.clamp(0.5, 1.5)) * 20.0).round() / 20.0;
    c.weather_city = c.weather_city.trim().to_string();
    if c.weather_city.is_empty() {
        c.weather_city = d.weather_city;
    }
    c.bg_tint = c.bg_tint.min(100);
    if c.bar_app_style != "dots" {
        c.bar_app_style = "icons".into();
    }
    if c.ui_style != "fluent" {
        c.ui_style = "material".into();
    }
    c.screen_border_size = c.screen_border_size.clamp(6, 16);
    c.search_placeholder = c.search_placeholder.trim().to_string();
    if c.windows_theme_mode != "auto" {
        c.windows_theme_mode = "manual".into();
    }
    c.wallpaper_dir = c.wallpaper_dir.trim().trim_matches('"').to_string();
    c.wallpaper_command = c.wallpaper_command.trim().to_string();
    if c.wallpaper_switcher != "external" {
        c.wallpaper_switcher = "builtin".into();
    }
    if c.win_key_action != "launcher" {
        c.win_key_action = "activities".into();
    }
    if c.touchpad_swipe_up != "overview" {
        c.touchpad_swipe_up = "activities".into();
    }
    sanitize_slot(&mut c.colors.bg0, &d.colors.bg0);
    sanitize_slot(&mut c.colors.bg1, &d.colors.bg1);
    sanitize_slot(&mut c.colors.fg, &d.colors.fg);
    sanitize_slot(&mut c.colors.accent, &d.colors.accent);
    sanitize_panels(&mut c.panels);
    c
}

pub fn load(app: &AppHandle) -> Config {
    let Some(path) = config_path(app) else { return Config::default() };
    let Ok(text) = fs::read_to_string(&path) else { return Config::default() };

    let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let mut cfg: Config = serde_json::from_value(raw.clone()).unwrap_or_default();

    // Migrate from the old format (one custom theme color -> custom accent).
    if raw.get("colors").is_none()
        && raw.get("theme_mode").and_then(|v| v.as_str()) == Some("custom")
    {
        if let Some(color) = raw.get("theme_color").and_then(|v| v.as_str()) {
            cfg.colors.accent = ColorSlot { mode: "custom".into(), value: color.into() };
        }
    }
    sanitize(cfg)
}

fn save(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let path = config_path(app).ok_or("config folder not found")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// ---------- Hotkeys ----------

/// Registered shortcuts -> names of the panels they toggle.
static HOTKEY_MAP: Mutex<Vec<(Shortcut, &'static str)>> = Mutex::new(Vec::new());

fn parse_key(k: &str) -> Option<Code> {
    const LETTERS: [Code; 26] = [
        Code::KeyA, Code::KeyB, Code::KeyC, Code::KeyD, Code::KeyE, Code::KeyF, Code::KeyG,
        Code::KeyH, Code::KeyI, Code::KeyJ, Code::KeyK, Code::KeyL, Code::KeyM, Code::KeyN,
        Code::KeyO, Code::KeyP, Code::KeyQ, Code::KeyR, Code::KeyS, Code::KeyT, Code::KeyU,
        Code::KeyV, Code::KeyW, Code::KeyX, Code::KeyY, Code::KeyZ,
    ];
    const DIGITS: [Code; 10] = [
        Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5,
        Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9,
    ];
    const FKEYS: [Code; 12] = [
        Code::F1, Code::F2, Code::F3, Code::F4, Code::F5, Code::F6, Code::F7, Code::F8,
        Code::F9, Code::F10, Code::F11, Code::F12,
    ];

    if k.len() == 1 {
        return match k.as_bytes()[0] {
            c @ b'a'..=b'z' => Some(LETTERS[(c - b'a') as usize]),
            c @ b'0'..=b'9' => Some(DIGITS[(c - b'0') as usize]),
            _ => None,
        };
    }
    if k == "space" {
        return Some(Code::Space);
    }
    let n: usize = k.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&n).then(|| FKEYS[n - 1])
}

/// "Ctrl+Alt+J" -> shortcut. Must include at least one modifier.
fn parse_hotkey(text: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "win" | "super" | "meta" => mods |= Modifiers::SUPER,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(parse_key(other)?);
            }
        }
    }
    if mods.is_empty() {
        return None;
    }
    Some(Shortcut::new(Some(mods), key?))
}

/// Register all hotkeys again. Returns warnings (empty = all succeeded).
fn apply_hotkeys(app: &AppHandle, hk: &Hotkeys) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut map: Vec<(Shortcut, &'static str)> = Vec::new();
    let mut warnings = Vec::new();
    for (label, name, text) in [
        ("bar", "bar", &hk.bar),
        ("notch", "notch", &hk.notch),
        ("launcher", "launcher", &hk.launcher),
    ] {
        match parse_hotkey(text) {
            Some(sc) => match gs.register(sc) {
                Ok(()) => {
                    eprintln!("[hotkey] {text} -> {name}: terdaftar");
                    map.push((sc, label));
                }
                Err(e) => warnings.push(format!(
                    "Cannot register '{text}' ({name}) — likely already in use by another app or Windows: {e}"
                )),
            },
            None => warnings.push(format!(
                "Unrecognized format for {name}: '{text}' (e.g. Ctrl+Alt+B)"
            )),
        }
    }
    if let Ok(mut m) = HOTKEY_MAP.lock() {
        *m = map;
    }
    warnings
}

/// Install the hotkey plugin and register configured hotkeys. Called once at startup.
pub fn init_hotkeys(app: &AppHandle, hk: &Hotkeys) -> tauri::Result<()> {
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, shortcut, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                let label = HOTKEY_MAP
                    .lock()
                    .ok()
                    .and_then(|m| m.iter().find(|(s, _)| s == shortcut).map(|(_, l)| *l));
                if let Some(label) = label {
                    app.state::<Panels>().toggle(label);
                }
            })
            .build(),
    )?;
    for w in apply_hotkeys(app, hk) {
        eprintln!("{w}");
    }
    Ok(())
}

// ---------- Startup ----------

const RUN_KEY: windows::core::PCWSTR =
    windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: windows::core::PCWSTR = windows::core::w!("ORUMI");

/// Register or remove the Windows per-user Run entry (`HKCU\...\Run\ORUMI`).
/// Refuses to register a dev build (`tauri dev`) so it never points startup at `target\debug`.
pub fn apply_autostart(enable: bool) -> Result<(), String> {
    if enable && cfg!(dev) {
        return Err(
            "Autostart can only be registered from a release/installed build (skipped in dev mode)."
                .into(),
        );
    }

    use windows::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ},
    };

    unsafe {
        if enable {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let quoted = format!("\"{}\"", exe.display());
            let mut wide: Vec<u16> = quoted.encode_utf16().collect();
            wide.push(0);
            let status = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(wide.as_ptr() as *const std::ffi::c_void),
                (wide.len() * 2) as u32,
            );
            if status != ERROR_SUCCESS {
                return Err(format!("Failed to register startup entry (Win32 error {})", status.0));
            }
        } else {
            let status = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(format!("Failed to remove startup entry (Win32 error {})", status.0));
            }
        }
    }
    Ok(())
}

// ---------- Commands ----------
/// Save the most recently applied wallpaper. Backend-only, so it does not go through set_config.
pub fn remember_wallpaper(app: &AppHandle, path: &str) {
    let state = app.state::<ConfigState>();
    let snapshot = {
        let Ok(mut c) = state.0.lock() else { return };
        c.last_wallpaper = path.to_string();
        c.clone()
    };
    let _ = save(app, &snapshot);
}

/// Tray menu: enable or disable the custom Alt+Tab switcher.
pub fn toggle_alt_tab(app: &AppHandle) {
    let state = app.state::<ConfigState>();
    let snapshot = {
        let Ok(mut c) = state.0.lock() else { return };
        c.alt_tab = !c.alt_tab;
        c.clone()
    };
    crate::altswitch::set_enabled(snapshot.alt_tab && snapshot.panels.switcher);
    crate::wm_ipc::sync_shell_relay(app);
    let _ = save(app, &snapshot);
    let _ = app.emit("config-changed", &snapshot);
}

/// Apply the configured Win-key target and enabled state.
pub fn sync_win_key(cfg: &Config) {
    let target_launcher = cfg.win_key_action == "launcher";
    crate::activities::set_win_key_target_launcher(target_launcher);
    crate::activities::set_win_key_enabled(target_launcher || cfg.panels.activities);
}

/// Current config for other Rust modules.
pub fn current(app: &AppHandle) -> Config {
    app.state::<ConfigState>()
        .0
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_config(state: State<'_, ConfigState>) -> Config {
    state.0.lock().map(|c| c.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn set_config(
    config: Config,
    app: AppHandle,
    state: State<'_, ConfigState>,
) -> Result<SaveResult, String> {
    let old = state.0.lock().map_err(|e| e.to_string())?.clone();
    let mut new = sanitize(config);
    // Backend-owned field: do not let an old copy from the UI overwrite it.
    new.last_wallpaper = old.last_wallpaper.clone();
    let mut warnings = Vec::new();

    // A new hotkey that failed to register: keep the old one.
    if new.hotkeys != old.hotkeys {
        let w = apply_hotkeys(&app, &new.hotkeys);
        if !w.is_empty() {
            warnings = w;
            new.hotkeys = old.hotkeys.clone();
            let _ = apply_hotkeys(&app, &old.hotkeys);
        }
    }

    if new.autostart != old.autostart {
        if let Err(e) = apply_autostart(new.autostart) {
            warnings.push(e);
            new.autostart = old.autostart;
        }
    }

    save(&app, &new)?;
    *state.0.lock().map_err(|e| e.to_string())? = new.clone();

    crate::altswitch::set_enabled(new.alt_tab && new.panels.switcher);
    crate::touchpad::set_config(new.touchpad_gestures, &new.touchpad_swipe_up);
    sync_win_key(&new);
    if new.alt_tab != old.alt_tab
        || new.panels.switcher != old.panels.switcher
        || new.panels.activities != old.panels.activities
        || new.win_key_action != old.win_key_action
    {
        crate::wm_ipc::sync_shell_relay(&app);
    }
    if new.panels != old.panels {
        crate::edge::set_panel_prefs(new.panels.clone());
        let panels = app.state::<Panels>();
        if !new.panels.bar.enabled {
            panels.set("bar", false);
            panels.set("preview", false);
        }
        if !new.panels.notch.enabled {
            panels.set("notch", false);
        }
        if !new.panels.overlay.enabled {
            panels.set("overlay", false);
        }
        if !new.panels.overview {
            panels.set("overview", false);
        }
        if !new.panels.switcher {
            panels.set("switcher", false);
        }
        if !new.panels.activities && crate::activities::is_open() {
            crate::activities::dismiss(&app);
        }
        crate::apply_layout(&app);
    }
    if new.taskbar_autohide != old.taskbar_autohide || new.taskbar_hidden != old.taskbar_hidden {
        crate::taskbar::apply(&app, new.taskbar_autohide, new.taskbar_hidden);
        crate::wm_ipc::reapply();
    }

    if new.wallpaper_dir != old.wallpaper_dir {
        crate::wallpaper::warm(app.clone());
    }

    if (new.ui_scale - old.ui_scale).abs() > f64::EPSILON
        || new.screen_border != old.screen_border
        || new.screen_border_size != old.screen_border_size
    {
        if new.screen_border != old.screen_border {
            crate::edge::set_border_startup_hold(false);
        }
        crate::edge::set_ui_scale(new.ui_scale);
        crate::edge::set_screen_border(new.screen_border, new.screen_border_size);
        crate::apply_layout(&app);
        crate::wm_ipc::sync_outer_inset(&app);
    }

    if new.windows_theme_mode == "auto" && new.windows_theme_mode != old.windows_theme_mode {
        let sync_app = app.clone();
        std::thread::spawn(move || crate::quick::sync_windows_theme_from_wallpaper(&sync_app));
    }

    if new.notifications_enabled != old.notifications_enabled
        || new.notifications_popup != old.notifications_popup
        || new.notifications_suppress_native != old.notifications_suppress_native
        || new.panels.notch.enabled != old.panels.notch.enabled
    {
        let notch_on = new.panels.notch.enabled;
        crate::notifications::apply_config(
            &app,
            new.notifications_enabled && notch_on,
            new.notifications_popup && notch_on,
            new.notifications_suppress_native && notch_on,
        );
    }

    let _ = app.emit("config-changed", &new);
    Ok(SaveResult { config: new, warnings })
}
