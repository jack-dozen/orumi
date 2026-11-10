//! Snapshot window terbuka + thumbnail statis (PrintWindow, diambil saat Activities dibuka).
//!
//! Window yang diperkecil, tidak merespons, atau gagal ditangkap memakai thumbnail terakhir
//! yang berhasil (cache per hwnd). Kalau belum pernah ada, `thumb` = None dan React
//! menampilkan ikon huruf.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{collections::HashMap, ffi::c_void, io::Cursor, mem::size_of, sync::Mutex};

use base64::{engine::general_purpose::STANDARD, Engine};
use rayon::prelude::*;
use serde::Serialize;
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    },
    Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
    UI::WindowsAndMessaging::{GetWindowRect, IsHungAppWindow, IsIconic},
};

use crate::winlist::WindowEntry;

const THUMB_W: u32 = 420;
const THUMB_H: u32 = 280;
/// Batas jumlah window yang ditangkap per pembukaan (sisanya hanya memakai cache).
const MAX_CAPTURE: usize = 24;
/// PW_RENDERFULLCONTENT: ikut menangkap window berbasis GPU (Chrome, Electron, UWP).
const PW_RENDERFULLCONTENT: u32 = 2;

/// Satu window untuk React: field WindowEntry (hwnd, title, exe, minimized, focused) + thumb.
#[derive(Serialize)]
pub struct SnapWindow {
    #[serde(flatten)]
    pub entry: WindowEntry,
    /// JPEG base64 tanpa prefix "data:".
    pub thumb: Option<String>,
}

static CACHE: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);

pub fn snapshot(entries: Vec<WindowEntry>) -> Vec<SnapWindow> {
    let n = entries.len().min(MAX_CAPTURE);
    let fresh: HashMap<i64, String> = entries[..n]
        .par_iter()
        .filter_map(|e| unsafe { capture(e.hwnd) }.map(|t| (e.hwnd, t)))
        .collect();

    let mut cache = CACHE
        .lock()
        .ok()
        .and_then(|mut c| c.take())
        .unwrap_or_default();
    cache.retain(|hwnd, _| entries.iter().any(|e| e.hwnd == *hwnd));
    cache.extend(fresh);

    let out = entries
        .into_iter()
        .map(|entry| {
            let thumb = cache.get(&entry.hwnd).cloned();
            SnapWindow { entry, thumb }
        })
        .collect();

    if let Ok(mut c) = CACHE.lock() {
        *c = Some(cache);
    }
    out
}

unsafe fn capture(hwnd: i64) -> Option<String> {
    let h = HWND(hwnd as isize as *mut c_void);
    if IsIconic(h).as_bool() || IsHungAppWindow(h).as_bool() {
        return None;
    }
    let mut r = RECT::default();
    GetWindowRect(h, &mut r).ok()?;
    let (w, ht) = (r.right - r.left, r.bottom - r.top);
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&ht) {
        return None;
    }

    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let bmp = CreateCompatibleBitmap(screen, w, ht);
    let old = SelectObject(mem, bmp.into());
    let printed = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
    // GetDIBits mensyaratkan bitmap tidak sedang dipilih di DC mana pun.
    SelectObject(mem, old);

    let mut pixels = vec![0u8; (w as usize) * (ht as usize) * 4];
    let lines = if printed {
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -ht, // negatif = baris dari atas ke bawah
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };
        GetDIBits(
            screen,
            bmp,
            0,
            ht as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        )
    } else {
        0
    };

    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    if lines == 0 {
        return None;
    }

    // BGRA -> RGBA, alpha dipaksa buram. Hasil hitam total = gagal (cloaked/terproteksi).
    let mut any = false;
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
        if px[0] | px[1] | px[2] != 0 {
            any = true;
        }
    }
    if !any {
        return None;
    }

    let img = image::RgbaImage::from_raw(w as u32, ht as u32, pixels)?;
    let small = image::DynamicImage::ImageRgba8(img)
        .thumbnail(THUMB_W, THUMB_H)
        .to_rgb8();
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(small)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .ok()?;
    Some(STANDARD.encode(buf))
}
