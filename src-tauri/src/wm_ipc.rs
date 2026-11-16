//! Named Pipe IPC client and lifecycle manager for the standalone `vscroll-wm` process.
//!
//! Keeping `vscroll-wm` in its own process isolates its `WH_KEYBOARD_LL` hook, `SetWinEventHook`,
//! and Win32 message loop from ORUMI's `altswitch` and `winkey` hooks, while mirroring workspace
//! state into `WM_SNAPSHOT` over `\\.\pipe\vscroll-wm-events` for zero-latency reads.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, RwLock};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

const CMD_PIPE: &str = r"\\.\pipe\vscroll-wm";
const EVENT_PIPE: &str = r"\\.\pipe\vscroll-wm-events";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ERROR_PIPE_BUSY: i32 = 231;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static SPAWNED_BY_ORUMI: AtomicBool = AtomicBool::new(false);

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub index: usize,
    pub active: bool,
    pub windows: Vec<i64>,
}

#[derive(Deserialize)]
struct RawSnapshot {
    paused: bool,
    active: usize,
    workspaces: Vec<WorkspaceInfo>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct WmSnapshot {
    connected: bool,
    paused: bool,
    active_ws: usize,
    inactive_windows: HashSet<i64>,
    workspaces: Vec<WorkspaceInfo>,
}

static WM_SNAPSHOT: LazyLock<RwLock<WmSnapshot>> = LazyLock::new(|| RwLock::new(WmSnapshot::default()));

fn build_snapshot(raw: RawSnapshot) -> WmSnapshot {
    let mut inactive_windows = HashSet::new();
    for ws in &raw.workspaces {
        if ws.index != raw.active {
            for &hwnd in &ws.windows {
                inactive_windows.insert(hwnd);
            }
        }
    }
    WmSnapshot {
        connected: true,
        paused: raw.paused,
        active_ws: raw.active,
        inactive_windows,
        workspaces: raw.workspaces,
    }
}

fn apply_snapshot(app: Option<&AppHandle>, next: WmSnapshot) {
    let changed = {
        let Ok(mut g) = WM_SNAPSHOT.write() else { return };
        if *g == next {
            false
        } else {
            *g = next;
            true
        }
    };
    if changed {
        if let Some(app) = app {
            let _ = app.emit("windows-changed", ());
            let _ = app.emit("workspaces-changed", ());
        }
    }
}

fn send_cmd(cmd: &str) -> Result<String, String> {
    let mut file = None;
    for _ in 0..8 {
        match OpenOptions::new().read(true).write(true).open(CMD_PIPE) {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("vscroll-wm IPC unavailable: {e}")),
        }
    }
    let mut file = file.ok_or_else(|| "vscroll-wm IPC pipe busy".to_string())?;
    writeln!(file, "{cmd}").map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;

    let mut line = String::new();
    BufReader::new(file)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let trimmed = line.trim().to_string();
    if let Some(err) = trimmed.strip_prefix("err ") {
        Err(err.to_string())
    } else if trimmed.is_empty() {
        Err("empty IPC reply from vscroll-wm".into())
    } else {
        Ok(trimmed)
    }
}

fn refresh_snapshot_sync() {
    if let Ok(json) = send_cmd("list") {
        if let Ok(raw) = serde_json::from_str::<RawSnapshot>(&json) {
            apply_snapshot(None, build_snapshot(raw));
        }
    }
}

