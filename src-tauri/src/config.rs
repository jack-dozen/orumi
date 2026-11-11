//! Konfigurasi app: file JSON di folder config, plus hotkey global yang bisa diubah.

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
    /// "auto" (dari wallpaper) atau "custom".
    pub mode: String,
    /// Warna yang dipakai kalau mode = "custom", format #rrggbb.
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
    /// Latar panel (bar, notch, launcher, overlay).
    pub bg0: ColorSlot,
    /// Latar kartu.
    pub bg1: ColorSlot,
    /// Teks dan ikon.
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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub ui_scale: f64,
    pub weather_city: String,
    /// 0-100: seberapa kuat warna wallpaper terlihat di latar dan teks mode "auto".
    pub bg_tint: u8,
    pub colors: Colors,
    /// Folder wallpaper untuk pengalih bawaan. Kosong = folder Pictures.
    pub wallpaper_dir: String,
    /// "builtin" (pengalih di launcher) atau "external" (jalankan perintah di bawah).
    pub wallpaper_switcher: String,
    pub wallpaper_command: String,
    /// Wallpaper terakhir yang diterapkan lewat app ini. Hanya diisi backend.
    pub last_wallpaper: String,
    /// true = Alt+Tab diganti switcher kustom; false = Alt+Tab bawaan Windows.
    pub alt_tab: bool,
    /// true = taskbar Windows diset auto-hide selama edgeshell berjalan (dipulihkan saat keluar).
    pub taskbar_autohide: bool,
    /// "icons" atau "dots": tampilan daftar app terbuka di bar.
    pub bar_app_style: String,
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
            taskbar_autohide: true,
            bar_app_style: "icons".into(),
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
    if s.mode != "custom" {
        s.mode = "auto".into();
    }
    if is_hex_color(&s.value) {
        s.value = s.value.to_lowercase();
    } else {
        s.value = default.value.clone();
    }
}

/// Rapikan nilai supaya selalu valid, apa pun isi file atau kiriman UI-nya.
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
    c.wallpaper_dir = c.wallpaper_dir.trim().trim_matches('"').to_string();
    c.wallpaper_command = c.wallpaper_command.trim().to_string();
    if c.wallpaper_switcher != "external" {
        c.wallpaper_switcher = "builtin".into();
    }
    sanitize_slot(&mut c.colors.bg0, &d.colors.bg0);
    sanitize_slot(&mut c.colors.bg1, &d.colors.bg1);
    sanitize_slot(&mut c.colors.fg, &d.colors.fg);
    sanitize_slot(&mut c.colors.accent, &d.colors.accent);
    c
}

pub fn load(app: &AppHandle) -> Config {
    let Some(path) = config_path(app) else { return Config::default() };
    let Ok(text) = fs::read_to_string(&path) else { return Config::default() };

    let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let mut cfg: Config = serde_json::from_value(raw.clone()).unwrap_or_default();

    // Migrasi dari format lama (satu warna tema custom -> aksen custom).
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
    let path = config_path(app).ok_or("folder config tidak ditemukan")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// ---------- Hotkey ----------

/// Shortcut yang sedang terdaftar -> nama panel yang di-toggle.
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

/// "Ctrl+Alt+J" -> Shortcut. Wajib memakai minimal satu modifier.
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

/// Daftarkan ulang semua hotkey. Mengembalikan peringatan (kosong = semua berhasil).
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
                    "Hotkey {text} ({name}) tidak bisa dipakai, mungkin dipakai app lain: {e}"
                )),
            },
            None => warnings.push(format!("Hotkey {name} tidak valid: {text}")),
        }
    }
    if let Ok(mut m) = HOTKEY_MAP.lock() {
        *m = map;
    }
    warnings
}

/// Pasang plugin hotkey dan daftarkan hotkey dari config. Dipanggil sekali saat startup.
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

// ---------- Command ----------
/// Simpan wallpaper terakhir yang diterapkan. Backend-only, jadi tidak lewat set_config.
pub fn remember_wallpaper(app: &AppHandle, path: &str) {
    let state = app.state::<ConfigState>();
    let snapshot = {
        let Ok(mut c) = state.0.lock() else { return };
        c.last_wallpaper = path.to_string();
        c.clone()
    };
    let _ = save(app, &snapshot);
}

/// Menu tray: nyalakan atau matikan Alt+Tab kustom.
pub fn toggle_alt_tab(app: &AppHandle) {
    let state = app.state::<ConfigState>();
    let snapshot = {
        let Ok(mut c) = state.0.lock() else { return };
        c.alt_tab = !c.alt_tab;
        c.clone()
    };
    crate::altswitch::set_enabled(snapshot.alt_tab);
    let _ = save(app, &snapshot);
    let _ = app.emit("config-changed", &snapshot);
}

/// Config terkini untuk modul Rust lain.
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
    // Field milik backend: jangan sampai tertimpa salinan lama dari UI.
    new.last_wallpaper = old.last_wallpaper.clone();
    let mut warnings = Vec::new();

    // Hotkey baru yang gagal terdaftar: pertahankan yang lama.
    if new.hotkeys != old.hotkeys {
        let w = apply_hotkeys(&app, &new.hotkeys);
        if !w.is_empty() {
            warnings = w;
            new.hotkeys = old.hotkeys.clone();
            let _ = apply_hotkeys(&app, &old.hotkeys);
        }
    }

    save(&app, &new)?;
    *state.0.lock().map_err(|e| e.to_string())? = new.clone();

    crate::altswitch::set_enabled(new.alt_tab);
    if new.taskbar_autohide != old.taskbar_autohide {
        crate::taskbar::apply(&app, new.taskbar_autohide);
    }

    if new.wallpaper_dir != old.wallpaper_dir {
        crate::wallpaper::warm(app.clone());
    }

    if (new.ui_scale - old.ui_scale).abs() > f64::EPSILON {
        crate::edge::set_ui_scale(new.ui_scale);
        crate::apply_layout(&app);
    }

    let _ = app.emit("config-changed", &new);
    Ok(SaveResult { config: new, warnings })
}
