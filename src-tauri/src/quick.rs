//! Quick settings for the notch and bar.
//! - Wi-Fi, Bluetooth, and airplane mode: radios through `Windows.Devices.Radios` (pattern from Bloom, GPL-3.0).
//! - Do Not Disturb: the per-user toast master switch `ToastEnabled` (pattern from Astraeus, GPL-3.0,
//!   `sys_notifs.cpp`). Windows 11's own Do Not Disturb has no public API, so this does not show the
//!   moon icon in Windows UI and does not follow DND turned on from Windows quick settings.
//! - Mobile hotspot: `Windows.Networking.NetworkOperators::NetworkOperatorTetheringManager`.
//! - Microphone mute: Windows Core Audio default capture endpoint (`eCapture`).
//! - Caffeine / Keep awake: `SetThreadExecutionState` on a dedicated worker thread.
//! - Windows dark/light mode: `HKCU\...\Themes\Personalize` + `WM_SETTINGCHANGE`.
//! - Power mode / plan: `PowerGetEffectiveOverlayScheme` / `PowerSetActiveOverlayScheme` with
//!   fallback to `PowerGetActiveScheme` / `PowerSetActiveScheme`.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::UNIX_EPOCH,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::{w, GUID},
    Devices::Radios::{Radio, RadioKind, RadioState},
    Networking::{
        Connectivity::NetworkInformation,
        NetworkOperators::{NetworkOperatorTetheringManager, TetheringOperationalState},
    },
    Win32::{
        Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree, LPARAM, WPARAM},
        Media::Audio::{
            eCapture, eConsole, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
            MMDeviceEnumerator,
        },
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            Power::{
                PowerGetActiveScheme, PowerSetActiveScheme, SetThreadExecutionState,
                ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
            },
            Registry::{
                RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD,
            },
        },
        UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        },
    },
};

const PUSH_KEY: windows::core::PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications");
const PERSONALIZE_KEY: windows::core::PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

const MAX_WALLPAPER_BYTES: u64 = 80_000_000;

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> GUID {
    GUID { data1, data2, data3, data4 }
}

// Windows 11 Power Mode overlay GUIDs.
const OVERLAY_SAVER: GUID = guid(0x961cc777, 0x2547, 0x4f9d, [0x81, 0x74, 0x7d, 0x86, 0x18, 0x1b, 0x8a, 0x7a]);
const OVERLAY_BALANCED: GUID = guid(0, 0, 0, [0; 8]);
const OVERLAY_PERF: GUID = guid(0xded574b5, 0x45a0, 0x4f42, [0x87, 0x37, 0x46, 0x34, 0x5c, 0x09, 0xc2, 0x38]);
const OVERLAY_MAX_PERF: GUID = guid(0x3af9b8d9, 0x7c97, 0x431d, [0xad, 0x78, 0x34, 0xa8, 0xbf, 0xea, 0x43, 0x9f]);

// Classic Windows power scheme GUIDs.
const SCHEME_SAVER: GUID = guid(0xa1841308, 0x3541, 0x4fab, [0xbc, 0x81, 0xf7, 0x15, 0x56, 0xf2, 0x0b, 0x4a]);
const SCHEME_BALANCED: GUID = guid(0x381b4222, 0xf694, 0x41f0, [0x96, 0x85, 0xff, 0x5b, 0xb2, 0x60, 0xdf, 0x2e]);
const SCHEME_PERF: GUID = guid(0x8c5e7fda, 0xe8bf, 0x4a96, [0x9a, 0x85, 0xa6, 0xe2, 0x3a, 0x8c, 0x63, 0x5c]);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

type GetOverlayFn = unsafe extern "system" fn(*mut GUID) -> u32;
type SetOverlayFn = unsafe extern "system" fn(*const GUID) -> u32;

static OVERLAY_FNS: OnceLock<(Option<GetOverlayFn>, Option<SetOverlayFn>)> = OnceLock::new();

