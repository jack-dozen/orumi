//! Active wallpaper and theme (color token) storage shared by all windows.

use std::{
    collections::hash_map::DefaultHasher,
    ffi::c_void,
    fs,
    hash::{Hash, Hasher},
    sync::Mutex,
    time::UNIX_EPOCH,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use image::{imageops::FilterType, ImageFormat};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETDESKWALLPAPER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Most recent theme (JSON tokens), so newly loaded windows can use it immediately.
static THEME: Mutex<Option<String>> = Mutex::new(None);
/// Cached blurred wallpaper path for the Windows 11 Mica effect.
static MICA_PATH: Mutex<Option<String>> = Mutex::new(None);

const MICA_VERSION: u32 = 1;
const MICA_WIDTH: u32 = 420;
const MICA_BLUR_SIGMA: f32 = 22.0;

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

/// Called by the notch after calculating the palette: save and broadcast it to all windows.
#[tauri::command]
pub fn set_theme(json: String, app: AppHandle) {
    if let Ok(mut t) = THEME.lock() {
        *t = Some(json.clone());
    }
    let _ = app.emit("theme-changed", json);
    let sync_app = app.clone();
    std::thread::spawn(move || {
        crate::quick::sync_windows_theme_from_wallpaper(&sync_app);
        crate::wm_ipc::sync_theme_colors(&sync_app);
    });
}

#[tauri::command]
pub fn get_theme() -> Option<String> {
    THEME.lock().ok().and_then(|t| t.clone())
}

/// Extract Material You surface/container/primary hex tokens (`#rrggbb`) from the latest theme map.
pub fn current_colors() -> Option<(String, String, String)> {
    let json = get_theme()?;
    let map: std::collections::HashMap<String, String> = serde_json::from_str(&json).ok()?;
    let primary = map.get("--primary")?.clone();
    let surface = map.get("--surface").cloned().unwrap_or_else(|| "#1c1f26".to_string());
    let container = map
        .get("--surface-container-high")
        .or_else(|| map.get("--surface-container"))
        .cloned()
        .unwrap_or_else(|| "#2d323e".to_string());
    Some((surface, container, primary))
}

/// Extract the computed `--primary` hex color (`#rrggbb`) from the latest theme tokens.
pub fn current_primary() -> Option<String> {
    current_colors().map(|(_, _, p)| p)
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

#[tauri::command]
pub fn get_mica() -> Option<String> {
    MICA_PATH.lock().ok().and_then(|m| m.clone())
}

fn mica_hash(stamp: &str, sw: i32, sh: i32) -> u64 {
    let mut h = DefaultHasher::new();
    (MICA_VERSION, stamp, sw, sh).hash(&mut h);
    h.finish()
}

fn generate_mica_file(app: &AppHandle) -> Option<String> {
    let path = wallpaper_path()?;
    let stamp = wallpaper_stamp()?;
    let meta = fs::metadata(&path).ok()?;
    if meta.len() == 0 || meta.len() > 80_000_000 {
        return None;
    }

    let (sw, sh) = unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w > 0 && h > 0 { (w, h) } else { (1920, 1080) }
    };

    let dir = app
        .path()
        .app_cache_dir()
        .ok()
        .map(|d| d.join("mica"))
        .unwrap_or_else(|| std::env::temp_dir().join("edgeshell-mica"));
    let _ = fs::create_dir_all(&dir);
    let _ = app.asset_protocol_scope().allow_directory(&dir, false);

    let file = dir.join(format!("{:016x}.jpg", mica_hash(&stamp, sw, sh)));
    if file.is_file() {
        return Some(file.to_string_lossy().into_owned());
    }

    let img = image::ImageReader::open(&path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (iw, ih) = (img.width().max(1), img.height().max(1));

    // Cover-crop to the primary monitor aspect ratio so the blurred Mica backdrop aligns with
    // the desktop wallpaper behind every panel.
    let target_ar = sw as f64 / sh as f64;
    let img_ar = iw as f64 / ih as f64;
    let cropped = if img_ar > target_ar {
        let cw = ((ih as f64 * target_ar).round() as u32).clamp(1, iw);
        let x = (iw - cw) / 2;
        img.crop_imm(x, 0, cw, ih)
    } else {
        let ch = ((iw as f64 / target_ar).round() as u32).clamp(1, ih);
        let y = (ih - ch) / 2;
        img.crop_imm(0, y, iw, ch)
    };

    let out_w = MICA_WIDTH;
    let out_h = ((out_w as f64 / target_ar).round() as u32).max(1);
    let resized = cropped.resize_exact(out_w, out_h, FilterType::Triangle);
    let blurred = image::imageops::blur(&resized.to_rgba8(), MICA_BLUR_SIGMA);
    let rgb = image::DynamicImage::ImageRgba8(blurred).to_rgb8();

    let tmp = file.with_extension("jpg.tmp");
    rgb.save_with_format(&tmp, ImageFormat::Jpeg).ok()?;
    let _ = fs::rename(&tmp, &file);

    // Keep cache directory tidy (only the active Mica backdrop).
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p != file {
                let _ = fs::remove_file(p);
            }
        }
    }

    Some(file.to_string_lossy().into_owned())
}

/// Generate/reuse the heavily blurred wallpaper backdrop for Windows 11 Fluent Mica panels
/// and broadcast its path to all windows.
#[tauri::command]
pub async fn mica_backdrop(app: AppHandle) -> Result<Option<String>, String> {
    let app_bg = app.clone();
    let next = tauri::async_runtime::spawn_blocking(move || generate_mica_file(&app_bg))
        .await
        .map_err(|e| e.to_string())?;

    let changed = if let Ok(mut g) = MICA_PATH.lock() {
        let c = *g != next;
        *g = next.clone();
        c
    } else {
        true
    };
    if changed {
        let _ = app.emit("mica-changed", next.clone());
    }
    Ok(next)
}
