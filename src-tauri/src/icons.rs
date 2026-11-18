//! App icons: extracted through IShellItemImageFactory (works for both Win32 and
//! Store/UWP apps), saved as PNGs in the cache, and loaded by the UI through the asset protocol.
//! The extraction pattern is adapted from Bloom (GPL-3.0): commands.rs and utils.rs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    ffi::c_void,
    fs,
    hash::{Hash, Hasher},
    io::Cursor,
    mem::size_of,
    path::PathBuf,
    sync::{mpsc, Mutex, OnceLock},
    thread,
};

use tauri::{AppHandle, Manager};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, FILETIME, HWND, LPARAM, SIZE, WPARAM},
        Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, RGBQUAD,
        },
        System::{
            Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
            Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            Shell::{
                ExtractIconExW, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF,
                SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
            },
            WindowsAndMessaging::{
                GetClassLongPtrW, GetIconInfo, GetWindowThreadProcessId, IsWindow,
                SendMessageTimeoutW, GCLP_HICON, GCLP_HICONSM, HICON, ICONINFO, ICON_BIG,
                ICON_SMALL, ICON_SMALL2, SMTO_ABORTIFHUNG, WM_GETICON,
            },
        },
    },
};

/// Requested icon size (px). 128 is sharp enough for icons up to ~64 CSS px on high-DPI displays.
const ICON_SIZE: i32 = 128;
/// Increment to invalidate the old cache (e.g. if the color conversion method changes).
const ICON_VERSION: u32 = 3;

type Reply = mpsc::Sender<Option<PathBuf>>;
static TX: OnceLock<Mutex<mpsc::Sender<(String, Reply)>>> = OnceLock::new();

fn hash_key(target: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (ICON_VERSION, target).hash(&mut h);
    h.finish()
}

/// "aumid:<AppID>" -> shell:AppsFolder\<AppID>; "path:<path>" -> the path itself.
fn parsing_name(target: &str) -> Option<String> {
    if let Some(id) = target.strip_prefix("aumid:") {
        let id = id.trim();
        (!id.is_empty()).then(|| format!("shell:AppsFolder\\{id}"))
    } else if let Some(p) = target.strip_prefix("path:") {
        let p = p.trim();
        (!p.is_empty()).then(|| p.to_string())
    } else {
        None
    }
}

/// Shared 32-bit HBITMAP -> BGRA pixel buffer reader.
unsafe fn read_bitmap_bgra(hbitmap: HBITMAP) -> Option<(i32, i32, Vec<u8>)> {
    let mut bm: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        hbitmap.into(),
        size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut BITMAP as *mut c_void),
    );
    if got == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if w <= 0 || h <= 0 || w > 512 || h > 512 {
        return None;
    }

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative = rows run from top to bottom
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
    let hdc = GetDC(None);
    let rows = GetDIBits(
        hdc,
        hbitmap,
        0,
        h as u32,
        Some(px.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    ReleaseDC(None, hdc);
    if rows == 0 {
        return None;
    }
    Some((w, h, px))
}

/// 32-bit bitmap from Shell (premultiplied alpha, BGRA order) -> PNG (regular alpha, RGBA).
unsafe fn hbitmap_to_png(hbitmap: HBITMAP) -> Option<Vec<u8>> {
    let (w, h, mut px) = read_bitmap_bgra(hbitmap)?;

    // Older icons without an alpha channel have all alpha values set to 0. Treat them as fully opaque.
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    for p in px.chunks_exact_mut(4) {
        let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
        if !has_alpha {
            p.copy_from_slice(&[r, g, b, 255]);
            continue;
        }
        let un = |c: u8| -> u8 {
            if a == 0 {
                0
            } else {
                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
            }
        };
        p.copy_from_slice(&[un(r), un(g), un(b), a]);
    }

    let img = image::RgbaImage::from_raw(w as u32, h as u32, px)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// HICON -> PNG (preserving straight alpha or applying the 1-bit mask bitmap when alpha is absent).
unsafe fn hicon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    let mut ii = ICONINFO::default();
    if GetIconInfo(hicon, &mut ii).is_err() {
        return None;
    }
    let result = (|| {
        if ii.hbmColor.0.is_null() {
            return None;
        }
        let (w, h, mut px) = read_bitmap_bgra(ii.hbmColor)?;
        let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
        if has_alpha {
            for p in px.chunks_exact_mut(4) {
                let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
                p.copy_from_slice(&[r, g, b, a]);
            }
        } else {
            let mask = (!ii.hbmMask.0.is_null())
                .then(|| read_bitmap_bgra(ii.hbmMask))
                .flatten()
                .filter(|(mw, mh, _)| *mw == w && *mh == h);
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                let (b, g, r) = (p[0], p[1], p[2]);
                let a = match &mask {
                    Some((_, _, m)) if m[i * 4] > 127 => 0,
                    _ => 255,
                };
                p.copy_from_slice(&[r, g, b, a]);
            }
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, px)?;
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    })();
    if !ii.hbmColor.0.is_null() {
        let _ = DeleteObject(ii.hbmColor.into());
    }
    if !ii.hbmMask.0.is_null() {
        let _ = DeleteObject(ii.hbmMask.into());
    }
    result
}

unsafe fn extract_shell(target: &str) -> Option<Vec<u8>> {
    let name = parsing_name(target)?;
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let factory: IShellItemImageFactory =
        SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;

    let size = SIZE { cx: ICON_SIZE, cy: ICON_SIZE };
    let hbitmap = factory
        .GetImage(size, SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0))
        .or_else(|_| factory.GetImage(size, SIIGBF_BIGGERSIZEOK))
        .ok()?;
    if hbitmap.0.is_null() {
        return None;
    }
    let png = hbitmap_to_png(hbitmap);
    let _ = DeleteObject(hbitmap.into());
    png
}