fn overlay_fns() -> (Option<GetOverlayFn>, Option<SetOverlayFn>) {
    *OVERLAY_FNS.get_or_init(|| unsafe {
        let h = LoadLibraryW(w!("powrprof.dll").as_ptr());
        if h.is_null() {
            return (None, None);
        }
        let get_ptr = GetProcAddress(h, b"PowerGetEffectiveOverlayScheme\0".as_ptr());
        let set_ptr = GetProcAddress(h, b"PowerSetActiveOverlayScheme\0".as_ptr());
        let get_fn = (!get_ptr.is_null())
            .then(|| std::mem::transmute::<*const c_void, GetOverlayFn>(get_ptr));
        let set_fn = (!set_ptr.is_null())
            .then(|| std::mem::transmute::<*const c_void, SetOverlayFn>(set_ptr));
        (get_fn, set_fn)
    })
}

/// Radio kinds that airplane mode switches off.
const AIRPLANE_KINDS: [RadioKind; 3] =
    [RadioKind::WiFi, RadioKind::Bluetooth, RadioKind::MobileBroadband];

/// Kinds that were on before airplane mode was turned on (in memory only, lost on restart).
static REMEMBERED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Caffeine state and dedicated execution-state thread (`SetThreadExecutionState` is per-thread).
static CAFFEINE_ON: AtomicBool = AtomicBool::new(false);
static CAFFEINE_TX: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

/// Cached wallpaper luminance decision keyed by `path|mtime|size`.
static LAST_WALLPAPER_LUMA: Mutex<Option<(String, bool)>> = Mutex::new(None);

#[derive(Serialize, Clone, Default)]
pub struct QuickState {
    /// None = this PC has no such radio.
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    /// At least one radio exists and none of Wi-Fi / Bluetooth / mobile is on.
    pub airplane: bool,
    pub dnd: bool,
    /// None = tethering unavailable / no base profile; Some(true) = On.
    pub hotspot: Option<bool>,
    /// None = no capture device; Some(true) = mic active (unmuted), Some(false) = muted.
    pub mic: Option<bool>,
    /// Keep awake via SetThreadExecutionState on a dedicated worker thread.
    pub caffeine: bool,
    /// Windows dark mode (true = dark, false = light).
    pub dark_mode: bool,
    /// Active power mode: "saver" | "balanced" | "performance".
    pub power_mode: String,
}

// ---------- Radios (Wi-Fi, Bluetooth, Airplane) ----------

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

// ---------- Do Not Disturb ----------

fn read_hkcu_dword(subkey: windows::core::PCWSTR, value_name: windows::core::PCWSTR) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            value_name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(value)
}

fn write_hkcu_dword(subkey: windows::core::PCWSTR, value_name: windows::core::PCWSTR, value: u32) {
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            subkey,
            value_name,
            REG_DWORD.0,
            Some(&value as *const u32 as *const c_void),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn dnd_enabled() -> bool {
    if crate::notifications::suppression_active() {
        return crate::notifications::is_dnd();
    }
    // Missing value = Windows default (banners on).
    read_hkcu_dword(PUSH_KEY, w!("ToastEnabled")) == Some(0)
}

fn set_dnd(app: &AppHandle, on: bool) {
    if crate::notifications::suppression_active() {
        crate::notifications::set_dnd(app, on);
        return;
    }
    let value: u32 = if on { 0 } else { 1 }; // inverted: the value means "toasts enabled"
    write_hkcu_dword(PUSH_KEY, w!("ToastEnabled"), value);
}

// ---------- Mobile Hotspot ----------

fn tethering_manager() -> Option<NetworkOperatorTetheringManager> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile).ok()
}

fn hotspot_state() -> Option<bool> {
    let mgr = tethering_manager()?;
    let st = mgr.TetheringOperationalState().ok()?;
    Some(st == TetheringOperationalState::On)
}

