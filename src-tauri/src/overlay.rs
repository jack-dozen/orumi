#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use windows::{
    core::{GUID, HRESULT, Interface, IUnknown, PCWSTR},
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::{
            eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDevice, IMMDeviceEnumerator,
            MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
            CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
        },
        UI::Shell::PropertiesSystem::IPropertyStore,
    },
};

use crate::edge::Panels;

/// Content of the active overlay: "power", "mixer", "volume", or "brightness".
static CURRENT_MODE: Mutex<String> = Mutex::new(String::new());
/// When and what content was last closed (to prevent the same button click from reopening it).
static LAST_CLOSE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
/// true = the overlay is shown as an automatic popup (not opened manually).
static AUTO_POPUP: AtomicBool = AtomicBool::new(false);
/// When we last changed volume/mute ourselves (used to skip the automatic OSD popup).
static LAST_OWN_CHANGE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize, Clone)]
pub struct VolumeState {
    volume: f32,
    muted: bool,
}

// ---------- Overlay: buka / tutup ----------

fn current_mode() -> String {
    CURRENT_MODE.lock().map(|m| m.clone()).unwrap_or_default()
}

fn show(app: &AppHandle, mode: &str, manual: bool) {
    if let Ok(mut m) = CURRENT_MODE.lock() {
        *m = mode.to_string();
    }
    AUTO_POPUP.store(!manual, Ordering::Relaxed);
    let _ = app.emit_to("overlay", "overlay-mode", json!({ "mode": mode, "manual": manual }));
    app.state::<Panels>().set("overlay", true);
}

/// Automatic popup (keyboard key). Does not interfere with a manually opened overlay,
/// but may replace another automatic popup (volume -> brightness).
pub fn show_popup(app: &AppHandle, mode: &str) {
    let open = app.state::<Panels>().is_open("overlay");
    if !open || AUTO_POPUP.load(Ordering::Relaxed) {
        show(app, mode, false);
    }
}

/// Cursor dwelled at the right screen edge: show volume + brightness; hides when the cursor leaves.
pub fn hover_open(app: &AppHandle) {
    if let Ok(mut m) = CURRENT_MODE.lock() {
        *m = "mixer".to_string();
    }
    AUTO_POPUP.store(false, Ordering::Relaxed);
    let _ = app.emit_to("overlay", "overlay-mode", json!({ "mode": "mixer", "manual": true }));
    app.state::<Panels>().set_transient("overlay");
}

/// Called from the bar: open an overlay with `mode` content, or close it if the same content is already open.
#[tauri::command]
pub fn overlay_toggle(mode: String, app: AppHandle, panels: State<'_, Panels>) {
    if panels.is_open("overlay") && current_mode() == mode {
        overlay_close(panels);
        return;
    }
    // Clicking the same button immediately after the overlay closes itself (due to focus loss)
    // counts as a close click, not a request to reopen it.
    if let Ok(last) = LAST_CLOSE.lock() {
        if let Some((at, m)) = last.as_ref() {
            if *m == mode && at.elapsed() < Duration::from_millis(350) {
                return;
            }
        }
    }
    show(&app, &mode, true);
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.set_focus();
    }
}

#[tauri::command]
pub fn overlay_close(panels: State<'_, Panels>) {
    panels.set("overlay", false);
    AUTO_POPUP.store(false, Ordering::Relaxed);
    if let Ok(mut last) = LAST_CLOSE.lock() {
        *last = Some((Instant::now(), current_mode()));
    }
}

// ---------- Volume (Windows Core Audio) ----------

unsafe fn endpoint() -> Option<IAudioEndpointVolume> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
}

unsafe fn read_volume(ep: &IAudioEndpointVolume) -> Option<VolumeState> {
    let v = ep.GetMasterVolumeLevelScalar().ok()?;
    let muted = ep.GetMute().ok()?.as_bool();
    Some(VolumeState { volume: v * 100.0, muted })
}

