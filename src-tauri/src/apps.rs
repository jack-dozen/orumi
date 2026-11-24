#![allow(unsafe_op_in_unsafe_fn)]

use std::{os::windows::process::CommandExt, process::Command, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::{
    core::{w, GUID, Interface, PWSTR},
    Win32::{
        Foundation::PROPERTYKEY,
        System::Com::{
            CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{
            BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2,
            SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
        },
    },
};

/// Prevent PowerShell/Explorer from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Singleflight lock so Launcher and Activities don't enumerate Start apps twice in parallel at boot.
static LOAD_LOCK: Mutex<()> = Mutex::new(());

/// System.AppUserModel.ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
const PKEY_APPUSERMODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID {
        data1: 0x9F4C_2855,
        data2: 0x9F79,
        data3: 0x4B39,
        data4: [0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3],
    },
    pid: 5,
};

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

/// Cache of the app list (None = not loaded yet).
pub struct AppsState(Mutex<Option<Vec<AppEntry>>>);

impl AppsState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

unsafe fn take_pwstr(pwstr: PWSTR) -> String {
    if pwstr.0.is_null() {
        return String::new();
    }
    let s = String::from_utf16_lossy(pwstr.as_wide()).trim().to_string();
    CoTaskMemFree(Some(pwstr.0 as _));
    s
}

/// Enumerate Start Menu applications directly via COM `shell:AppsFolder` (avoids spawning PowerShell).
fn load_apps_native() -> Vec<AppEntry> {
    unsafe {
        let com_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let mut apps = Vec::new();

        if let Ok(folder) =
            SHCreateItemFromParsingName::<_, _, IShellItem>(w!("shell:AppsFolder"), None)
        {
            if let Ok(enum_items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems)
            {
                let mut batch: [Option<IShellItem>; 1] = [None];
                loop {
                    let mut fetched = 0u32;
                    let hr = enum_items.Next(&mut batch, Some(&mut fetched));
                    if hr.is_err() || fetched == 0 {
                        break;
                    }
                    let Some(item) = batch[0].take() else { break };

                    let name = item
                        .GetDisplayName(SIGDN_NORMALDISPLAY)
                        .ok()
                        .map(|p| take_pwstr(p))
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }

                    let mut id = item
                        .cast::<IShellItem2>()
                        .ok()
                        .and_then(|item2| item2.GetString(&PKEY_APPUSERMODEL_ID).ok())
                        .map(|p| take_pwstr(p))
                        .unwrap_or_default();
                    if id.is_empty() {
                        id = item
                            .GetDisplayName(SIGDN_PARENTRELATIVEPARSING)
                            .ok()
                            .map(|p| take_pwstr(p))
                            .unwrap_or_default();
                    }
                    if !id.is_empty() {
                        apps.push(AppEntry { name, id });
                    }
                }
            }
        }

        if com_ok {
            CoUninitialize();
        }
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps.dedup_by(|a, b| a.name == b.name && a.id == b.id);
        apps
    }
}

/// Fallback: read the Start menu app list through PowerShell (takes ~1-2 seconds).
fn load_apps_ps() -> Vec<AppEntry> {
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

fn load_apps() -> Vec<AppEntry> {
    let native = load_apps_native();
    if !native.is_empty() {
        native
    } else {
        load_apps_ps()
    }
}

#[tauri::command]
pub async fn list_apps(refresh: bool, app: AppHandle) -> Result<Vec<AppEntry>, String> {
    if !refresh {
        if let Some(cached) = app
            .state::<AppsState>()
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
        {
            return Ok(cached.clone());
        }
    }
    let app_bg = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = LOAD_LOCK.lock().map_err(|e| e.to_string())?;
        let state = app_bg.state::<AppsState>();
        if !refresh {
            if let Some(cached) = state.0.lock().map_err(|e| e.to_string())?.as_ref() {
                return Ok(cached.clone());
            }
        }
        let apps = load_apps();
        *state.0.lock().map_err(|e| e.to_string())? = Some(apps.clone());
        Ok(apps)
    })
    .await
    .map_err(|e| e.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_apps_folder_loads_entries() {
        let apps = load_apps_native();
        assert!(!apps.is_empty(), "expected shell:AppsFolder to return Start apps");
        assert!(apps.iter().all(|a| !a.name.is_empty() && !a.id.is_empty()));
    }
}