fn set_hotspot(on: bool) -> Result<(), String> {
    let mgr = tethering_manager()
        .ok_or_else(|| "Hotspot tidak tersedia tanpa koneksi internet".to_string())?;
    if on {
        let _ = mgr
            .StartTetheringAsync()
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
    } else {
        let _ = mgr
            .StopTetheringAsync()
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- Microphone Mute ----------

unsafe fn mic_endpoint() -> Option<IAudioEndpointVolume> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
    device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
}

fn mic_state() -> Option<bool> {
    unsafe {
        let ep = mic_endpoint()?;
        let muted = ep.GetMute().ok()?.as_bool();
        Some(!muted)
    }
}

fn set_mic(active: bool) {
    unsafe {
        if let Some(ep) = mic_endpoint() {
            let _ = ep.SetMute(!active, std::ptr::null());
        }
    }
}

// ---------- Caffeine / Keep Awake ----------

fn caffeine_tx() -> &'static mpsc::Sender<bool> {
    CAFFEINE_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<bool>();
        thread::spawn(move || {
            while let Ok(mut on) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    on = newer;
                }
                let flags = if on {
                    ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
                } else {
                    ES_CONTINUOUS
                };
                unsafe {
                    SetThreadExecutionState(flags);
                }
            }
        });
        tx
    })
}

fn set_caffeine(on: bool) {
    CAFFEINE_ON.store(on, Ordering::Relaxed);
    let _ = caffeine_tx().send(on);
}

// ---------- Windows Dark / Light Mode + Wallpaper Sync ----------

fn dark_mode_enabled() -> bool {
    read_hkcu_dword(PERSONALIZE_KEY, w!("AppsUseLightTheme")) == Some(0)
}

pub fn set_dark_mode(dark: bool) {
    let val: u32 = if dark { 0 } else { 1 };
    write_hkcu_dword(PERSONALIZE_KEY, w!("AppsUseLightTheme"), val);
    write_hkcu_dword(PERSONALIZE_KEY, w!("SystemUsesLightTheme"), val);
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            200,
            None,
        );
    }
}

fn wallpaper_wants_dark() -> Option<bool> {
    let path = crate::theme::current_wallpaper_path()?;
    let meta = fs::metadata(&path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_WALLPAPER_BYTES {
        return None;
    }
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = format!("{path}|{secs}|{}", meta.len());

    if let Ok(guard) = LAST_WALLPAPER_LUMA.lock() {
        if let Some((cached_stamp, want_dark)) = guard.as_ref() {
            if *cached_stamp == stamp {
                return Some(*want_dark);
            }
        }
    }

    let img = image::ImageReader::open(&path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let small = img.thumbnail(64, 64).to_rgb8();
    let raw = small.as_raw();
    if raw.is_empty() {
        return None;
    }
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for px in raw.chunks_exact(3) {
        let luma = (0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64) / 255.0;
        sum += luma;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let want_dark = (sum / count as f64) < 0.5;
    if let Ok(mut guard) = LAST_WALLPAPER_LUMA.lock() {
        *guard = Some((stamp, want_dark));
    }
    Some(want_dark)
}

/// Sync Windows Dark/Light mode from current wallpaper luminance if `windows_theme_mode == "auto"`.
pub fn sync_windows_theme_from_wallpaper(app: &AppHandle) {
    if crate::config::current(app).windows_theme_mode != "auto" {
        return;
    }
    let Some(want_dark) = wallpaper_wants_dark() else { return };
    if dark_mode_enabled() != want_dark {
        set_dark_mode(want_dark);
        let _ = app.emit("quick-changed", read_state());
        let _ = app.emit("theme-refresh", ());
    }
}

// ---------- Power Mode / Power Plan ----------

fn read_power_mode() -> String {
    let (get_overlay, _) = overlay_fns();
    unsafe {
        if let Some(get_fn) = get_overlay {
            let mut overlay = GUID::default();
            if get_fn(&mut overlay) == 0 && overlay != OVERLAY_BALANCED {
                if overlay == OVERLAY_SAVER {
                    return "saver".into();
                }
                if overlay == OVERLAY_PERF || overlay == OVERLAY_MAX_PERF {
                    return "performance".into();
                }
            }
        }
        let mut active: *mut GUID = std::ptr::null_mut();
        if PowerGetActiveScheme(None, &mut active) == ERROR_SUCCESS && !active.is_null() {
            let scheme = *active;
            let _ = LocalFree(Some(HLOCAL(active as *mut c_void)));
            if scheme == SCHEME_SAVER {
                return "saver".into();
            }
            if scheme == SCHEME_PERF {
                return "performance".into();
            }
        }
    }
    "balanced".into()
}

fn set_power_mode(mode: &str) -> Result<(), String> {
    let (overlay_guid, classic_guid) = match mode {
        "saver" => (OVERLAY_SAVER, SCHEME_SAVER),
        "performance" => (OVERLAY_PERF, SCHEME_PERF),
        _ => (OVERLAY_BALANCED, SCHEME_BALANCED),
    };
    let (_, set_overlay) = overlay_fns();
    unsafe {
        let mut overlay_rc: Option<u32> = None;
        if let Some(set_fn) = set_overlay {
            let rc = set_fn(&overlay_guid);
            if rc == 0 {
                return Ok(());
            }
            overlay_rc = Some(rc);
        }
        let rc2 = PowerSetActiveScheme(None, Some(&classic_guid));
        if rc2 == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!(
                "gagal mengubah mode daya (overlay={:?}, scheme={})",
                overlay_rc, rc2.0
            ))
        }
    }
}

fn next_power_mode(current: &str) -> &'static str {
    match current {
        "saver" => "balanced",
        "balanced" => "performance",
        _ => "saver",
    }
}