fn find_vscroll_exe() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("vscroll-wm.exe"));
            for anc in dir.ancestors().take(6) {
                candidates.push(anc.join(r"ref\vscroll-wm\target\release\vscroll-wm.exe"));
                candidates.push(anc.join(r"ref\vscroll-wm\target\debug\vscroll-wm.exe"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("vscroll-wm.exe"));
        candidates.push(cwd.join(r"ref\vscroll-wm\target\release\vscroll-wm.exe"));
        candidates.push(cwd.join(r"ref\vscroll-wm\target\debug\vscroll-wm.exe"));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join(r"ref\vscroll-wm\target\release\vscroll-wm.exe"));
            candidates.push(parent.join(r"ref\vscroll-wm\target\debug\vscroll-wm.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn maybe_spawn_vscroll(app: &AppHandle) {
    if send_cmd("list").is_ok() {
        return;
    }
    let Some(exe) = find_vscroll_exe() else {
        eprintln!("[wm-ipc] vscroll-wm.exe not found; waiting for manual launch");
        return;
    };
    let mut cmd = Command::new(&exe);
    cmd.arg("--no-tray").creation_flags(CREATE_NO_WINDOW);
    if let Ok(dir) = app.path().app_config_dir() {
        if std::env::var_os("VSCROLL_CONFIG").is_none() {
            let conf = dir.join("vscroll-wm.conf");
            if conf.is_file() {
                cmd.env("VSCROLL_CONFIG", conf);
            }
        }
        if std::env::var_os("VSCROLL_IGNORE").is_none() {
            let ign = dir.join("vscroll-wm.ignore");
            if ign.is_file() {
                cmd.env("VSCROLL_IGNORE", ign);
            }
        }
    }
    match cmd.spawn() {
        Ok(_) => {
            SPAWNED_BY_ORUMI.store(true, Ordering::Relaxed);
            eprintln!("[wm-ipc] launched {}", exe.display());
        }
        Err(e) => eprintln!("[wm-ipc] failed to launch {}: {e}", exe.display()),
    }
}

/// Start the event listener and auto-launch `vscroll-wm.exe` if it is not already running.
pub fn start(app: AppHandle) {
    SHUTDOWN.store(false, Ordering::Relaxed);
    thread::Builder::new()
        .name("vscroll-ipc-client".into())
        .spawn(move || {
            maybe_spawn_vscroll(&app);
            while !SHUTDOWN.load(Ordering::Relaxed) {
                match File::open(EVENT_PIPE) {
                    Ok(file) => {
                        let reader = BufReader::new(file);
                        for line in reader.lines() {
                            if SHUTDOWN.load(Ordering::Relaxed) {
                                return;
                            }
                            let Ok(line) = line else { break };
                            if let Ok(raw) = serde_json::from_str::<RawSnapshot>(line.trim()) {
                                apply_snapshot(Some(&app), build_snapshot(raw));
                            }
                        }
                        apply_snapshot(Some(&app), WmSnapshot::default());
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(400));
                    }
                }
            }
        })
        .ok();
}

/// Ask a spawned `vscroll-wm.exe` instance to restore windows and exit cleanly when ORUMI closes.
pub fn shutdown() {
    SHUTDOWN.store(true, Ordering::Relaxed);
    if SPAWNED_BY_ORUMI.load(Ordering::Relaxed) {
        let _ = send_cmd("quit");
    }
}

pub fn toggle_pause() {
    thread::spawn(|| {
        let _ = send_cmd("toggle_pause");
    });
}

pub fn reload_config() {
    thread::spawn(|| {
        let _ = send_cmd("reload");
    });
}

pub fn reapply() {
    thread::spawn(|| {
        let _ = send_cmd("reapply");
    });
}

pub fn show_cheatsheet() {
    thread::spawn(|| {
        let _ = send_cmd("cheatsheet");
    });
}

/// True if `hwnd` belongs to an inactive workspace while `vscroll-wm` is connected and active.
pub fn is_in_other_workspace(hwnd: i64) -> bool {
    let Ok(g) = WM_SNAPSHOT.read() else {
        return false;
    };
    g.connected && !g.paused && g.inactive_windows.contains(&hwnd)
}

pub fn list_workspaces() -> Vec<WorkspaceInfo> {
    let connected = WM_SNAPSHOT.read().is_ok_and(|g| g.connected);
    if !connected {
        refresh_snapshot_sync();
    }
    let Ok(g) = WM_SNAPSHOT.read() else {
        return vec![WorkspaceInfo { index: 0, active: true, windows: Vec::new() }];
    };
    if g.workspaces.is_empty() {
        vec![WorkspaceInfo { index: 0, active: true, windows: Vec::new() }]
    } else {
        g.workspaces.clone()
    }
}

pub fn switch_workspace(index: usize, focus: bool) -> Result<(), String> {
    let _ = send_cmd(&format!("switch {index} {}", if focus { 1 } else { 0 }))?;
    refresh_snapshot_sync();
    Ok(())
}

pub fn move_window_to_workspace(hwnd: i64, index: usize) -> Result<(), String> {
    let _ = send_cmd(&format!("move {hwnd} {index}"))?;
    refresh_snapshot_sync();
    Ok(())
}

/// If `hwnd` is parked in another workspace, switch to that workspace and unpark it before focusing.
pub fn ensure_window_active(hwnd: i64) {
    if !is_in_other_workspace(hwnd) {
        return;
    }
    let _ = send_cmd(&format!("ensure {hwnd}"));
    refresh_snapshot_sync();
}
