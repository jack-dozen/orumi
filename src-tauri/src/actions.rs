use std::{os::windows::process::CommandExt, process::Command, thread, time::Duration};

use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, HWND_BROADCAST, WM_SYSCOMMAND},
};

/// Prevent the launched program from opening a console window.
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
        // Use .NET to perform an actual sleep (not hibernation, even when hibernation is enabled).
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
            // Brief delay: mouse movement while pressing Enter can immediately turn the display back on.
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