unsafe fn has_embedded_icons(exe_path: &str) -> bool {
    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
    ExtractIconExW(PCWSTR(wide.as_ptr()), -1, None, None, 0) > 0
}

unsafe fn extract_hwnd_icon(hwnd_val: i64) -> Option<Vec<u8>> {
    let hwnd = HWND(hwnd_val as isize as *mut c_void);
    if !IsWindow(Some(hwnd)).as_bool() {
        return None;
    }
    for kind in [ICON_BIG, ICON_SMALL2, ICON_SMALL] {
        let mut res = 0usize;
        let ok = SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            WPARAM(kind as usize),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            80,
            Some(&mut res),
        );
        if ok.0 != 0 && res != 0 {
            if let Some(png) = hicon_to_png(HICON(res as *mut c_void)) {
                return Some(png);
            }
        }
    }
    for idx in [GCLP_HICON, GCLP_HICONSM] {
        let h = GetClassLongPtrW(hwnd, idx);
        if h != 0 {
            if let Some(png) = hicon_to_png(HICON(h as *mut c_void)) {
                return Some(png);
            }
        }
    }
    None
}

/// Include PID + process creation timestamp so reused HWNDs never collide with stale disk icons.
unsafe fn hwnd_cache_key(hwnd_val: i64) -> String {
    let hwnd = HWND(hwnd_val as isize as *mut c_void);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let mut started = 0u64;
    if pid != 0 {
        if let Ok(proc_h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut c = FILETIME::default();
            let mut e = FILETIME::default();
            let mut k = FILETIME::default();
            let mut u = FILETIME::default();
            if GetProcessTimes(proc_h, &mut c, &mut e, &mut k, &mut u).is_ok() {
                started = ((c.dwHighDateTime as u64) << 32) | (c.dwLowDateTime as u64);
            }
            let _ = CloseHandle(proc_h);
        }
    }
    format!("hwnd:{hwnd_val}:{pid}:{started}")
}

/// One thread for all extraction: extracting icons can be slow and requires COM.
fn worker(cache_dir: PathBuf, rx: mpsc::Receiver<(String, Reply)>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let _ = fs::create_dir_all(&cache_dir);
    let mut failed: HashSet<String> = HashSet::new();

    let try_cached = |key: &str, extract_fn: &dyn Fn() -> Option<Vec<u8>>| -> Option<PathBuf> {
        let file = cache_dir.join(format!("{:016x}.png", hash_key(key)));
        if file.exists() {
            return Some(file);
        }
        if let Some(png) = extract_fn() {
            if fs::write(&file, png).is_ok() {
                return Some(file);
            }
        }
        None
    };

    while let Ok((target, reply)) = rx.recv() {
        let mut aumid: Option<&str> = None;
        let mut path: Option<&str> = None;
        let mut hwnd: Option<i64> = None;
        for part in target.lines() {
            let part = part.trim();
            if part.starts_with("aumid:") && aumid.is_none() {
                aumid = Some(part);
            } else if part.starts_with("path:") && path.is_none() {
                path = Some(part);
            } else if let Some(h) = part.strip_prefix("hwnd:") {
                if hwnd.is_none() {
                    hwnd = h.trim().parse::<i64>().ok();
                }
            }
        }

        let result = if failed.contains(&target) {
            None
        } else {
            let mut found: Option<PathBuf> = None;
            if let Some(a) = aumid {
                found = try_cached(a, &|| unsafe { extract_shell(a) });
            }
            if found.is_none() {
                if let Some(p_target) = path {
                    let raw_path = p_target.strip_prefix("path:").unwrap_or("");
                    if unsafe { has_embedded_icons(raw_path) } {
                        found = try_cached(p_target, &|| unsafe { extract_shell(p_target) });
                    }
                }
            }
            if found.is_none() {
                if let Some(h) = hwnd {
                    let hkey = unsafe { hwnd_cache_key(h) };
                    found = try_cached(&hkey, &|| unsafe { extract_hwnd_icon(h) });
                }
            }
            // Final fallback: let Shell try path even without embedded icons (e.g. shortcuts/associations).
            if found.is_none() {
                if let Some(p_target) = path {
                    found = try_cached(p_target, &|| unsafe { extract_shell(p_target) });
                }
            }
            if found.is_none() {
                failed.insert(target);
            }
            found
        };
        let _ = reply.send(result);
    }
}

fn sender(app: &AppHandle) -> Option<mpsc::Sender<(String, Reply)>> {
    let cell = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let dir = app
            .path()
            .app_cache_dir()
            .map(|d| d.join("icons"))
            .unwrap_or_else(|_| std::env::temp_dir().join("edgeshell-icons"));
        thread::spawn(move || worker(dir, rx));
        Mutex::new(tx)
    });
    cell.lock().ok().map(|t| t.clone())
}

/// target: "aumid:<AppID>" or "path:<.exe path>".
/// Returns the cached PNG file path (loaded by the UI through the asset protocol), or None.
#[tauri::command]
pub async fn app_icon(target: String, app: AppHandle) -> Result<Option<String>, String> {
    let Some(tx) = sender(&app) else { return Ok(None) };
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send((target, reply_tx)).map_err(|e| e.to_string())?;

    let path = tauri::async_runtime::spawn_blocking(move || reply_rx.recv().ok().flatten())
        .await
        .map_err(|e| e.to_string())?;

    if let Some(p) = &path {
        let _ = app.asset_protocol_scope().allow_file(p);
    }
    Ok(path.map(|p| p.to_string_lossy().into_owned()))
}
