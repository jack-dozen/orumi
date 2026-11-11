//! Active wallpaper and theme (color token) storage shared by all windows.

use std::{ffi::c_void, fs, sync::Mutex, time::UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine};
use tauri::{AppHandle, Emitter};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Most recent theme (JSON tokens), so newly loaded windows can use it immediately.
static THEME: Mutex<Option<String>> = Mutex::new(None);

/// Active wallpaper file path, or None if the color is plain or absent.
fn wallpaper_path() -> Option<String> {
    let mut buf = [0u16; 520];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = String::from_utf16_lossy(&buf[..len]);
    (!path.is_empty()).then_some(path)
}

/// Active wallpaper path according to Windows (for other modules).
pub fn current_wallpaper_path() -> Option<String> {
    wallpaper_path()
}

/// Cheap marker for detecting wallpaper changes (path + modification time + size).
#[tauri::command]
pub fn wallpaper_stamp() -> Option<String> {
    let path = wallpaper_path()?;
    let meta = fs::metadata(&path).ok()?;
    let secs = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{path}|{secs}|{}", meta.len()))
}

/// Wallpaper contents as a data URL. Called only when its stamp changes.
#[tauri::command]
pub async fn wallpaper_image() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let path = wallpaper_path()?;
        let meta = fs::metadata(&path).ok()?;
        if meta.len() == 0 || meta.len() > 40_000_000 {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        let mime = if bytes.starts_with(&[0xFF, 0xD8]) {
            "image/jpeg"
        } else if bytes.starts_with(&[0x89, 0x50]) {
            "image/png"
        } else if bytes.starts_with(b"BM") {
            "image/bmp"
        } else {
            "image/jpeg"
        };
        Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
    })
    .await
    .map_err(|e| e.to_string())
}

/// Called by the notch after calculating the palette: save and broadcast it to all windows.
#[tauri::command]
pub fn set_theme(json: String, app: AppHandle) {
    if let Ok(mut t) = THEME.lock() {
        *t = Some(json.clone());
    }
    let _ = app.emit("theme-changed", json);
}

#[tauri::command]
pub fn get_theme() -> Option<String> {
    THEME.lock().ok().and_then(|t| t.clone())
}

/// Small sample of active wallpaper pixels (RGBA, base64) for extracting dominant colors.
/// Resize the image here, not in the WebView: decoding and processing a 4K wallpaper
/// can briefly use hundreds of MB in JavaScript.
#[derive(serde::Serialize)]
pub struct SmallImage {
    width: u32,
    height: u32,
    data: String,
}

#[tauri::command]
pub async fn wallpaper_pixels() -> Result<Option<SmallImage>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let path = wallpaper_path()?;
        let meta = fs::metadata(&path).ok()?;
        if meta.len() == 0 || meta.len() > 80_000_000 {
            return None;
        }
        let img = image::ImageReader::open(&path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?;
        let small = img.thumbnail(128, 128).to_rgba8();
        Some(SmallImage {
            width: small.width(),
            height: small.height(),
            data: STANDARD.encode(small.as_raw()),
        })
    })
    .await
    .map_err(|e| e.to_string())
}
