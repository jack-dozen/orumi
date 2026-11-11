//! Pratinjau thumbnail window saat ikon app di bar di-hover. Tangkapan memakai PrintWindow
//! (pola dock Bloom, GPL-3.0: utils.rs), disimpan sebagai JPEG kecil di cache, dan dimuat UI
//! lewat asset protocol.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    fs,
    io::Cursor,
    mem::size_of,
    path::{Path, PathBuf},
    sync::{atomic::{AtomicU64, Ordering}, Mutex},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
            ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
            RGBQUAD,
        },
    },
    UI::WindowsAndMessaging::{GetWindowRect, IsHungAppWindow, IsIconic, IsWindow},
};

use crate::edge::Panels;

/// PrintWindow tidak ada di fitur crate yang kita pakai: deklarasikan langsung.
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdc_blt: HDC, flags: u32) -> i32;
}

const PW_RENDERFULLCONTENT: u32 = 0x2;

// Ukuran kartu (px CSS, harus sama dengan CSS .pv*): thumbnail, ruang, judul.
const PV_W: f64 = 220.0;
const PV_H: f64 = 124.0;
const PV_PAD: f64 = 12.0;
const PV_GAP: f64 = 10.0;
const PV_TITLE: f64 = 28.0;
const MAX_ITEMS: usize = 4;
/// Tangkapan disimpan 2x lebih besar dari kartu supaya tajam di layar DPI tinggi.
const CAP_W: u32 = 440;
const CAP_H: u32 = 248;

static GEN: AtomicU64 = AtomicU64::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Bumped by every preview_open; lets a slow background refresh detect that it was superseded.
static OPEN_SEQ: AtomicU64 = AtomicU64::new(0);
/// A thumbnail younger than this is reused instead of capturing the window again.
const CACHE_TTL: Duration = Duration::from_millis(1500);
/// Last window rect sent to the preview window (x, y, w, h), to skip redundant moves.
static LAST_WIN: Mutex<Option<[i64; 4]>> = Mutex::new(None);

#[derive(Deserialize)]
pub struct PreviewReq {
    hwnd: i64,
    title: String,
    icon: Option<String>,
}

#[derive(Serialize, Clone)]
struct PreviewItem {
    hwnd: i64,
    title: String,
    icon: Option<String>,
    thumb: Option<String>,
    minimized: bool,
}

/// Event payload for the preview window.
#[derive(Serialize, Clone)]
struct PreviewPayload {
    items: Vec<PreviewItem>,
    /// Card geometry inside the preview window, in CSS px.
    top: f64,
    width: f64,
    height: f64,
}

/// Where the card sits inside the preview window (CSS px).
struct CardGeom {
    top: f64,
    width: f64,
    height: f64,
}

fn cache_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|d| d.join("previews"))
}

/// Kosongkan cache saat app mulai: id window bisa dipakai ulang oleh Windows.
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

/// Tangkap isi window menjadi JPEG kecil. None kalau diperkecil atau gagal.
unsafe fn capture_jpeg(hwnd_raw: i64) -> Option<Vec<u8>> {
    let hwnd = HWND(hwnd_raw as isize as *mut c_void);
    if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
        return None;
    }

    let mut win = RECT::default();
    GetWindowRect(hwnd, &mut win).ok()?;
    let (w, h) = (win.right - win.left, win.bottom - win.top);
    if w <= 10 || h <= 10 || w > 7680 || h > 4320 {
        return None;
    }

    // Batas yang benar-benar terlihat (tanpa bayangan/bingkai tak terlihat).
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
    let printed = PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT) != 0;

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
            // BGRA -> RGBA, alpha tidak bermakna di sini.
            for c in px.chunks_exact_mut(4) {
                c.swap(0, 2);
                c[3] = 255;
            }
            if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, px) {
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
                    .thumbnail(CAP_W, CAP_H)
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

/// Items built only from what is already on disk (last thumbnail, if any). No capture, so the
/// card can appear immediately.
fn cached_items(dir: &Path, reqs: &[PreviewReq]) -> Vec<PreviewItem> {
    reqs.iter()
        .map(|r| PreviewItem {
            hwnd: r.hwnd,
            title: r.title.clone(),
            icon: r.icon.clone(),
            thumb: files_for(dir, r.hwnd)
                .pop()
                .map(|p| p.to_string_lossy().into_owned()),
            minimized: unsafe { IsIconic(HWND(r.hwnd as isize as *mut c_void)).as_bool() },
        })
        .collect()
}

/// Captures fresh thumbnails (in parallel). Windows whose last thumbnail is younger than
/// CACHE_TTL, minimized windows, and hung windows are not captured.
fn build_items(dir: &Path, reqs: Vec<PreviewReq>) -> Vec<PreviewItem> {
    use rayon::prelude::*;

    let _ = fs::create_dir_all(dir);
    reqs.into_par_iter()
        .map(|r| {
            let hwnd = HWND(r.hwnd as isize as *mut c_void);
            let minimized = unsafe { IsIconic(hwnd).as_bool() };

            let cached = files_for(dir, r.hwnd).pop();
            let fresh = cached.as_ref().is_some_and(|p| {
                fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age < CACHE_TTL)
            });

            let mut thumb: Option<PathBuf> = None;
            if !minimized && !fresh && !unsafe { IsHungAppWindow(hwnd).as_bool() } {
                if let Some(jpg) = unsafe { capture_jpeg(r.hwnd) } {
                    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
                    // The file name changes on every capture: the most reliable way to bypass image caching.
                    let file = dir.join(format!("{}_{:010}.jpg", r.hwnd, seq));
                    if fs::write(&file, jpg).is_ok() {
                        for old in files_for(dir, r.hwnd) {
                            if old != file {
                                let _ = fs::remove_file(old);
                            }
                        }
                        thumb = Some(file);
                    }
                }
            }
            // Minimized, fresh enough, hung or failed: use the last capture if there is one.
            if thumb.is_none() {
                thumb = cached;
            }

            PreviewItem {
                hwnd: r.hwnd,
                title: r.title,
                icon: r.icon,
                thumb: thumb.map(|p| p.to_string_lossy().into_owned()),
                minimized,
            }
        })
        .collect()
}

