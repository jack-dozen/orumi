//! App window list and change monitoring.
//! The filtering rules and SetWinEventHook pattern are adapted from Bloom (GPL-3.0): services.rs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    mem::size_of,
    path::Path,
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            Accessibility::{SetWinEventHook, HWINEVENTHOOK},
            WindowsAndMessaging::{
                DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetMessageW,
                GetSystemMetrics, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
                GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic,
                IsWindowVisible, IsZoomed, SetForegroundWindow, ShowWindow, SwitchToThisWindow,
                TranslateMessage, GWL_EXSTYLE, GW_OWNER, MSG, SM_CXSCREEN, SM_CYSCREEN,
                SW_MINIMIZE, SW_RESTORE, WINEVENT_OUTOFCONTEXT, WS_EX_APPWINDOW,
                WS_EX_TOOLWINDOW,
            },
        },
    },
};

use windows::{
    core::GUID,
    Win32::{
        Foundation::PROPERTYKEY,
        System::Com::{
            CoInitializeEx, CoTaskMemFree, CoUninitialize,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
            COINIT_MULTITHREADED,
        },
        UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
    },
};

#[derive(Serialize)]
pub struct WindowEntry {
    pub hwnd: i64,
    pub title: String,
    /// Program name without .exe. Empty for Store apps (wrapped by ApplicationFrameHost).
    pub exe: String,
    pub minimized: bool,
    /// This window is in the foreground.
    pub focused: bool,
    /// Full program path (empty for Store apps). Used to fetch the icon.
    pub path: String,
    /// AppUserModelID, only for Store apps. Used to fetch the icon.
    pub aumid: String,
}

/// Full program path (e.g. C:\...\chrome.exe), or empty if it cannot be read.
unsafe fn process_path(pid: u32) -> String {
    let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return String::new();
    };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_WIN32,
        PWSTR(buf.as_mut_ptr()),
        &mut len,
    )
    .is_ok();
    let _ = CloseHandle(handle);
    if !ok {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Window AppUserModelID (the identity used by the taskbar). For Store apps, this is the only way
/// to get the icon because the window is owned by ApplicationFrameHost.
unsafe fn window_aumid_inner(hwnd: HWND) -> Option<String> {
    // System.AppUserModel.ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
    let key = PROPERTYKEY {
        fmtid: GUID {
            data1: 0x9F4C2855,
            data2: 0x9F79,
            data3: 0x4B39,
            data4: [0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3],
        },
        pid: 5,
    };
    let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
    let mut prop = store.GetValue(&key).ok()?;
    let text = PropVariantToStringAlloc(&prop).ok().map(|pwstr| {
        let value = String::from_utf16_lossy(pwstr.as_wide()).trim().to_string();
        CoTaskMemFree(Some(pwstr.0 as _));
        value
    });
    let _ = PropVariantClear(&mut prop);
    text.filter(|s| !s.is_empty())
}

unsafe fn window_aumid(hwnd: HWND) -> Option<String> {
    let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
    let result = window_aumid_inner(hwnd);
    if com {
        CoUninitialize();
    }
    result
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Other visible windows own it (e.g. "Save As" or "Find" dialogs).
unsafe fn has_visible_owner(hwnd: HWND) -> bool {
    GetWindow(hwnd, GW_OWNER)
        .map(|o| !o.0.is_null() && IsWindowVisible(o).as_bool())
        .unwrap_or(false)
}

#[derive(Clone)]
struct WinMeta {
    pid: u32,
    class: String,
    path: String,
    exe: String,
    lower: String,
    aumid: String,
}

#[derive(Default)]
struct MetaCache {
    /// pid -> (full path, exe stem, lowercase filename)
    procs: HashMap<u32, (String, String, String)>,
    /// hwnd -> static window metadata
    wins: HashMap<isize, WinMeta>,
}

static META_CACHE: OnceLock<Mutex<MetaCache>> = OnceLock::new();
/// Last foreground HWND belonging to another process (so focus can be restored when our panels close).
static LAST_EXTERNAL_FG: AtomicIsize = AtomicIsize::new(0);

fn meta_cache() -> &'static Mutex<MetaCache> {
    META_CACHE.get_or_init(|| Mutex::new(MetaCache::default()))
}

/// Return an entry if this window belongs in the "alt-tab" list.
unsafe fn inspect(
    hwnd: HWND,
    my_pid: u32,
    foreground: HWND,
    cache: &mut MetaCache,
    seen_hwnds: &mut HashSet<isize>,
    seen_pids: &mut HashSet<u32>,
) -> Option<WindowEntry> {
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }

    // Fast in-process checks first before DWM / cross-process queries.
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return None;
    }
    // A dialog owned by another window is not counted separately unless it requests a taskbar button.
    if ex_style & WS_EX_APPWINDOW.0 == 0 && has_visible_owner(hwnd) {
        return None;
    }

    // Our own windows (bar, notch, launcher, overlay, switcher, etc.).
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 || pid == my_pid {
        return None;
    }

    // Cloaked windows: other virtual desktops and suspended Store apps.
    // IsWindowVisible remains true for them, so a DWM check is required.
    let mut cloaked: u32 = 0;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut c_void,
        size_of::<u32>() as u32,
    );
    if cloaked != 0 {
        return None;
    }

    // Skip hung windows before GetWindowTextW (which sends WM_GETTEXT and can block).
    if IsHungAppWindow(hwnd).as_bool() {
        return None;
    }

    let key = hwnd.0 as isize;
    seen_hwnds.insert(key);
    seen_pids.insert(pid);

    let meta = match cache.wins.get(&key) {
        Some(m) if m.pid == pid => m.clone(),
        _ => {
            let class = class_name(hwnd);
            let (path, exe, lower) = cache
                .procs
                .entry(pid)
                .or_insert_with(|| {
                    let p = process_path(pid);
                    let file = Path::new(&p)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let low = file.to_lowercase();
                    let stem = if low == "applicationframehost.exe" {
                        String::new()
                    } else {
                        Path::new(&file)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    };
                    (p, stem, low)
                })
                .clone();
            let is_store_frame =
                lower == "applicationframehost.exe" && class == "ApplicationFrameWindow";
            let aumid = if is_store_frame {
                window_aumid(hwnd).unwrap_or_default()
            } else {
                String::new()
            };
            let m = WinMeta {
                pid,
                class,
                path: if is_store_frame { String::new() } else { path },
                exe,
                lower,
                aumid,
            };
            cache.wins.insert(key, m.clone());
            m
        }
    };

    // Core Store app window; users interact with its ApplicationFrameWindow.
    if meta.class == "Windows.UI.Core.CoreWindow" {
        return None;
    }

    let is_explorer_window = meta.class == "CabinetWClass" || meta.class == "ExploreWClass";
    if meta.lower == "conhost.exe"
        || meta.lower == "shellexperiencehost.exe"
        || meta.lower == "startmenuexperiencehost.exe"
        || meta.lower == "searchhost.exe"
        || meta.lower == "textinputhost.exe"
        || (meta.lower == "explorer.exe" && !is_explorer_window)
        || (meta.lower == "applicationframehost.exe" && meta.class != "ApplicationFrameWindow")
    {
        return None;
    }

    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n <= 0 {
        return None;
    }
    let title = String::from_utf16_lossy(&buf[..n as usize]);
    if title == "Program Manager" {
        return None;
    }

    Some(WindowEntry {
        hwnd: key as i64,
        title,
        minimized: IsIconic(hwnd).as_bool(),
        focused: hwnd == foreground,
        aumid: meta.aumid,
        path: meta.path,
        exe: meta.exe,
    })
}

