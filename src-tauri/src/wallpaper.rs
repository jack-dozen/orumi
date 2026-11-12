//! Wallpaper picker: image list and thumbnails (cached on disk and loaded through the asset protocol),
//! and desktop wallpaper changes. The cache and asset protocol approach was inspired by the
//! "carousel" project (rewritten, not copied).

use std::{
    collections::hash_map::DefaultHasher,
    ffi::c_void,
    fs,
    hash::{Hash, Hasher},
    io::Cursor,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use rayon::{prelude::*, ThreadPoolBuilder};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_ITEMS: usize = 400;
/// Images larger than this are skipped (they may use too much memory when decoded).
const MAX_FILE_BYTES: u64 = 80_000_000;
const THUMB_W: u32 = 400;
const THUMB_H: u32 = 225;

#[derive(Serialize)]
pub struct WallpaperItem {
    name: String,
    path: String,
    /// Thumbnail file path on disk (loaded by the frontend through the asset protocol). Empty = not available.
    thumb: String,
}

#[derive(Serialize)]
pub struct WallpaperList {
    dir: String,
    items: Vec<WallpaperItem>,
    /// Index of the active wallpaper (if it is in this folder).
    start: Option<usize>,
    error: Option<String>,
}

fn failed(dir: String, msg: &str) -> WallpaperList {
    WallpaperList { dir, items: Vec::new(), start: None, error: Some(msg.to_string()) }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "bmp"))
        .unwrap_or(false)
}

/// Normalize path format for comparison (\\?\ prefix, slash direction, case).
fn norm(p: &str) -> String {
    p.trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}

fn cores() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(2)
}

fn resolve_dir(app: &AppHandle, configured: &str) -> Option<PathBuf> {
    if configured.is_empty() {
        app.path().picture_dir().ok()
    } else {
        Some(PathBuf::from(configured))
    }
}

fn scan_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_absolute() {
        return Err("Gunakan path lengkap, misalnya D:\\Wallpapers".into());
    }
    let entries = fs::read_dir(dir)
        .map_err(|_| "Folder tidak ditemukan atau tidak bisa dibaca".to_string())?;

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    files.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    files.truncate(MAX_ITEMS);
    Ok(files)
}

// ---------- Thumbnail ----------

/// Thumbnail location in the cache. Keyed by path + size + modification time, so a changed image
/// is automatically regenerated.
fn thumb_target(cache_dir: &Path, path: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    (path.to_string_lossy().as_ref(), meta.len(), secs).hash(&mut hasher);
    Some(cache_dir.join(format!("{:016x}.jpg", hasher.finish())))
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create a thumbnail if none exists. true = the thumbnail file is ready to use.
fn ensure_thumb(path: &Path, target: &Path) -> bool {
    if fs::metadata(target).map(|m| m.len() > 0).unwrap_or(false) {
        return true;
    }
    let Some(img) = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.decode().ok())
    else {
        return false;
    };
    let thumb = image::DynamicImage::ImageRgb8(img.thumbnail(THUMB_W, THUMB_H).to_rgb8());
    let mut buf = Vec::new();
    if thumb
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .is_err()
    {
        return false;
    }
    // Write through a temporary file so the cache never contains an incomplete file.
    let tmp = target.with_extension(format!("{}.tmp", TMP_SEQ.fetch_add(1, Ordering::Relaxed)));
    fs::write(&tmp, &buf).is_ok() && fs::rename(&tmp, target).is_ok()
}

/// Create all missing thumbnails in parallel with a limited number of threads
/// so the computer remains responsive.
fn make_thumbs(files: &[PathBuf], cache_dir: &Path, threads: usize) -> Vec<Option<PathBuf>> {
    let _ = fs::create_dir_all(cache_dir);
    let work = |p: &PathBuf| {
        let target = thumb_target(cache_dir, p)?;
        ensure_thumb(p, &target).then_some(target)
    };
    match ThreadPoolBuilder::new().num_threads(threads.max(1)).build() {
        Ok(pool) => pool.install(|| files.par_iter().map(&work).collect()),
        Err(_) => files.iter().map(&work).collect(),
    }
}

