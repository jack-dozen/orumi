#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use windows::Win32::{
    Media::Audio::{
        eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
        MMDeviceEnumerator,
    },
    System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
};

use crate::edge::Panels;

/// Content of the active overlay: "power", "mixer", "volume", or "brightness".
static CURRENT_MODE: Mutex<String> = Mutex::new(String::new());
/// When and what content was last closed (to prevent the same button click from reopening it).
static LAST_CLOSE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
/// true = the overlay is shown as an automatic popup (not opened manually).
static AUTO_POPUP: AtomicBool = AtomicBool::new(false);

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
                    show_popup(&app, "volume");
                }
            }
        }
    });
}

/// Called from the speaker icon in the overlay. The volume watcher also updates the popup/slider.
#[tauri::command]
pub async fn toggle_mute() -> Result<(), String> {
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
