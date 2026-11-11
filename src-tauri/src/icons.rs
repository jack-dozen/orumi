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
        Foundation::SIZE,
        Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, RGBQUAD,
        },
        System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
        UI::Shell::{
            IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
            SIIGBF_ICONONLY,
        },
    },
};

/// Requested icon size (px). 128 is sharp enough for icons up to ~64 CSS px on high-DPI displays.
const ICON_SIZE: i32 = 128;
/// Increment to invalidate the old cache (e.g. if the color conversion method changes).
const ICON_VERSION: u32 = 2;

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

/// 32-bit bitmap from Shell (premultiplied alpha, BGRA order) -> PNG (regular alpha, RGBA).
unsafe fn hbitmap_to_png(hbitmap: HBITMAP) -> Option<Vec<u8>> {
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

unsafe fn extract(target: &str) -> Option<Vec<u8>> {
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

/// One thread for all extraction: extracting icons can be slow and requires COM.
fn worker(cache_dir: PathBuf, rx: mpsc::Receiver<(String, Reply)>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let _ = fs::create_dir_all(&cache_dir);
    let mut failed: HashSet<String> = HashSet::new();

    while let Ok((target, reply)) = rx.recv() {
        let file = cache_dir.join(format!("{:016x}.png", hash_key(&target)));
        let result = if file.exists() {
            Some(file)
        } else if failed.contains(&target) {
            None
        } else {
            let ok = unsafe { extract(&target) }
                .map(|png| fs::write(&file, png).is_ok())
                .unwrap_or(false);
            if ok {
                Some(file)
            } else {
                failed.insert(target);
                None
            }
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
