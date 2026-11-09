//! Tombol tray di bar: membuka flyout "ikon tersembunyi" bawaan Windows 11 lewat UI Automation.
//! Pola pencarian chevron dan penanganan state basi diadaptasi dari Bloom (GPL-3.0): tray.rs.
//! Ikon dan menu kanannya tetap ditangani Windows, jadi semua app tray berfungsi normal.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{thread, time::Duration};

use tauri::{AppHandle, Manager};
use windows::{
    core::{w, Interface, PCWSTR},
    Win32::{
        Foundation::{HWND, RECT},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
                IUIAutomationInvokePattern, TreeScope_Descendants, UIA_InvokePatternId,
            },
            WindowsAndMessaging::{
                FindWindowExW, FindWindowW, GetWindowRect, IsWindowVisible, SetWindowPos,
                SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            },
        },
    },
};

/// false = flyout dibiarkan di posisi bawaan Windows (dekat taskbar).
const MOVE_FLYOUT: bool = true;

struct ComGuard(bool);

impl ComGuard {
    unsafe fn new() -> Self {
        Self(CoInitializeEx(None, COINIT_MULTITHREADED).is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Window flyout ikon tersembunyi: nama kelasnya berbeda antar build Windows.
fn overflow_visible() -> Option<HWND> {
    unsafe {
        let hwnd = FindWindowW(w!("TopLevelWindowForOverflowXamlIsland"), PCWSTR::null())
            .or_else(|_| FindWindowW(w!("NotifyIconOverflowWindow"), PCWSTR::null()))
            .ok()?;
        IsWindowVisible(hwnd).as_bool().then_some(hwnd)
    }
}

/// Cari tombol chevron di taskbar. Namanya dilokalkan ("Show Hidden Icons", dst.), jadi
/// dikenali dari AutomationId dan kelasnya.
unsafe fn find_chevron(automation: &IUIAutomation) -> Result<IUIAutomationElement, String> {
    let taskbar = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null())
        .map_err(|_| "Taskbar Windows tidak ditemukan".to_string())?;
    // Kontrol tray Windows 11 ada di jembatan XAML anak, bukan di Shell_TrayWnd langsung.
    let bridge = FindWindowExW(
        Some(taskbar),
        None,
        w!("Windows.UI.Composition.DesktopWindowContentBridge"),
        PCWSTR::null(),
    )
    .unwrap_or(taskbar);

    let root = automation.ElementFromHandle(bridge).map_err(|e| e.to_string())?;
    let condition = automation.CreateTrueCondition().map_err(|e| e.to_string())?;
    let elements = root
        .FindAll(TreeScope_Descendants, &condition)
        .map_err(|e| e.to_string())?;

    let mut fallback: Option<IUIAutomationElement> = None;
    let mut seen = Vec::new();
    for i in 0..elements.Length().unwrap_or(0) {
        let Ok(el) = elements.GetElement(i) else { continue };
        let id = el.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
        if id != "SystemTrayIcon" {
            continue;
        }
        let class = el.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
        let name = el.CurrentName().map(|s| s.to_string()).unwrap_or_default();
        if class == "SystemTray.NormalButton" {
            return Ok(el);
        }
        // Cadangan untuk build lama: nama bahasa Inggris tombolnya.
        if fallback.is_none() && name.to_lowercase().contains("hidden") {
            fallback = Some(el);
        }
        seen.push(format!("[{class}] \"{name}\""));
    }
    fallback.ok_or_else(|| {
        format!(
            "Tombol chevron tray tidak ditemukan. Kandidat di taskbar: {}",
            if seen.is_empty() { "tidak ada".to_string() } else { seen.join(", ") }
        )
    })
}

/// Pindahkan flyout ke samping bar (best-effort; gagal = dibiarkan di tempatnya).
unsafe fn place_near(hwnd: HWND, anchor: (i32, i32)) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return;
    }
    let height = r.bottom - r.top;
    let _ = SetWindowPos(
        hwnd,
        None,
        anchor.0,
        (anchor.1 - height).max(0),
        0,
        0,
        SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
    );
}

fn toggle_overflow(anchor: Option<(i32, i32)>) -> Result<(), String> {
    unsafe {
        let _com = ComGuard::new();
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        if let Ok(options) = automation.cast::<IUIAutomation2>() {
            let _ = options.SetConnectionTimeout(1000);
            let _ = options.SetTransactionTimeout(2000);
        }

        let was_open = overflow_visible().is_some();
        let chevron = find_chevron(&automation)?;
        let pattern = chevron
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .map_err(|e| e.to_string())?;
        pattern.Invoke().map_err(|e| e.to_string())?;
        if was_open {
            return Ok(()); // chevron bekerja sebagai sakelar: ini menutup flyout
        }

        // Explorer mengingat state "terbuka" walau flyout sudah hilang; kalau flyout tidak muncul,
        // sakelar sekali lagi lalu tunggu lagi.
        for attempt in 0..2 {
            for _ in 0..15 {
                if let Some(hwnd) = overflow_visible() {
                    if MOVE_FLYOUT {
                        if let Some(a) = anchor {
                            place_near(hwnd, a);
                        }
                    }
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(40));
            }
            if attempt == 0 {
                pattern.Invoke().map_err(|e| e.to_string())?;
            }
        }
        Err("Flyout ikon tersembunyi tidak muncul".into())
    }
}

/// Titik di kanan bar (x) dan dekat dasarnya (y), dalam pixel fisik.
fn bar_anchor(app: &AppHandle) -> Option<(i32, i32)> {
    let bar = app.get_webview_window("bar")?;
    let pos = bar.outer_position().ok()?;
    let size = bar.outer_size().ok()?;
    let scale = bar.scale_factor().ok()?;
    Some((
        pos.x + size.width as i32 + (8.0 * scale) as i32,
        pos.y + size.height as i32 - (16.0 * scale) as i32,
    ))
}

#[tauri::command]
pub async fn tray_toggle(app: AppHandle) -> Result<(), String> {
    let anchor = bar_anchor(&app);
    let result = tauri::async_runtime::spawn_blocking(move || toggle_overflow(anchor))
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) = &result {
        eprintln!("[tray] {e}");
    }
    result
}