// ---------- State & Commands ----------

fn read_state() -> QuickState {
    let list = radios().unwrap_or_default();
    let wifi = kind_state(&list, RadioKind::WiFi);
    let bluetooth = kind_state(&list, RadioKind::Bluetooth);
    let airplane = !list.is_empty()
        && !list.iter().any(|r| {
            r.Kind().map(|k| AIRPLANE_KINDS.contains(&k)).unwrap_or(false) && is_on(r)
        });
    QuickState {
        wifi,
        bluetooth,
        airplane,
        dnd: dnd_enabled(),
        hotspot: hotspot_state(),
        mic: mic_state(),
        caffeine: CAFFEINE_ON.load(Ordering::Relaxed),
        dark_mode: dark_mode_enabled(),
        power_mode: read_power_mode(),
    }
}

#[tauri::command]
pub async fn quick_get() -> QuickState {
    tauri::async_runtime::spawn_blocking(read_state).await.unwrap_or_default()
}

/// `which`: "wifi" | "bluetooth" | "airplane" | "dnd" | "hotspot" | "mic" | "caffeine" | "dark_mode" | "power_mode".
#[tauri::command]
pub async fn quick_set(which: String, on: bool, app: AppHandle) -> Result<QuickState, String> {
    let auto_theme = crate::config::current(&app).windows_theme_mode == "auto";
    let dnd_app = app.clone();
    let state = tauri::async_runtime::spawn_blocking(move || -> Result<QuickState, String> {
        match which.as_str() {
            "wifi" | "bluetooth" => {
                let kind = if which == "wifi" { RadioKind::WiFi } else { RadioKind::Bluetooth };
                for r in radios()?.iter().filter(|r| r.Kind().ok() == Some(kind)) {
                    set_radio(r, on);
                }
            }
            "airplane" => set_airplane(on)?,
            "dnd" => set_dnd(&dnd_app, on),
            "hotspot" => set_hotspot(on)?,
            "mic" => set_mic(on),
            "caffeine" => set_caffeine(on),
            "dark_mode" => {
                if !auto_theme {
                    set_dark_mode(on);
                    let _ = dnd_app.emit("theme-refresh", ());
                }
            }
            "power_mode" => {
                let next = next_power_mode(&read_power_mode());
                set_power_mode(next)?;
            }
            other => return Err(format!("unknown quick setting: {other}")),
        }
        Ok(read_state())
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = app.emit("quick-changed", &state);
    Ok(state)
}

#[tauri::command]
pub async fn cycle_power_mode(app: AppHandle) -> Result<QuickState, String> {
    let state = tauri::async_runtime::spawn_blocking(|| -> Result<QuickState, String> {
        let next = next_power_mode(&read_power_mode());
        set_power_mode(next)?;
        Ok(read_state())
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = app.emit("quick-changed", &state);
    Ok(state)
}
