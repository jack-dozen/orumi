//! Window thumbnail preview when hovering over an app icon in the bar. Thumbnails come from
//! `capture` (shared with Activities) and the UI loads them through the asset protocol.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    fs,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::IsIconic};

use crate::{capture, edge::Panels};

// Card dimensions (CSS px, must match CSS .pv*): thumbnail, spacing, title.
const PV_W: f64 = 220.0;
const PV_H: f64 = 124.0;
const PV_PAD: f64 = 12.0;
const PV_GAP: f64 = 10.0;
const PV_TITLE: f64 = 28.0;
const MAX_ITEMS: usize = 4;

static GEN: AtomicU64 = AtomicU64::new(0);
/// Bumped by every preview_open; lets a slow background refresh detect that it was superseded.
static OPEN_SEQ: AtomicU64 = AtomicU64::new(0);
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

/// Items built only from what is already on disk (last thumbnail, if any). No capture, so the
/// card can appear immediately.
fn cached_items(dir: &Path, reqs: &[PreviewReq]) -> Vec<PreviewItem> {
    reqs.iter()
        .map(|r| PreviewItem {
            hwnd: r.hwnd,
            title: r.title.clone(),
            icon: r.icon.clone(),
            thumb: capture::latest(dir, r.hwnd).map(|p| p.to_string_lossy().into_owned()),
            minimized: unsafe { IsIconic(HWND(r.hwnd as isize as *mut c_void)).as_bool() },
        })
        .collect()
}

/// Captures fresh thumbnails in parallel. `capture::refresh` reuses a thumbnail younger than its
/// TTL and falls back to the last good one for minimized, hung, or protected windows.
fn build_items(dir: &Path, reqs: Vec<PreviewReq>) -> Vec<PreviewItem> {
    use rayon::prelude::*;

    reqs.into_par_iter()
        .map(|r| {
            let minimized = unsafe { IsIconic(HWND(r.hwnd as isize as *mut c_void)).as_bool() };
            let thumb = capture::refresh(dir, r.hwnd).map(|p| p.to_string_lossy().into_owned());
            PreviewItem {
                hwnd: r.hwnd,
                title: r.title,
                icon: r.icon,
                thumb,
                minimized,
            }
        })
        .collect()
}

/// Card dimensions (CSS px) for n windows.
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

/// Whether the cursor is over the bar window and over the preview card (physical px).
fn cursor_state(app: &AppHandle) -> Option<(bool, bool)> {
    let bar = app.get_webview_window("bar")?;
    let c = bar.cursor_position().ok()?;
    let p = bar.outer_position().ok()?;
    let s = bar.outer_size().ok()?;
    let over_bar = c.x >= p.x as f64
        && c.x < p.x as f64 + s.width as f64
        && c.y >= p.y as f64
        && c.y < p.y as f64 + s.height as f64;
    let over_card = crate::edge::hit_rect("preview")
        .is_some_and(|(x, y, w, h)| c.x >= x && c.x < x + w && c.y >= y && c.y < y + h);
    Some((over_bar, over_card))
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
    // IPC calls can be reordered: when this request arrives after the cursor already left, the
    // matching preview_close_soon has run and can no longer be cancelled by anything. Drop the
    // request and make sure no preview stays open (unless the cursor is on the card itself).
    if let Some((over_bar, over_card)) = cursor_state(&app) {
        if !over_bar {
            if !over_card {
                close_now(&app);
            }
            return Ok(());
        }
    }
    let reqs: Vec<PreviewReq> = items.into_iter().take(MAX_ITEMS).collect();
    if reqs.is_empty() {
        return Ok(());
    }
    let Some(dir) = capture::cache_dir(&app) else { return Ok(()) };
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

/// Cursor returned to the preview area or bar: cancel the pending close.
#[tauri::command]
pub fn preview_keep() {
    GEN.fetch_add(1, Ordering::SeqCst);
}

/// Close shortly, unless preview_keep or preview_open is called before then.
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