/// Ukuran kartu (px CSS) untuk n window.
fn card_size(n: usize) -> (f64, f64) {
    let n = n as f64;
    (2.0 * PV_PAD + n * PV_W + (n - 1.0) * PV_GAP, 2.0 * PV_PAD + PV_H + PV_TITLE)
}

/// Keeps the preview window fixed (right of the bar, as tall as the bar, wide enough for
/// MAX_ITEMS cards) and returns where the card should sit inside it. The card is moved with CSS
/// so it can animate between icons; edge.rs only needs the card rectangle as its hit area.
fn position(app: &AppHandle, n: usize, anchor_y: f64) -> Option<CardGeom> {
    let bar = app.get_webview_window("bar")?;
    let pos = bar.outer_position().ok()?;
    let size = bar.outer_size().ok()?;
    let scale = bar.scale_factor().ok()?;
    let u = scale * crate::edge::ui_scale();

    let (card_w, card_h) = card_size(n);
    let (max_w, _) = card_size(MAX_ITEMS);
    let win_x = pos.x as f64 + size.width as f64;
    let win_y = pos.y as f64;
    let win_w = max_w * u;
    let win_h = size.height as f64;

    let win_h_css = win_h / u;
    let top = (anchor_y - card_h / 2.0).clamp(8.0, (win_h_css - card_h - 8.0).max(8.0));

    crate::edge::set_hit_rect("preview", Some((win_x, win_y + top * u, card_w * u, card_h * u)));

    // Only move or resize the window when it actually changed.
    let rect = [
        win_x.round() as i64,
        win_y.round() as i64,
        win_w.round() as i64,
        win_h.round() as i64,
    ];
    let changed = LAST_WIN
        .lock()
        .map(|mut l| {
            let c = *l != Some(rect);
            *l = Some(rect);
            c
        })
        .unwrap_or(true);
    if changed {
        crate::edge::set_free_rect("preview", Some((win_x, win_y, win_w, win_h)));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || crate::place_panel(&a, "preview"));
    }

    Some(CardGeom { top, width: card_w, height: card_h })
}

/// Grants asset access to the thumbnails, positions the card, and sends the payload to the UI.
fn emit_items(app: &AppHandle, items: Vec<PreviewItem>, anchor_y: f64) -> Option<()> {
    let scope = app.asset_protocol_scope();
    for it in &items {
        if let Some(t) = &it.thumb {
            let _ = scope.allow_file(t);
        }
    }
    let geom = position(app, items.len(), anchor_y)?;
    let payload = PreviewPayload {
        items,
        top: geom.top,
        width: geom.width,
        height: geom.height,
    };
    let _ = app.emit_to("preview", "preview-data", &payload);
    Some(())
}

fn close_now(app: &AppHandle) {
    GEN.fetch_add(1, Ordering::SeqCst);
    app.state::<Panels>().set("preview", false);
}

/// Called by the bar after the cursor rests on an app icon.
/// Cache-first: the card appears right away with the last known thumbnails, then fresh
/// thumbnails are captured in the background and swapped in (the UI preloads them).
#[tauri::command]
pub async fn preview_open(
    items: Vec<PreviewReq>,
    anchor_y: f64,
    app: AppHandle,
) -> Result<(), String> {
    // Cancels a pending preview_close_soon.
    GEN.fetch_add(1, Ordering::SeqCst);
    let open_seq = OPEN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let reqs: Vec<PreviewReq> = items.into_iter().take(MAX_ITEMS).collect();
    if reqs.is_empty() {
        return Ok(());
    }
    let Some(dir) = cache_dir(&app) else { return Ok(()) };
    let _ = fs::create_dir_all(&dir);

    // 1. Show now, using only what is already on disk.
    let first = cached_items(&dir, &reqs);
    if emit_items(&app, first.clone(), anchor_y).is_none() {
        return Ok(());
    }
    app.state::<Panels>().set_transient("preview");

    // 2. Capture in the background.
    let dir_bg = dir.clone();
    let built = tauri::async_runtime::spawn_blocking(move || build_items(&dir_bg, reqs))
        .await
        .map_err(|e| e.to_string())?;

    // Superseded by a newer preview_open, or closed in the meantime: drop the result.
    if OPEN_SEQ.load(Ordering::SeqCst) != open_seq || !app.state::<Panels>().is_open("preview") {
        return Ok(());
    }
    let changed = built
        .iter()
        .zip(&first)
        .any(|(a, b)| a.thumb != b.thumb || a.minimized != b.minimized);
    if changed {
        let _ = emit_items(&app, built, anchor_y);
    }
    Ok(())
}

/// Kursor kembali ke area pratinjau atau bar: batalkan penutupan yang tertunda.
#[tauri::command]
pub fn preview_keep() {
    GEN.fetch_add(1, Ordering::SeqCst);
}

/// Tutup sebentar lagi, kecuali ada preview_keep atau preview_open sebelum itu.
#[tauri::command]
pub fn preview_close_soon(app: AppHandle) {
    let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(260));
        if GEN.load(Ordering::SeqCst) == generation {
            close_now(&app);
        }
    });
}

#[tauri::command]
pub fn preview_close(app: AppHandle) {
    close_now(&app);
}
