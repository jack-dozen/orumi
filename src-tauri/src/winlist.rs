//! Daftar window aplikasi dan pemantauan perubahannya.
//! Aturan filter dan pola SetWinEventHook diadaptasi dari Bloom (GPL-3.0): services.rs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    mem::size_of,
    path::Path,
    sync::{mpsc, OnceLock},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            Accessibility::{SetWinEventHook, HWINEVENTHOOK},
            WindowsAndMessaging::{
                DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetMessageW,
                GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
                GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
                ShowWindow, TranslateMessage, GWL_EXSTYLE, GW_OWNER, MSG, SW_MINIMIZE,
                SW_RESTORE, WINEVENT_OUTOFCONTEXT, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
            },
        },
    },
};

#[derive(Serialize)]
pub struct WindowEntry {
    hwnd: i64,
    title: String,
    /// Nama program tanpa .exe. Kosong untuk app Store (dibungkus ApplicationFrameHost).
    exe: String,
    minimized: bool,
    /// Window ini sedang di depan (foreground).
    focused: bool,
}

/// Nama file program (misalnya "chrome.exe"), atau kosong kalau tidak bisa dibaca.
unsafe fn process_file_name(pid: u32) -> String {
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
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Window lain yang terlihat di layar memilikinya (dialog "Save As", "Find", dst).
unsafe fn has_visible_owner(hwnd: HWND) -> bool {
    GetWindow(hwnd, GW_OWNER)
        .map(|o| !o.0.is_null() && IsWindowVisible(o).as_bool())
        .unwrap_or(false)
}

/// Kembalikan entri kalau ini window yang layak tampil di daftar "alt-tab".
unsafe fn inspect(hwnd: HWND, my_pid: u32, foreground: HWND) -> Option<WindowEntry> {
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }

    // Window yang di-"cloak": desktop virtual lain dan app Store yang ditangguhkan.
    // IsWindowVisible tetap true untuk mereka, jadi cek DWM diperlukan.
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

    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return None;
    }
    // Dialog milik window lain tidak dihitung sendiri, kecuali ia meminta tombol taskbar.
    if ex_style & WS_EX_APPWINDOW.0 == 0 && has_visible_owner(hwnd) {
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

    // Window kita sendiri (bar, notch, launcher, overlay).
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == my_pid {
        return None;
    }

    let class = class_name(hwnd);
    // Jendela inti app Store; yang dipakai pengguna adalah ApplicationFrameWindow-nya.
    if class == "Windows.UI.Core.CoreWindow" {
        return None;
    }

    let file = process_file_name(pid);
    let lower = file.to_lowercase();
    let is_explorer_window = class == "CabinetWClass" || class == "ExploreWClass";

    if lower == "conhost.exe"
        || lower == "shellexperiencehost.exe"
        || lower == "startmenuexperiencehost.exe"
        || lower == "searchhost.exe"
        || lower == "textinputhost.exe"
        || (lower == "explorer.exe" && !is_explorer_window)
        || (lower == "applicationframehost.exe" && class != "ApplicationFrameWindow")
    {
        return None;
    }

    let exe = if lower == "applicationframehost.exe" {
        String::new()
    } else {
        Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    Some(WindowEntry {
        hwnd: hwnd.0 as isize as i64,
        title,
        exe,
        minimized: IsIconic(hwnd).as_bool(),
        focused: hwnd == foreground,
    })
}

struct Ctx {
    list: Vec<WindowEntry>,
    my_pid: u32,
    foreground: HWND,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut Ctx);
    if let Some(entry) = inspect(hwnd, ctx.my_pid, ctx.foreground) {
        ctx.list.push(entry);
    }
    true.into()
}

/// Daftar window terbuka, urut dari yang paling atas di layar.
#[tauri::command]
pub async fn list_windows() -> Result<Vec<WindowEntry>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut ctx = Ctx {
            list: Vec::new(),
            my_pid: std::process::id(),
            foreground: unsafe { GetForegroundWindow() },
        };
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        ctx.list
    })
    .await
    .map_err(|e| e.to_string())
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

// ---------- Pemantauan perubahan ----------

static EVENT_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// Callback harus secepat mungkin: hanya memberi tanda ke thread pekerja.
unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Kontrol anak (tombol, slider) juga melaporkan event; yang kita mau hanya window induk.
    if hwnd.0.is_null() || id_object != 0 || id_child != 0 {
        return;
    }
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(());
    }
}

/// Memancarkan event "windows-changed" saat daftar window mungkin berubah.
pub fn start_events(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<()>();
    let _ = EVENT_TX.set(tx);

    // Gabungkan rentetan event menjadi satu pemberitahuan.
    thread::spawn(move || {
        while rx.recv().is_ok() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(400)
                && rx.recv_timeout(Duration::from_millis(80)).is_ok()
            {}
            let _ = app.emit("windows-changed", ());
        }
    });

    // Hook WinEvent butuh thread dengan message loop.
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
