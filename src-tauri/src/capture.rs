//! Window thumbnail capture shared by the bar preview and Activities.
//!
//! Uses PrintWindow (Bloom dock pattern, GPL-3.0: utils.rs). Every capture is written as a small
//! JPEG named `<hwnd>_<seq>.jpg` in the app cache and the UI loads the files through the asset
//! protocol. A new file name per capture bypasses image caching, and the newest file of a window
//! doubles as the cache that is shown while a fresh capture is in progress.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    fs,
    io::Cursor,
    mem::size_of,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tauri::{AppHandle, Manager};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
            ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
            RGBQUAD,
        },
    },
    Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
    UI::WindowsAndMessaging::{GetWindowRect, IsHungAppWindow, IsIconic, IsWindow},
};

/// Thumbnails fit inside this box (aspect ratio kept). Wide enough for 2x of the bar preview card
/// (220 CSS px) and tall enough for the Activities window cards.
const THUMB_W: u32 = 440;
const THUMB_H: u32 = 280;
/// PW_RENDERFULLCONTENT: also captures GPU-based windows (Chrome, Electron, UWP).
const PW_RENDERFULLCONTENT: u32 = 0x2;

/// A thumbnail younger than this is reused instead of capturing the window again.
const CACHE_TTL: Duration = Duration::from_millis(1500);

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn cache_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|d| d.join("previews"))
}

/// Clear the cache when the app starts: Windows may reuse window IDs.
pub fn clear_cache(app: &AppHandle) {
    if let Some(dir) = cache_dir(app) {
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
    }
}

fn files_for(dir: &Path, hwnd: i64) -> Vec<PathBuf> {
    let prefix = format!("{hwnd}_");
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix) && n.ends_with(".jpg"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

/// Newest cached thumbnail of a window, if there is one. Never captures.
pub fn latest(dir: &Path, hwnd: i64) -> Option<PathBuf> {
    files_for(dir, hwnd).pop()
}

fn is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < CACHE_TTL)
}

/// Returns the newest thumbnail file of a window. A new capture is taken first, unless the last
/// one is younger than CACHE_TTL. When the capture is skipped or fails (minimized, hung, protected
/// or fully black window), the last good thumbnail is returned instead; None if there never was one.
pub fn refresh(dir: &Path, hwnd: i64) -> Option<PathBuf> {
    let cached = latest(dir, hwnd);
    if cached.as_deref().is_some_and(is_fresh) {
        return cached;
    }
    let Some(jpg) = (unsafe { capture_jpeg(hwnd) }) else {
        return cached;
    };

    let _ = fs::create_dir_all(dir);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let file = dir.join(format!("{hwnd}_{seq:010}.jpg"));
    if fs::write(&file, jpg).is_err() {
        return cached;
    }
    for old in files_for(dir, hwnd) {
        if old != file {
            let _ = fs::remove_file(old);
        }
    }
    Some(file)
}

/// Capture the window contents as a small JPEG. None if minimized, hung, or the capture fails.
unsafe fn capture_jpeg(hwnd_raw: i64) -> Option<Vec<u8>> {
    let hwnd = HWND(hwnd_raw as isize as *mut c_void);
    if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() || IsHungAppWindow(hwnd).as_bool()
    {
        return None;
    }

    let mut win = RECT::default();
    GetWindowRect(hwnd, &mut win).ok()?;
    let (w, h) = (win.right - win.left, win.bottom - win.top);
    if w <= 10 || h <= 10 || w > 7680 || h > 4320 {
        return None;
    }

    // Actual visible bounds (excluding invisible shadows/borders).
    let mut frame = RECT::default();
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut frame as *mut RECT as *mut c_void,
        size_of::<RECT>() as u32,
    );

    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
    let hbm = CreateCompatibleBitmap(hdc_screen, w, h);
    let old = SelectObject(hdc_mem, hbm.into());
    let printed = PrintWindow(hwnd, hdc_mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();

    let mut result = None;
    if printed {
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        if GetDIBits(
            hdc_mem,
            hbm,
            0,
            h as u32,
            Some(px.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        ) != 0
        {
            // BGRA -> RGBA; alpha is not meaningful here. A fully black result means the
            // capture failed (cloaked or protected window).
            let mut any_pixel = false;
            for c in px.chunks_exact_mut(4) {
                c.swap(0, 2);
                c[3] = 255;
                if c[0] | c[1] | c[2] != 0 {
                    any_pixel = true;
                }
            }
            if let Some(img) =
                image::RgbaImage::from_raw(w as u32, h as u32, px).filter(|_| any_pixel)
            {
                let (mut cx, mut cy, mut cw, mut ch) = (0u32, 0u32, img.width(), img.height());
                if frame.right > frame.left && frame.bottom > frame.top {
                    let x = (frame.left - win.left).max(0) as u32;
                    let y = (frame.top - win.top).max(0) as u32;
                    let fw = ((frame.right - frame.left) as u32).min(img.width().saturating_sub(x));
                    let fh = ((frame.bottom - frame.top) as u32).min(img.height().saturating_sub(y));
                    if fw > 10 && fh > 10 {
                        (cx, cy, cw, ch) = (x, y, fw, fh);
                    }
                }
                let cropped = image::imageops::crop_imm(&img, cx, cy, cw, ch).to_image();
                let rgb = image::DynamicImage::ImageRgba8(cropped)
                    .thumbnail(THUMB_W, THUMB_H)
                    .to_rgb8();
                let mut out = Cursor::new(Vec::new());
                if image::DynamicImage::ImageRgb8(rgb)
                    .write_to(&mut out, image::ImageFormat::Jpeg)
                    .is_ok()
                {
                    result = Some(out.into_inner());
                }
            }
        }
    }

    SelectObject(hdc_mem, old);
    let _ = DeleteObject(hbm.into());
    let _ = DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
    result
}