#[tauri::command]
pub async fn get_volume() -> Result<Option<VolumeState>, String> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        let ep = endpoint()?;
        read_volume(&ep)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_volume(percent: f32) -> Result<(), String> {
    LAST_OWN_CHANGE_MS.store(now_ms(), Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || unsafe {
        if let Some(ep) = endpoint() {
            let v = (percent / 100.0).clamp(0.0, 1.0);
            let _ = ep.SetMasterVolumeLevelScalar(v, std::ptr::null());
            if v > 0.0 {
                let _ = ep.SetMute(false, std::ptr::null());
            }
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Watch for volume/mute changes (including keyboard keys) and show the volume popup.
pub fn start_volume_watcher(app: AppHandle) {
    thread::spawn(move || unsafe {
        let mut ep = endpoint();
        let mut acquired = Instant::now();
        let mut last: Option<(i32, bool)> = None;

        loop {
            thread::sleep(Duration::from_millis(250));

            // The default audio device can change (headphones plugged in): refresh periodically.
            if ep.is_none() || acquired.elapsed() > Duration::from_secs(5) {
                ep = endpoint();
                acquired = Instant::now();
            }
            let Some(e) = ep.as_ref() else { continue };
            let Some(state) = read_volume(e) else {
                ep = None;
                continue;
            };

            let cur = (state.volume.round() as i32, state.muted);
            if last != Some(cur) {
                let first_read = last.is_none();
                last = Some(cur);
                if !first_read {
                    let _ = app.emit("volume-changed", state.clone());
                    // Suppress the automatic OSD popup right after our own UI slider/mute change.
                    if now_ms().saturating_sub(LAST_OWN_CHANGE_MS.load(Ordering::Relaxed)) >= 1500 {
                        show_popup(&app, "volume");
                    }
                }
            }
        }
    });
}

/// Called from the speaker icon in the overlay. The volume watcher also updates the popup/slider.
#[tauri::command]
pub async fn toggle_mute() -> Result<(), String> {
    LAST_OWN_CHANGE_MS.store(now_ms(), Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(|| unsafe {
        if let Some(ep) = endpoint() {
            if let Some(cur) = read_volume(&ep) {
                if let Err(e) = ep.SetMute(!cur.muted, std::ptr::null()) {
                    eprintln!("[volume] SetMute gagal: {e}");
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------- Audio Output Device Switcher ----------

#[derive(Serialize, Clone)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
    pub active: bool,
}

const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID {
        data1: 0xa45c254e,
        data2: 0xdf1c,
        data3: 0x4efd,
        data4: [0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0],
    },
    pid: 14,
};

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID {
    data1: 0x870af99c,
    data2: 0x171d,
    data3: 0x4f9e,
    data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
};

const IID_IPOLICY_CONFIG: GUID = GUID {
    data1: 0xf8679f50,
    data2: 0x850a,
    data3: 0x41cf,
    data4: [0x9c, 0x72, 0x43, 0x0f, 0x29, 0x02, 0x90, 0xc8],
};

const IID_IPOLICY_CONFIG_VISTA: GUID = GUID {
    data1: 0x568b9108,
    data2: 0x44bf,
    data3: 0x40b4,
    data4: [0x90, 0x06, 0x86, 0xaf, 0xe5, 0xb5, 0xa6, 0x20],
};

#[repr(C)]
struct IPolicyConfigVtbl {
    query_interface: unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    _reserved_3_to_12: [*const std::ffi::c_void; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut std::ffi::c_void, PCWSTR, u32) -> HRESULT,
}

#[repr(C)]
struct IPolicyConfigVistaVtbl {
    query_interface: unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    _reserved_3_to_11: [*const std::ffi::c_void; 9],
    set_default_endpoint: unsafe extern "system" fn(*mut std::ffi::c_void, PCWSTR, u32) -> HRESULT,
}

unsafe fn device_id(dev: &IMMDevice) -> Option<String> {
    let pwstr = dev.GetId().ok()?;
    let s = String::from_utf16_lossy(pwstr.as_wide());
    CoTaskMemFree(Some(pwstr.0 as _));
    (!s.is_empty()).then_some(s)
}

unsafe fn device_name(dev: &IMMDevice) -> Option<String> {
    let store: IPropertyStore = dev.OpenPropertyStore(STGM_READ).ok()?;
    let mut prop = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok()?;
    let text = PropVariantToStringAlloc(&prop).ok().map(|pwstr| {
        let value = String::from_utf16_lossy(pwstr.as_wide()).trim().to_string();
        CoTaskMemFree(Some(pwstr.0 as _));
        value
    });
    let _ = PropVariantClear(&mut prop);
    text.filter(|s| !s.is_empty())
}

#[tauri::command]
pub async fn list_audio_outputs() -> Result<Vec<AudioOutput>, String> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let active_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|d| device_id(&d));
        let col = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| e.to_string())?;
        let count = col.GetCount().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for i in 0..count {
            let Ok(dev) = col.Item(i) else { continue };
            let Some(id) = device_id(&dev) else { continue };
            let name = device_name(&dev).unwrap_or_else(|| "Audio Device".into());
            let active = active_id.as_deref() == Some(id.as_str());
            out.push(AudioOutput { id, name, active });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_audio_output(id: String, app: AppHandle) -> Result<(), String> {
    LAST_OWN_CHANGE_MS.store(now_ms(), Ordering::Relaxed);
    let new_vol = tauri::async_runtime::spawn_blocking(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let unknown: IUnknown =
            CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                .map_err(|e| e.to_string())?;

        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let dev_id = PCWSTR(wide.as_ptr());

        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        if unknown.query(&IID_IPOLICY_CONFIG, &mut ptr).is_ok() && !ptr.is_null() {
            let vtbl = *(ptr as *const *const IPolicyConfigVtbl);
            for role in 0..3u32 {
                let hr = ((*vtbl).set_default_endpoint)(ptr, dev_id, role);
                if hr.is_err() {
                    ((*vtbl).release)(ptr);
                    return Err(hr.message());
                }
            }
            ((*vtbl).release)(ptr);
        } else if unknown.query(&IID_IPOLICY_CONFIG_VISTA, &mut ptr).is_ok() && !ptr.is_null() {
            let vtbl = *(ptr as *const *const IPolicyConfigVistaVtbl);
            for role in 0..3u32 {
                let hr = ((*vtbl).set_default_endpoint)(ptr, dev_id, role);
                if hr.is_err() {
                    ((*vtbl).release)(ptr);
                    return Err(hr.message());
                }
            }
            ((*vtbl).release)(ptr);
        } else {
            return Err("IPolicyConfig tidak tersedia".into());
        }

        Ok(endpoint().and_then(|ep| read_volume(&ep)))
    })
    .await
    .map_err(|e| e.to_string())??;

    if let Some(state) = new_vol {
        let _ = app.emit("volume-changed", state);
    }
    Ok(())
}
