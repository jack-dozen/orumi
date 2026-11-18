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
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            Accessibility::{SetWinEventHook, HWINEVENTHOOK},
            Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN},
            WindowsAndMessaging::{
                AllowSetForegroundWindow, DispatchMessageW, EnumWindows, GetAncestor,
                GetClassNameW, GetForegroundWindow, GetMessageW, GetShellWindow, GetSystemMetrics,
                GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
                GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible, IsZoomed,
                SetForegroundWindow, ShowWindow, SwitchToThisWindow, TranslateMessage, ASFW_ANY,
                GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, MSG, SM_CXSCREEN, SM_CYSCREEN,
                SW_MINIMIZE, SW_RESTORE, WINEVENT_OUTOFCONTEXT, WS_CAPTION, WS_EX_APPWINDOW,
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

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetApplicationUserModelId(
        hprocess: windows::Win32::Foundation::HANDLE,
        applicationusermodelidlength: *mut u32,
        applicationusermodelid: PWSTR,
    ) -> i32;
}

/// Full program path (e.g. C:\...\chrome.exe) and packaged process AUMID (if MSIX/Store app).
unsafe fn process_info(pid: u32) -> (String, String) {
    let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return (String::new(), String::new());
    };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let path = if QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_WIN32,
        PWSTR(buf.as_mut_ptr()),
        &mut len,
    )
    .is_ok()
    {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    };

    let mut aumid_buf = [0u16; 260];
    let mut aumid_len = aumid_buf.len() as u32;
    let proc_aumid = if GetApplicationUserModelId(
        handle,
        &mut aumid_len,
        PWSTR(aumid_buf.as_mut_ptr()),
    ) == 0
        && aumid_len > 1
    {
        String::from_utf16_lossy(&aumid_buf[..(aumid_len as usize).saturating_sub(1)])
    } else {
        String::new()
    };

    let _ = CloseHandle(handle);
    (path, proc_aumid)
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
    /// pid -> (full path, exe stem, lowercase filename, process AUMID)
    procs: HashMap<u32, (String, String, String, String)>,
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
            let (path, exe, lower, proc_aumid) = cache
                .procs
                .entry(pid)
                .or_insert_with(|| {
                    let (p, proc_aumid) = process_info(pid);
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
                    (p, stem, low, proc_aumid)
                })
                .clone();
            let is_store_frame =
                lower == "applicationframehost.exe" && class == "ApplicationFrameWindow";
            let aumid = window_aumid(hwnd)
                .filter(|s| !s.is_empty())
                .unwrap_or(proc_aumid);
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

/// Open windows in the active workspace, ordered from front to back.
pub fn collect_windows() -> Vec<WindowEntry> {
    collect_windows_inner(false)
}

/// Open windows across all workspaces (used by Activities snapshots).
pub fn collect_all_windows() -> Vec<WindowEntry> {
    collect_windows_inner(true)
}

fn collect_windows_inner(include_other_workspaces: bool) -> Vec<WindowEntry> {
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

    if !include_other_workspaces {
        list.retain(|w| !crate::wm_ipc::is_in_other_workspace(w.hwnd));
    }

    if let Some(first_focused) = list
        .iter()
        .find(|w| w.focused && !crate::wm_ipc::is_in_other_workspace(w.hwnd))
    {
        LAST_EXTERNAL_FG.store(first_focused.hwnd as isize, Ordering::Relaxed);
    } else if fg_pid == my_pid && !list.is_empty() {
        // An ORUMI panel (e.g. launcher or notch) currently holds foreground focus: treat the
        // previously active external window in the active workspace (or topmost non-minimized
        // window) as focused so Alt+Tab switches away from it to the next window.
        let last_ext = LAST_EXTERNAL_FG.load(Ordering::Relaxed) as i64;
        if let Some(w) = list.iter_mut().find(|w| {
            w.hwnd == last_ext && !w.minimized && !crate::wm_ipc::is_in_other_workspace(w.hwnd)
        }) {
            w.focused = true;
        } else if let Some(w) = list
            .iter_mut()
            .find(|w| !w.minimized && !crate::wm_ipc::is_in_other_workspace(w.hwnd))
        {
            w.focused = true;
        }
    }
    list
}

/// If an ORUMI window currently holds foreground focus, return focus to the last active external
/// window in the active workspace (or the desktop shell if the active workspace is empty).
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
        if target != 0 && !crate::wm_ipc::is_in_other_workspace(target as i64) {
            let h = HWND(target as *mut c_void);
            if IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() {
                SwitchToThisWindow(h, true);
                let fg = GetForegroundWindow();
                if fg != h
                    && GetAncestor(fg, GA_ROOT) != h
                    && crate::wm_ipc::is_elevated()
                {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                    let _ = crate::wm_ipc::focus_window(target as i64);
                }
                return;
            }
        }
        if let Some(first) = collect_windows().into_iter().find(|w| !w.minimized) {
            let h = to_hwnd(first.hwnd);
            SwitchToThisWindow(h, true);
            let fg = GetForegroundWindow();
            if fg != h
                && GetAncestor(fg, GA_ROOT) != h
                && crate::wm_ipc::is_elevated()
            {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = crate::wm_ipc::focus_window(first.hwnd);
            }
            return;
        }
        // Active workspace has no visible non-minimized windows: yield focus to the desktop
        // instead of pulling focus back to a parked window in another workspace.
        let shell = GetShellWindow();
        if !shell.0.is_null() {
            SwitchToThisWindow(shell, true);
        }
    }
}

