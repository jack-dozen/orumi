use std::{os::windows::process::CommandExt, process::Command, thread, time::Duration};

use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, HWND_BROADCAST, WM_SYSCOMMAND},
};

/// Supaya program yang dijalankan tidak memunculkan jendela konsol.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SC_MONITORPOWER: usize = 0xF170;

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_action(id: String) -> Result<(), String> {
    match id.as_str() {
        "lock" => run("rundll32.exe", &["user32.dll,LockWorkStation"]),
        // Lewat .NET supaya benar-benar sleep (bukan hibernate walau hibernate aktif).
        "sleep" => run(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; \
                 [System.Windows.Forms.Application]::SetSuspendState(\
                 [System.Windows.Forms.PowerState]::Suspend, $false, $false)",
            ],
        ),
        "hibernate" => run("shutdown.exe", &["/h"]),
        "shutdown" => run("shutdown.exe", &["/s", "/t", "0"]),
        "restart" => run("shutdown.exe", &["/r", "/t", "0"]),
        "logout" => run("shutdown.exe", &["/l"]),
        "display_off" => {
            // Jeda sebentar: gerakan mouse saat menekan Enter bisa langsung menyalakan layar lagi.
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(600));
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND_BROADCAST),
                        WM_SYSCOMMAND,
                        WPARAM(SC_MONITORPOWER),
                        LPARAM(2),
                    );
                }
            });
            Ok(())
        }
        _ => Err(format!("aksi tidak dikenal: {id}")),
    }
}
