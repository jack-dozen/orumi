//! Snapshot of open windows + thumbnails for Activities (taken when it opens).
//!
//! Thumbnails come from `capture`, shared with the bar preview: small JPEG files in the app cache
//! that the UI loads through the asset protocol. Minimized, unresponsive, or uncapturable windows
//! use their most recent successful thumbnail. If there is none, `thumb` = None and React shows
//! a letter icon.

use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{capture, winlist::WindowEntry};

/// Maximum number of windows to capture per opening (the rest only use what is already cached).
const MAX_CAPTURE: usize = 24;

/// One window for React: WindowEntry fields (hwnd, title, exe, minimized, focused) + thumb.
#[derive(Serialize)]
pub struct SnapWindow {
    #[serde(flatten)]
    pub entry: WindowEntry,
    /// Absolute path of the thumbnail file (the UI loads it with convertFileSrc).
    pub thumb: Option<String>,
}

pub fn snapshot(app: &AppHandle, entries: Vec<WindowEntry>) -> Vec<SnapWindow> {
    let Some(dir) = capture::cache_dir(app) else {
        return entries
            .into_iter()
            .map(|entry| SnapWindow { entry, thumb: None })
            .collect();
    };

    let n = entries.len().min(MAX_CAPTURE);
    let mut paths: Vec<Option<PathBuf>> = entries[..n]
        .par_iter()
        .map(|e| capture::refresh(&dir, e.hwnd))
        .collect();
    paths.extend(entries[n..].iter().map(|e| capture::latest(&dir, e.hwnd)));

    let scope = app.asset_protocol_scope();
    entries
        .into_iter()
        .zip(paths)
        .map(|(entry, path)| {
            let thumb = path.map(|p| {
                let _ = scope.allow_file(&p);
                p.to_string_lossy().into_owned()
            });
            SnapWindow { entry, thumb }
        })
        .collect()
}