struct Ctx<'a> {
    list: Vec<WindowEntry>,
    my_pid: u32,
    foreground: HWND,
    cache: &'a mut MetaCache,
    seen_hwnds: HashSet<isize>,
    seen_pids: HashSet<u32>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut Ctx<'_>);
    if let Some(entry) = inspect(
        hwnd,
        ctx.my_pid,
        ctx.foreground,
        ctx.cache,
        &mut ctx.seen_hwnds,
        &mut ctx.seen_pids,
    ) {
        ctx.list.push(entry);
    }
    true.into()
}

/// Open windows, ordered from front to back. Synchronous; also used by Alt+Tab.
pub fn collect_windows() -> Vec<WindowEntry> {
    let mut fallback = MetaCache::default();
    let mut guard = meta_cache().lock().ok();
    let cache = guard.as_deref_mut().unwrap_or(&mut fallback);

    let my_pid = std::process::id();
    let foreground = unsafe { GetForegroundWindow() };
    let mut fg_pid: u32 = 0;
    if !foreground.0.is_null() {
        unsafe {
            GetWindowThreadProcessId(foreground, Some(&mut fg_pid));
        }
    }

    let mut ctx = Ctx {
        list: Vec::new(),
        my_pid,
        foreground,
        cache,
        seen_hwnds: HashSet::new(),
        seen_pids: HashSet::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx<'_> as isize));
    }
    let Ctx { mut list, cache, seen_hwnds, seen_pids, .. } = ctx;
    cache.wins.retain(|k, _| seen_hwnds.contains(k));
    cache.procs.retain(|k, _| seen_pids.contains(k));

    if let Some(first_focused) = list.iter().find(|w| w.focused) {
        LAST_EXTERNAL_FG.store(first_focused.hwnd as isize, Ordering::Relaxed);
    } else if fg_pid == my_pid && !list.is_empty() {
        // An ORUMI panel (e.g. launcher or notch) currently holds foreground focus: treat the
        // previously active external window (or topmost non-minimized window) as focused so
        // Alt+Tab switches away from it to the next window.
        let last_ext = LAST_EXTERNAL_FG.load(Ordering::Relaxed) as i64;
        if let Some(w) = list.iter_mut().find(|w| w.hwnd == last_ext && !w.minimized) {
            w.focused = true;
        } else if let Some(w) = list.iter_mut().find(|w| !w.minimized) {
            w.focused = true;
        }
    }
    list
}