/// Prepare thumbnails in the background a few seconds after the app starts (and after the folder
/// changes) so the wallpaper picker is usually ready as soon as it opens.
pub fn warm(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(4));
        let cfg = crate::config::current(&app);
        let Some(dir) = resolve_dir(&app, &cfg.wallpaper_dir) else { return };
        let Some(cache) = app.path().app_cache_dir().ok().map(|d| d.join("thumbs")) else {
            return;
        };
        let Ok(files) = scan_files(&dir) else { return };
        let _ = make_thumbs(&files, &cache, (cores() / 4).max(1));
    });
}

// ---------- List ----------

/// List images in the wallpaper folder. thumbs = false only counts them (for settings status)
/// without creating thumbnails.
#[tauri::command]
pub async fn list_wallpapers(thumbs: bool, app: AppHandle) -> Result<WallpaperList, String> {
    let cfg = crate::config::current(&app);
    let dir = resolve_dir(&app, &cfg.wallpaper_dir);
    let cache = app.path().app_cache_dir().ok().map(|d| d.join("thumbs"));
    let current = crate::theme::current_wallpaper_path();
    let last = cfg.last_wallpaper.clone();
    let scope_app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let Some(dir) = dir else {
            return failed(String::new(), "Folder Pictures tidak ditemukan");
        };
        let shown = dir.to_string_lossy().into_owned();
        let files = match scan_files(&dir) {
            Ok(f) => f,
            Err(e) => return failed(shown, &e),
        };

        let thumb_paths: Vec<Option<PathBuf>> = match (&cache, thumbs) {
            (Some(c), true) => make_thumbs(&files, c, (cores() / 2).max(1)),
            _ => vec![None; files.len()],
        };

        // Allow the WebView to read thumbnail files through the asset protocol, one at a time.
        let scope = scope_app.asset_protocol_scope();
        for t in thumb_paths.iter().flatten() {
            let _ = scope.allow_file(t);
        }

        // Initial selection: wallpaper active according to Windows, or the last one applied through this app.
        let normalized: Vec<String> = files.iter().map(|p| norm(&p.to_string_lossy())).collect();
        let find = |target: &str| {
            let t = norm(target);
            normalized.iter().position(|p| *p == t)
        };
        let start = current
            .as_deref()
            .and_then(find)
            .or_else(|| if last.is_empty() { None } else { find(&last) });

        let items = files
            .iter()
            .zip(thumb_paths.iter())
            .map(|(p, t)| WallpaperItem {
                name: p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: p.to_string_lossy().into_owned(),
                thumb: t
                    .as_ref()
                    .map(|t| t.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            })
            .collect();

        WallpaperList { dir: shown, items, start, error: None }
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------- Change wallpaper ----------

#[tauri::command]
pub fn set_wallpaper(path: String, app: AppHandle) -> Result<(), String> {
    let p = Path::new(&path);
    if !is_image(p) || !p.is_file() {
        return Err("File wallpaper tidak valid".into());
    }
    let mut wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_mut_ptr().cast::<c_void>()),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .map_err(|e| e.to_string())?;
    }
    // Windows sometimes reports an internal copy instead of the original file: also save the path
    // we applied so the picker can open at the correct position.
    crate::config::remember_wallpaper(&app, &path);

    // Windows updates the wallpaper file asynchronously: wait before recalculating colors.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1200));
        let _ = app.emit("theme-refresh", ());
    });
    Ok(())
}

/// "external" mode: run the user's chosen wallpaper-picker command (from config).
#[tauri::command]
pub fn run_wallpaper_command(app: AppHandle) -> Result<(), String> {
    let cmd = crate::config::current(&app).wallpaper_command;
    if cmd.trim().is_empty() {
        return Err("Perintah belum diatur di Settings → Aplikasi".into());
    }
    Command::new("cmd")
        .args(["/C", cmd.as_str()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