unsafe fn is_on_primary_screen(hwnd: HWND) -> bool {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return false;
    }
    if rect.right - rect.left < 80 || rect.bottom - rect.top < 80 {
        return false;
    }
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    if sw <= 0 || sh <= 0 {
        return true;
    }
    rect.right > 0 && rect.left < sw && rect.bottom > 0 && rect.top < sh
}

/// True if the active workspace currently has any non-minimized window visible on the primary
/// screen (maximized, WM-tiled with gaps, or split). Returns false only when the desktop is empty.
pub fn has_visible_window() -> bool {
    collect_windows()
        .into_iter()
        .any(|w| !w.minimized && unsafe { is_on_primary_screen(to_hwnd(w.hwnd)) })
}

/// True if the active workspace has a non-minimized window overlapping the top-center notch
/// proximity zone on the primary monitor.
pub fn has_window_near_notch(mx: f64, my: f64, mw: f64, u: f64) -> bool {
    let half_w = 260.0 * u;
    let near_h = 96.0 * u;
    let zx0 = mx + mw / 2.0 - half_w;
    let zx1 = mx + mw / 2.0 + half_w;
    let zy0 = my;
    let zy1 = my + near_h;

    collect_windows().into_iter().any(|w| {
        if w.minimized {
            return false;
        }
        let hwnd = to_hwnd(w.hwnd);
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return false;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            if rect.right - rect.left < 80 || rect.bottom - rect.top < 80 {
                return false;
            }
            (rect.right as f64) > zx0
                && (rect.left as f64) < zx1
                && (rect.bottom as f64) > zy0
                && (rect.top as f64) < zy1
        }
    })
}

/// True when the foreground application on the primary monitor is in true fullscreen mode
/// (e.g. F11 browser/video or borderless/exclusive game), NOT merely maximized.
pub fn is_foreground_fullscreen() -> bool {
    unsafe {
        if let Ok(state) = SHQueryUserNotificationState() {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN {
                return true;
            }
        }

        let my_pid = std::process::id();
        let mut hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        if !hwnd.0.is_null() {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if hwnd.0.is_null() || pid == 0 || pid == my_pid {
            let last = LAST_EXTERNAL_FG.load(Ordering::Relaxed);
            if last == 0 || crate::wm_ipc::is_in_other_workspace(last as i64) {
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

        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return false;
        }
        // Only lock when fullscreen occupies the primary monitor where ORUMI panels live.
        if (mi.dwFlags & 1) == 0 {
            return false;
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let mr = mi.rcMonitor;
        let covers_monitor =
            rect.left <= mr.left && rect.top <= mr.top && rect.right >= mr.right && rect.bottom >= mr.bottom;
        if !covers_monitor {
            return false;
        }

        let zoomed = IsZoomed(hwnd).as_bool();
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let has_caption = (style & WS_CAPTION.0) == WS_CAPTION.0;
        // Normal maximized windows have IsZoomed == true and WS_CAPTION; F11 / borderless
        // fullscreen clears IsZoomed or strips the window caption.
        !zoomed || !has_caption
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
    crate::wm_ipc::ensure_window_active(hwnd);
    unsafe {
        let h = to_hwnd(hwnd);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        SwitchToThisWindow(h, true);
        let fg = GetForegroundWindow();
        if fg != h && GetAncestor(fg, GA_ROOT) != h && crate::wm_ipc::is_elevated() {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = crate::wm_ipc::focus_window(hwnd);
        }
    }
}

fn to_hwnd(hwnd: i64) -> HWND {
    HWND(hwnd as isize as *mut c_void)
}

#[tauri::command]
pub fn focus_window(hwnd: i64) -> Result<(), String> {
    crate::wm_ipc::ensure_window_active(hwnd);
    unsafe {
        let h = to_hwnd(hwnd);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if !SetForegroundWindow(h).as_bool() {
            SwitchToThisWindow(h, true);
        }
        let fg = GetForegroundWindow();
        if fg != h && GetAncestor(fg, GA_ROOT) != h && crate::wm_ipc::is_elevated() {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = crate::wm_ipc::focus_window(hwnd);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn minimize_window(hwnd: i64) {
    if crate::wm_ipc::is_elevated() && crate::wm_ipc::minimize_window(hwnd).is_ok() {
        return;
    }
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
