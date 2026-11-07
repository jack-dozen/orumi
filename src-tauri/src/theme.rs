//! Wallpaper aktif dan penyimpanan tema (token warna) yang dibagi ke semua window.

use std::{ffi::c_void, fs, sync::Mutex, time::UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine};
use tauri::{AppHandle, Emitter};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Tema terakhir (JSON token), supaya window yang baru memuat bisa langsung memakainya.
static THEME: Mutex<Option<String>> = Mutex::new(None);

/// Path file wallpaper aktif, atau None kalau warna polos / tidak ada.
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

/// Path wallpaper aktif menurut Windows (untuk modul lain).
pub fn current_wallpaper_path() -> Option<String> {
    wallpaper_path()
}

/// Penanda murah untuk mendeteksi wallpaper berganti (path + waktu ubah + ukuran).
#[tauri::command]
pub fn wallpaper_stamp() -> Option<String> {
    let path = wallpaper_path()?;
    let meta = fs::metadata(&path).ok()?;
    let secs = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{path}|{secs}|{}", meta.len()))
}

/// Isi wallpaper sebagai data URL. Hanya dipanggil saat stempelnya berubah.
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

/// Dipanggil window notch setelah menghitung palet: simpan dan siarkan ke semua window.
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