/// If an ORUMI window currently holds foreground focus, return focus to the last active external window.
pub fn restore_external_foreground_if_ours() {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid != std::process::id() {
            return;
        }
        let target = LAST_EXTERNAL_FG.load(Ordering::Relaxed);
        if target != 0 {
            let h = HWND(target as *mut c_void);
            if IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() {
                SwitchToThisWindow(h, true);
                return;
            }
        }
        if let Some(first) = collect_windows().into_iter().find(|w| !w.minimized) {
            SwitchToThisWindow(to_hwnd(first.hwnd), true);
        }
    }
}

/// True if the active external foreground window is maximized or covers the full screen.
pub fn is_foreground_maximized() -> bool {
    unsafe {
        let my_pid = std::process::id();
        let mut hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        if !hwnd.0.is_null() {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if hwnd.0.is_null() || pid == 0 || pid == my_pid {
            let last = LAST_EXTERNAL_FG.load(Ordering::Relaxed);
            if last == 0 {
                return false;
            }
            hwnd = HWND(last as *mut c_void);
        }
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let class = class_name(hwnd);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        if IsZoomed(hwnd).as_bool() {
            return true;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let sw = GetSystemMetrics(SM_CXSCREEN);
            let sh = GetSystemMetrics(SM_CYSCREEN);
            if sw > 0
                && sh > 0
                && rect.left <= 0
                && rect.top <= 0
                && rect.right >= sw
                && rect.bottom >= sh
            {
                return true;
            }
        }
        false
    }
}

#[tauri::command]
pub async fn list_windows() -> Result<Vec<WindowEntry>, String> {
    tauri::async_runtime::spawn_blocking(collect_windows)
        .await
        .map_err(|e| e.to_string())
}

/// Switch to a window like native Alt+Tab: restore it if minimized, then bring it to the foreground.
/// Windows itself uses SwitchToThisWindow for Alt+Tab, and it is not blocked by focus rules
/// (SetForegroundWindow is rejected if our process is not active).
pub fn activate_window(hwnd: i64) {
    unsafe {
        let h = to_hwnd(hwnd);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        SwitchToThisWindow(h, true);
    }
}

fn to_hwnd(hwnd: i64) -> HWND {
    HWND(hwnd as isize as *mut c_void)
}

#[tauri::command]
pub fn focus_window(hwnd: i64) -> Result<(), String> {
    unsafe {
        let h = to_hwnd(hwnd);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if !SetForegroundWindow(h).as_bool() {
            return Err("Windows menolak memindahkan fokus".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn minimize_window(hwnd: i64) {
    unsafe {
        let _ = ShowWindow(to_hwnd(hwnd), SW_MINIMIZE);
    }
}

// ---------- Change monitoring ----------

static EVENT_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// The callback must be as fast as possible: only notify the worker thread.
unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Child controls (buttons, sliders) also report events; we only want the parent window.
    if hwnd.0.is_null() || id_object != 0 || id_child != 0 {
        return;
    }
    if event == 0x0003 {
        // EVENT_SYSTEM_FOREGROUND: track external foreground windows and keep the Alt+Tab hook fresh.
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 && pid != std::process::id() {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex_style & WS_EX_TOOLWINDOW.0 == 0 && IsWindowVisible(hwnd).as_bool() {
                LAST_EXTERNAL_FG.store(hwnd.0 as isize, Ordering::Relaxed);
            }
        }
        crate::altswitch::ensure_hook("foreground event");
    }
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(());
    }
}

/// Emit a "windows-changed" event when the window list may have changed.
pub fn start_events(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<()>();
    let _ = EVENT_TX.set(tx);

    // Coalesce a stream of events into one notification.
    thread::spawn(move || {
        while rx.recv().is_ok() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(400)
                && rx.recv_timeout(Duration::from_millis(80)).is_ok()
            {}
            let _ = app.emit("windows-changed", ());
        }
    });

    // The WinEvent hook requires a thread with a message loop.
    thread::spawn(|| unsafe {
        const EVENTS: [u32; 8] = [
            0x0003, // EVENT_SYSTEM_FOREGROUND
            0x0016, // EVENT_SYSTEM_MINIMIZESTART
            0x0017, // EVENT_SYSTEM_MINIMIZEEND
            0x8001, // EVENT_OBJECT_DESTROY
            0x8002, // EVENT_OBJECT_SHOW
            0x8003, // EVENT_OBJECT_HIDE
            0x8017, // EVENT_OBJECT_CLOAKED
            0x8018, // EVENT_OBJECT_UNCLOAKED
        ];
        for e in EVENTS {
            let _ = SetWinEventHook(e, e, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
