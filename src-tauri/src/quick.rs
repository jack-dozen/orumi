//! Quick settings for the notch.
//! - Wi-Fi, Bluetooth, and airplane mode: radios through `Windows.Devices.Radios` (pattern from Bloom, GPL-3.0).
//! - Do Not Disturb: the per-user toast master switch `ToastEnabled` (pattern from Astraeus, GPL-3.0,
//!   `sys_notifs.cpp`). Windows 11's own Do Not Disturb has no public API, so this does not show the
//!   moon icon in Windows UI and does not follow DND turned on from Windows quick settings.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{ffi::c_void, sync::Mutex};

use serde::Serialize;
use windows::{
    core::w,
    Devices::Radios::{Radio, RadioKind, RadioState},
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD},
        },
    },
};

const PUSH_KEY: windows::core::PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications");

/// Radio kinds that airplane mode switches off.
const AIRPLANE_KINDS: [RadioKind; 3] =
    [RadioKind::WiFi, RadioKind::Bluetooth, RadioKind::MobileBroadband];

/// Kinds that were on before airplane mode was turned on (in memory only, lost on restart).
static REMEMBERED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

#[derive(Serialize, Default)]
pub struct QuickState {
    /// None = this PC has no such radio.
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    /// At least one radio exists and none of Wi-Fi / Bluetooth / mobile is on.
    airplane: bool,
    dnd: bool,
}

fn radios() -> Result<Vec<Radio>, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let list = Radio::GetRadiosAsync()
        .and_then(|op| op.join())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..list.Size().unwrap_or(0) {
        if let Ok(r) = list.GetAt(i) {
            out.push(r);
        }
    }
    Ok(out)
}

fn is_on(radio: &Radio) -> bool {
    radio.State().ok() == Some(RadioState::On)
}

fn set_radio(radio: &Radio, on: bool) {
    let target = if on { RadioState::On } else { RadioState::Off };
    let _ = radio.SetStateAsync(target).and_then(|op| op.join());
}

fn kind_state(list: &[Radio], kind: RadioKind) -> Option<bool> {
    let mut found = false;
    let mut on = false;
    for r in list.iter().filter(|r| r.Kind().ok() == Some(kind)) {
        found = true;
        on |= is_on(r);
    }
    found.then_some(on)
}

fn dnd_enabled() -> bool {
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PUSH_KEY,
            w!("ToastEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    // Missing value = Windows default (banners on).
    rc == ERROR_SUCCESS && value == 0
}

fn set_dnd(on: bool) {
    let value: u32 = if on { 0 } else { 1 }; // inverted: the value means "toasts enabled"
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PUSH_KEY,
            w!("ToastEnabled"),
            REG_DWORD.0,
            Some(&value as *const u32 as *const c_void),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn set_airplane(on: bool) -> Result<(), String> {
    let list = radios()?;
    let mut remembered = REMEMBERED.lock().unwrap_or_else(|e| e.into_inner());
    if on {
        remembered.clear();
        for r in &list {
            let Ok(kind) = r.Kind() else { continue };
            if AIRPLANE_KINDS.contains(&kind) && is_on(r) {
                remembered.push(kind.0);
                set_radio(r, false);
            }
        }
    } else {
        // Nothing remembered (e.g. airplane mode was turned on from Windows): restore Wi-Fi + Bluetooth.
        let restore: Vec<i32> = if remembered.is_empty() {
            vec![RadioKind::WiFi.0, RadioKind::Bluetooth.0]
        } else {
            remembered.clone()
        };
        remembered.clear();
        for r in &list {
            if let Ok(kind) = r.Kind() {
                if restore.contains(&kind.0) {
                    set_radio(r, true);
                }
            }
        }
    }
    Ok(())
}

fn read_state() -> QuickState {
    let list = radios().unwrap_or_default();
    let wifi = kind_state(&list, RadioKind::WiFi);
    let bluetooth = kind_state(&list, RadioKind::Bluetooth);
    let airplane = !list.is_empty()
        && !list.iter().any(|r| {
            r.Kind().map(|k| AIRPLANE_KINDS.contains(&k)).unwrap_or(false) && is_on(r)
        });
    QuickState { wifi, bluetooth, airplane, dnd: dnd_enabled() }
}

#[tauri::command]
pub async fn quick_get() -> QuickState {
    tauri::async_runtime::spawn_blocking(read_state).await.unwrap_or_default()
}

/// `which`: "wifi" | "bluetooth" | "airplane" | "dnd". Returns the fresh state.
#[tauri::command]
pub async fn quick_set(which: String, on: bool) -> Result<QuickState, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<QuickState, String> {
        match which.as_str() {
            "wifi" | "bluetooth" => {
                let kind = if which == "wifi" { RadioKind::WiFi } else { RadioKind::Bluetooth };
                for r in radios()?.iter().filter(|r| r.Kind().ok() == Some(kind)) {
                    set_radio(r, on);
                }
            }
            "airplane" => set_airplane(on)?,
            "dnd" => set_dnd(on),
            other => return Err(format!("unknown quick setting: {other}")),
        }
        Ok(read_state())
    })
    .await
    .map_err(|e| e.to_string())?
}
