use std::{os::windows::process::CommandExt, process::Command, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

/// Supaya PowerShell/explorer tidak memunculkan jendela konsol.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Deserialize)]
struct RawApp {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "AppID")]
    id: String,
}

#[derive(Clone, Serialize)]
pub struct AppEntry {
    name: String,
    id: String,
}

/// Cache daftar app (None = belum pernah dimuat).
pub struct AppsState(Mutex<Option<Vec<AppEntry>>>);

impl AppsState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

/// Baca daftar app Start menu lewat PowerShell (butuh ~1-2 detik).
fn load_apps() -> Vec<AppEntry> {
    let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
                  ConvertTo-Json -Compress -InputObject @(Get-StartApps)";
    let Ok(out) = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim_start_matches('\u{feff}');
    let raw: Vec<RawApp> = serde_json::from_str(text).unwrap_or_default();

    let mut apps: Vec<AppEntry> = raw
        .into_iter()
        .filter(|a| !a.name.trim().is_empty() && !a.id.is_empty())
        .map(|a| AppEntry { name: a.name, id: a.id })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.name == b.name && a.id == b.id);
    apps
}

#[tauri::command]
pub async fn list_apps(refresh: bool, state: State<'_, AppsState>) -> Result<Vec<AppEntry>, String> {
    if !refresh {
        if let Some(cached) = state.0.lock().map_err(|e| e.to_string())?.as_ref() {
            return Ok(cached.clone());
        }
    }
    let apps = tauri::async_runtime::spawn_blocking(load_apps)
        .await
        .map_err(|e| e.to_string())?;
    *state.0.lock().map_err(|e| e.to_string())? = Some(apps.clone());
    Ok(apps)
}

#[tauri::command]
pub fn launch_app(id: String) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(format!("shell:AppsFolder\\{id}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
