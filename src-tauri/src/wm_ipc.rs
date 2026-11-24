//! Named Pipe IPC client and lifecycle manager for the standalone `vscroll-wm` process.
//!
//! Keeping `vscroll-wm` in its own process isolates its `WH_KEYBOARD_LL` hook, `SetWinEventHook`,
//! and Win32 message loop from ORUMI's `altswitch` and `winkey` hooks, while mirroring workspace
//! state into `WM_SNAPSHOT` over `\\.\pipe\vscroll-wm-events` for zero-latency reads.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

const CMD_PIPE: &str = r"\\.\pipe\vscroll-wm";
const EVENT_PIPE: &str = r"\\.\pipe\vscroll-wm-events";
const ELEVATED_TASK_NAME: &str = "ORUMI-vscroll-wm";
const ELEVATED_DIR: &str = r"C:\Program Files\ORUMI";
const ELEVATED_EXE_PATH: &str = r"C:\Program Files\ORUMI\vscroll-wm.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ERROR_PIPE_BUSY: i32 = 231;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static SPAWNED_BY_ORUMI: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

#[derive(Clone, Serialize)]
struct WorkspaceOsdPayload {
    active: usize,
    total: usize,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub index: usize,
    pub active: bool,
    pub windows: Vec<i64>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CheatsheetRow {
    pub keys: String,
    pub desc: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CheatsheetGroup {
    pub title: String,
    pub rows: Vec<CheatsheetRow>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct OverviewItemData {
    pub hwnd: i64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub span: String,
    pub title: String,
    pub exe: String,
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct OverviewWorkspaceData {
    pub index: usize,
    pub active: bool,
    pub width: i32,
    pub height: i32,
    pub view_y: i32,
    pub view_h: i32,
    pub focused: i64,
    pub items: Vec<OverviewItemData>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct OverviewStateData {
    pub paused: bool,
    pub active: usize,
    pub workspaces: Vec<OverviewWorkspaceData>,
}

#[derive(Deserialize)]
struct RawSnapshot {
    paused: bool,
    #[serde(default)]
    elevated: bool,
    active: usize,
    #[serde(default)]
    cheatsheet_seq: u64,
    #[serde(default)]
    overview_seq: u64,
    workspaces: Vec<WorkspaceInfo>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct WmSnapshot {
    connected: bool,
    paused: bool,
    elevated: bool,
    active_ws: usize,
    cheatsheet_seq: u64,
    overview_seq: u64,
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
        elevated: raw.elevated,
        active_ws: raw.active,
        cheatsheet_seq: raw.cheatsheet_seq,
        overview_seq: raw.overview_seq,
        inactive_windows,
        workspaces: raw.workspaces,
    }
}

fn apply_snapshot(app: Option<&AppHandle>, next: WmSnapshot) {
    let (osd_payload, cheatsheet_toggled, overview_toggled) = {
        let Ok(mut g) = WM_SNAPSHOT.write() else { return };
        if *g == next {
            return;
        }
        let payload = if g.connected && next.connected && !next.paused && g.active_ws != next.active_ws {
            Some(WorkspaceOsdPayload {
                active: next.active_ws,
                total: next.workspaces.len().max(next.active_ws + 1),
            })
        } else {
            None
        };
        let cs_toggled = g.connected && next.connected && g.cheatsheet_seq != next.cheatsheet_seq;
        let ov_toggled = g.connected && next.connected && g.overview_seq != next.overview_seq;
        *g = next;
        (payload, cs_toggled, ov_toggled)
    };
    if let Some(app) = app {
        let _ = app.emit("windows-changed", ());
        let _ = app.emit("workspaces-changed", ());
        if let Some(payload) = osd_payload {
            crate::edge::notify_notch_popup_with(app, Duration::from_millis(1600), move |a| {
                let _ = a.emit_to("notch", "workspace-osd", payload);
            });
        }
        if cheatsheet_toggled {
            toggle_cheatsheet_ui(app);
        }
        if overview_toggled {
            toggle_overview_ui(app, "hotkey");
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
            apply_snapshot(APP.get(), build_snapshot(raw));
        }
    }
}

fn find_vscroll_exe() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("vscroll-wm.exe"));
            for anc in dir.ancestors().take(6) {
                candidates.push(anc.join(r"vscroll-wm\target\release\vscroll-wm.exe"));
                candidates.push(anc.join(r"vscroll-wm\target\debug\vscroll-wm.exe"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("vscroll-wm.exe"));
        candidates.push(cwd.join(r"vscroll-wm\target\release\vscroll-wm.exe"));
        candidates.push(cwd.join(r"vscroll-wm\target\debug\vscroll-wm.exe"));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join(r"vscroll-wm\target\release\vscroll-wm.exe"));
            candidates.push(parent.join(r"vscroll-wm\target\debug\vscroll-wm.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn wm_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("VSCROLL_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("vscroll-wm.conf"))
}

fn wm_ignore_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("VSCROLL_IGNORE") {
        return Ok(PathBuf::from(p));
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("vscroll-wm.ignore"))
}

fn ensure_wm_files(app: &AppHandle) -> (Option<PathBuf>, Option<PathBuf>) {
    let conf = wm_config_path(app).ok().inspect(|p| {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !p.exists() {
            let _ = std::fs::write(p, "# vscroll-wm configuration\n");
        }
    });
    let ign = wm_ignore_path(app).ok().inspect(|p| {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !p.exists() {
            let _ = std::fs::write(
                p,
                "# One rule per line: exe:app.exe, class:WindowClass, title:substring\n",
            );
        }
    });
    (conf, ign)
}

fn elevated_task_installed() -> bool {
    if !std::path::Path::new(ELEVATED_EXE_PATH).is_file() {
        return false;
    }
    Command::new("schtasks.exe")
        .args(["/Query", "/TN", ELEVATED_TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn spawn_elevated_task() -> bool {
    let ok = Command::new("schtasks.exe")
        .args(["/Run", "/TN", ELEVATED_TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        return false;
    }
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(80));
        if send_cmd("list").is_ok() {
            return true;
        }
    }
    false
}

fn maybe_spawn_vscroll(app: &AppHandle) {
    if send_cmd("list").is_ok() {
        return;
    }
    let _ = ensure_wm_files(app);
    if elevated_task_installed() && spawn_elevated_task() {
        SPAWNED_BY_ORUMI.store(true, Ordering::Relaxed);
        eprintln!("[wm-ipc] launched elevated task {ELEVATED_TASK_NAME}");
        return;
    }
    let Some(exe) = find_vscroll_exe() else {
        eprintln!("[wm-ipc] vscroll-wm.exe not found; waiting for manual launch");
        return;
    };
    let mut cmd = Command::new(&exe);
    cmd.args(["--no-tray", "--no-osd", "--no-cheatsheet", "--no-overview"])
        .creation_flags(CREATE_NO_WINDOW);
    let (conf, ign) = ensure_wm_files(app);
    if std::env::var_os("VSCROLL_CONFIG").is_none() {
        if let Some(conf) = conf {
            cmd.env("VSCROLL_CONFIG", conf);
        }
    }
    if std::env::var_os("VSCROLL_IGNORE").is_none() {
        if let Some(ign) = ign {
            cmd.env("VSCROLL_IGNORE", ign);
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

pub fn sync_shell_relay(app: &AppHandle) {
    let alt_tab = crate::config::current(app).alt_tab;
    let winkey = crate::activities::is_win_key_enabled();
    let cmd = format!(
        "shell_relay {} {}",
        if alt_tab { 1 } else { 0 },
        if winkey { 1 } else { 0 }
    );
    thread::spawn(move || {
        let _ = send_cmd(&cmd);
    });
}

pub fn sync_shell_relay_current() {
    if let Some(app) = APP.get() {
        sync_shell_relay(app);
    }
}

/// Sync the screen border's physical pixel thickness as outer padding in `vscroll-wm`.
pub fn sync_outer_inset(app: &AppHandle) {
    let cfg = crate::config::current(app);
    let px = if cfg.screen_border {
        let scale = app
            .get_webview_window("border")
            .or_else(|| app.get_webview_window("bar"))
            .and_then(|w| w.primary_monitor().ok().flatten())
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        ((cfg.screen_border_size as f64) * scale * cfg.ui_scale).ceil() as i32
    } else {
        0
    };
    let cmd = format!("outer_inset {px}");
    thread::spawn(move || {
        if let Err(e) = send_cmd(&cmd) {
            eprintln!("[wm-ipc] {cmd} failed: {e}");
        }
    });
}

fn handle_event_line(app: &AppHandle, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return;
    };
    if let Some(kind) = val.get("shell_event").and_then(|s| s.as_str()) {
        if kind.starts_with("alt_tab_") {
            let reverse = val.get("reverse").and_then(|b| b.as_bool()).unwrap_or(false);
            let delta = val.get("delta").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
            crate::altswitch::on_elevated_event(kind, reverse, delta);
        } else if kind.starts_with("win_") {
            let combo = val.get("combo").and_then(|b| b.as_bool()).unwrap_or(false);
            crate::activities::on_elevated_win_event(kind, combo);
        }
        return;
    }
    if let Ok(raw) = serde_json::from_value::<RawSnapshot>(val) {
        apply_snapshot(Some(app), build_snapshot(raw));
    }
}

/// Start the event listener and auto-launch `vscroll-wm.exe` if it is not already running.
pub fn start(app: AppHandle) {
    SHUTDOWN.store(false, Ordering::Relaxed);
    let _ = APP.set(app.clone());
    thread::Builder::new()
        .name("vscroll-ipc-client".into())
        .spawn(move || {
            maybe_spawn_vscroll(&app);
            while !SHUTDOWN.load(Ordering::Relaxed) {
                match File::open(EVENT_PIPE) {
                    Ok(file) => {
                        let _ = send_cmd("osd off");
                        let _ = send_cmd("cheatsheet_ui off");
                        let _ = send_cmd("overview_ui off");
                        sync_shell_relay(&app);
                        sync_outer_inset(&app);
                        let sync_app = app.clone();
                        thread::spawn(move || sync_theme_colors(&sync_app));
                        let reader = BufReader::new(file);
                        for line in reader.lines() {
                            if SHUTDOWN.load(Ordering::Relaxed) {
                                return;
                            }
                            let Ok(line) = line else { break };
                            handle_event_line(&app, &line);
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
    } else {
        let _ = send_cmd("outer_inset 0");
        let _ = send_cmd("shell_relay 0 0");
        let _ = send_cmd("osd on");
        let _ = send_cmd("cheatsheet_ui on");
        let _ = send_cmd("overview_ui on");
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

pub fn ensure_hook() {
    thread::spawn(|| {
        let _ = send_cmd("reinstall_hook");
    });
}

pub fn toggle_cheatsheet_ui(app: &AppHandle) {
    let panels = app.state::<crate::Panels>();
    if panels.is_open("launcher") && crate::edge::launcher_is_cheatsheet() {
        panels.set("launcher", false);
        return;
    }
    crate::edge::launcher_view("cheatsheet".to_string(), app.clone());
    let _ = app.emit_to("launcher", "launcher-view", "cheatsheet");
    panels.set("launcher", true);
}

pub fn toggle_overview_ui(app: &AppHandle, source: &str) {
    let panels = app.state::<crate::Panels>();
    if panels.is_open("overview") {
        panels.set("overview", false);
        return;
    }
    let _ = app.emit_to("overview", "overview-source", source);
    panels.set("bar", false);
    panels.set("preview", false);
    panels.set("overview", true);
}

#[tauri::command]
pub fn wm_cheatsheet_toggle(app: AppHandle) {
    toggle_cheatsheet_ui(&app);
}

#[tauri::command]
pub fn wm_cheatsheet_groups() -> Result<Vec<CheatsheetGroup>, String> {
    let json = send_cmd("cheatsheet_data")?;
    serde_json::from_str(&json).map_err(|e| format!("invalid cheatsheet IPC reply: {e}"))
}

#[tauri::command]
pub fn wm_overview_toggle(app: AppHandle, source: Option<String>) {
    toggle_overview_ui(&app, source.as_deref().unwrap_or("bar"));
}

#[tauri::command]
pub fn wm_overview_state(app: AppHandle) -> Result<OverviewStateData, String> {
    let json = send_cmd("overview_data")?;
    let mut data: OverviewStateData =
        serde_json::from_str(&json).map_err(|e| format!("invalid overview IPC reply: {e}"))?;

    let icons: HashMap<i64, String> = crate::winlist::collect_all_windows()
        .into_iter()
        .map(|w| {
            let mut parts = Vec::new();
            if !w.aumid.is_empty() {
                parts.push(format!("aumid:{}", w.aumid));
            }
            if !w.path.is_empty() {
                parts.push(format!("path:{}", w.path));
            }
            parts.push(format!("hwnd:{}", w.hwnd));
            (w.hwnd, parts.join("\n"))
        })
        .collect();

    let dir = crate::capture::cache_dir(&app);
    let cached = dir.as_ref().map(|d| crate::capture::latest_all(d));
    let scope = app.asset_protocol_scope();
    let mut active_hwnds = Vec::new();
    for ws in &mut data.workspaces {
        for item in &mut ws.items {
            item.icon = icons.get(&item.hwnd).cloned();
            if let Some(p) = cached.as_ref().and_then(|c| c.get(&item.hwnd)) {
                let _ = scope.allow_file(p);
                item.thumb = Some(p.to_string_lossy().into_owned());
            }
            if ws.active {
                active_hwnds.push(item.hwnd);
            }
        }
    }
    if dir.is_some() && !active_hwnds.is_empty() {
        let app_bg = app.clone();
        thread::spawn(move || {
            let Some(dir) = crate::capture::cache_dir(&app_bg) else { return };
            let scope = app_bg.asset_protocol_scope();
            let mut refreshed: HashMap<i64, String> = HashMap::new();
            for hwnd in active_hwnds {
                if let Some(p) = crate::capture::refresh(&dir, hwnd) {
                    let _ = scope.allow_file(&p);
                    refreshed.insert(hwnd, p.to_string_lossy().into_owned());
                }
            }
            if !refreshed.is_empty() {
                let _ = app_bg.emit_to("overview", "overview-thumbs", refreshed);
            }
        });
    }

    Ok(data)
}

#[tauri::command]
pub fn wm_overview_drop(
    app: AppHandle,
    src_hwnd: i64,
    target_ws: usize,
    kind: String,
    target_hwnd: Option<i64>,
    first: Option<bool>,
) -> Result<(), String> {
    let cmd = format!(
        "overview_drop {src_hwnd} {target_ws} {kind} {} {}",
        target_hwnd.unwrap_or(0),
        if first.unwrap_or(false) { 1 } else { 0 }
    );
    let _ = send_cmd(&cmd)?;
    refresh_snapshot_sync();
    let _ = app.emit_to("overview", "overview-refresh", ());
    Ok(())
}

#[tauri::command]
pub fn wm_overview_pick(app: AppHandle, hwnd: i64) -> Result<(), String> {
    app.state::<crate::Panels>().set("overview", false);
    let _ = send_cmd(&format!("overview_pick {hwnd}"))?;
    refresh_snapshot_sync();
    Ok(())
}

/// True if `vscroll-wm` is connected.
pub fn is_connected() -> bool {
    WM_SNAPSHOT.read().is_ok_and(|g| g.connected)
}

/// True if `vscroll-wm` is connected and running elevated (capable of controlling admin windows).
pub fn is_elevated() -> bool {
    WM_SNAPSHOT
        .read()
        .is_ok_and(|g| g.connected && g.elevated)
}

pub fn focus_window(hwnd: i64) -> Result<(), String> {
    let _ = send_cmd(&format!("focus {hwnd}"))?;
    Ok(())
}

pub fn minimize_window(hwnd: i64) -> Result<(), String> {
    let _ = send_cmd(&format!("minimize {hwnd}"))?;
    Ok(())
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

// ---------- WM configuration read/write & commands ----------

const THEME_BORDER_MARKER: &str = "# orumi:focus_border = theme";

fn default_border_mode() -> String {
    "off".to_string()
}

fn default_true() -> bool {
    true
}

fn default_snap_trigger() -> i32 {
    24
}

#[derive(Clone, Deserialize, Serialize)]
pub struct WmConfigData {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub elevated: bool,
    #[serde(default)]
    pub elevated_task_installed: bool,
    #[serde(default)]
    pub config_path: String,
    #[serde(default)]
    pub ignore_path: String,
    pub gap: i32,
    pub anim_ms: i32,
    pub focus_border: String,
    #[serde(default = "default_border_mode")]
    pub focus_border_mode: String,
    pub resize_step: i32,
    pub height_mode: String,
    pub height_step: i32,
    pub mouse_drag: bool,
    pub remember_layout: bool,
    #[serde(default = "default_true")]
    pub snap_layouts: bool,
    #[serde(default = "default_true")]
    pub snap_assist: bool,
    #[serde(default = "default_snap_trigger")]
    pub snap_trigger: i32,
    #[serde(default = "default_true")]
    pub snap_layouts_defaults: bool,
    #[serde(default)]
    pub binds: HashMap<String, String>,
    #[serde(default)]
    pub ignore_text: String,
}

fn default_wm_config() -> WmConfigData {
    WmConfigData {
        connected: false,
        paused: false,
        elevated: false,
        elevated_task_installed: false,
        config_path: String::new(),
        ignore_path: String::new(),
        gap: 16,
        anim_ms: 160,
        focus_border: String::new(),
        focus_border_mode: "off".to_string(),
        resize_step: 80,
        height_mode: "preset".to_string(),
        height_step: 80,
        mouse_drag: true,
        remember_layout: true,
        snap_layouts: true,
        snap_assist: true,
        snap_trigger: 24,
        snap_layouts_defaults: true,
        binds: HashMap::new(),
        ignore_text: String::new(),
    }
}

fn parse_wm_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn normalize_hex(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
        return Some(String::new());
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex.to_ascii_lowercase()))
    } else {
        None
    }
}

fn parse_wm_conf(text: &str) -> WmConfigData {
    let mut d = default_wm_config();
    let mut theme_marked = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line == THEME_BORDER_MARKER {
            theme_marked = true;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        match key.as_str() {
            "gap" => {
                if let Ok(n) = val.parse::<i32>() {
                    d.gap = n.clamp(0, 200);
                }
            }
            "anim_ms" => {
                if val.is_empty() {
                    d.anim_ms = 160;
                } else if let Ok(n) = val.parse::<f32>() {
                    if n.is_finite() {
                        d.anim_ms = (n.round() as i32).clamp(0, 1000);
                    }
                }
            }
            "focus_border" => {
                if let Some(hex) = normalize_hex(val) {
                    d.focus_border = hex;
                }
            }
            "resize_step" => {
                if let Ok(n) = val.parse::<i32>() {
                    d.resize_step = n.clamp(10, 400);
                }
            }
            "height_mode" => {
                let m = val.to_ascii_lowercase();
                if m == "preset" || m == "step" {
                    d.height_mode = m;
                }
            }
            "height_step" => {
                if let Ok(n) = val.parse::<i32>() {
                    d.height_step = n.clamp(10, 400);
                }
            }
            "mouse_drag" => {
                if let Some(b) = parse_wm_bool(val) {
                    d.mouse_drag = b;
                }
            }
            "remember_layout" => {
                if let Some(b) = parse_wm_bool(val) {
                    d.remember_layout = b;
                }
            }
            "snap_layouts" => {
                if let Some(b) = parse_wm_bool(val) {
                    d.snap_layouts = b;
                }
            }
            "snap_assist" => {
                if let Some(b) = parse_wm_bool(val) {
                    d.snap_assist = b;
                }
            }
            "snap_trigger" => {
                if let Ok(n) = val.parse::<i32>() {
                    d.snap_trigger = n.clamp(2, 300);
                }
            }
            "snap_layouts_defaults" => {
                if let Some(b) = parse_wm_bool(val) {
                    d.snap_layouts_defaults = b;
                }
            }
            _ => {
                if let Some(action) = key.strip_prefix("bind.") {
                    let action = action.trim();
                    if !action.is_empty() && !val.is_empty() {
                        d.binds.insert(action.to_string(), val.to_string());
                    }
                }
            }
        }
    }
    d.focus_border_mode = if theme_marked {
        "theme".to_string()
    } else if !d.focus_border.is_empty() {
        "custom".to_string()
    } else {
        "off".to_string()
    };
    d
}

fn current_normalized_theme_colors() -> Option<(String, String, String)> {
    let (surface, container, primary) = crate::theme::current_colors()?;
    let surface = normalize_hex(&surface).filter(|s| !s.is_empty())?;
    let container = normalize_hex(&container).filter(|s| !s.is_empty())?;
    let primary = normalize_hex(&primary).filter(|s| !s.is_empty())?;
    Some((surface, container, primary))
}

fn render_wm_conf(
    existing: &str,
    data: &WmConfigData,
    theme_colors: Option<(&str, &str, &str)>,
) -> String {
    let focus_border_val = if data.focus_border_mode == "off" {
        "off".to_string()
    } else {
        normalize_hex(&data.focus_border)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "off".to_string())
    };
    let height_mode_val = if data.height_mode.eq_ignore_ascii_case("step") {
        "step"
    } else {
        "preset"
    };

    let mut managed: Vec<(&str, String)> = vec![
        ("gap", data.gap.clamp(0, 200).to_string()),
        ("anim_ms", data.anim_ms.clamp(0, 1000).to_string()),
        ("focus_border", focus_border_val),
        ("resize_step", data.resize_step.clamp(10, 400).to_string()),
        ("height_mode", height_mode_val.to_string()),
        ("height_step", data.height_step.clamp(10, 400).to_string()),
        (
            "mouse_drag",
            if data.mouse_drag { "on" } else { "off" }.to_string(),
        ),
        (
            "remember_layout",
            if data.remember_layout { "on" } else { "off" }.to_string(),
        ),
        (
            "snap_layouts",
            if data.snap_layouts { "on" } else { "off" }.to_string(),
        ),
        (
            "snap_assist",
            if data.snap_assist { "on" } else { "off" }.to_string(),
        ),
        ("snap_trigger", data.snap_trigger.clamp(2, 300).to_string()),
        (
            "snap_layouts_defaults",
            if data.snap_layouts_defaults { "on" } else { "off" }.to_string(),
        ),
    ];
    if let Some((surface, container, primary)) = theme_colors {
        managed.extend([
            ("overview_color_background", surface.to_string()),
            ("overview_color_box", container.to_string()),
            ("overview_color_active", primary.to_string()),
            ("overview_color_selected", primary.to_string()),
            ("overview_color_view", primary.to_string()),
        ]);
    }

    let mut written_keys: HashSet<String> = HashSet::new();
    let mut written_binds: HashSet<String> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();

    for raw in existing.lines() {
        let trimmed = raw.trim();
        if trimmed == THEME_BORDER_MARKER {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            lines.push(raw.to_string());
            continue;
        }
        let Some((k, _)) = trimmed.split_once('=') else {
            lines.push(raw.to_string());
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        if let Some((_, val)) = managed.iter().find(|(mk, _)| *mk == key) {
            if written_keys.insert(key.clone()) {
                if key == "focus_border" && data.focus_border_mode == "theme" {
                    lines.push(THEME_BORDER_MARKER.to_string());
                }
                lines.push(format!("{key} = {val}"));
            }
            continue;
        }
        if let Some(action) = key.strip_prefix("bind.") {
            let action = action.trim().to_string();
            if let Some(val) = data.binds.get(&action).map(|s| s.trim()).filter(|s| !s.is_empty())
            {
                if written_binds.insert(action.clone()) {
                    lines.push(format!("bind.{action} = {val}"));
                }
            }
            continue;
        }
        lines.push(raw.to_string());
    }

    for (key, val) in managed {
        if !written_keys.contains(key) {
            if key == "focus_border" && data.focus_border_mode == "theme" {
                lines.push(THEME_BORDER_MARKER.to_string());
            }
            lines.push(format!("{key} = {val}"));
        }
    }

    let mut extra_binds: Vec<(&String, &String)> = data
        .binds
        .iter()
        .filter(|(k, v)| !written_binds.contains(*k) && !v.trim().is_empty())
        .collect();
    extra_binds.sort_by(|a, b| a.0.cmp(b.0));
    for (action, val) in extra_binds {
        lines.push(format!("bind.{action} = {}", val.trim()));
    }

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Update `vscroll-wm.conf` and reload `vscroll-wm` so Snap Menu/Assist and (when enabled) focus border follow ORUMI's live theme.
pub fn sync_theme_colors(app: &AppHandle) {
    let Some((surface, container, primary)) = current_normalized_theme_colors() else {
        return;
    };
    let Ok(conf_path) = wm_config_path(app) else {
        return;
    };
    let existing = std::fs::read_to_string(&conf_path).unwrap_or_default();
    let mut data = parse_wm_conf(&existing);
    if data.focus_border_mode == "theme" {
        data.focus_border = primary.clone();
    }
    let rendered = render_wm_conf(&existing, &data, Some((&surface, &container, &primary)));
    if existing.replace("\r\n", "\n") == rendered {
        return;
    }
    if std::fs::write(&conf_path, rendered).is_ok() {
        let _ = send_cmd("reload");
        refresh_snapshot_sync();
    }
}

#[tauri::command]
pub fn wm_config_get(app: AppHandle) -> Result<WmConfigData, String> {
    let (conf_opt, ign_opt) = ensure_wm_files(&app);
    let conf_path = conf_opt.ok_or_else(|| "cannot resolve vscroll-wm.conf path".to_string())?;
    let ign_path = ign_opt.ok_or_else(|| "cannot resolve vscroll-wm.ignore path".to_string())?;

    let conf_text = std::fs::read_to_string(&conf_path).unwrap_or_default();
    let ign_text = std::fs::read_to_string(&ign_path).unwrap_or_default();

    let mut data = parse_wm_conf(&conf_text);
    data.config_path = conf_path.to_string_lossy().into_owned();
    data.ignore_path = ign_path.to_string_lossy().into_owned();
    data.ignore_text = ign_text;
    if let Ok(g) = WM_SNAPSHOT.read() {
        data.connected = g.connected;
        data.paused = g.paused;
        data.elevated = g.connected && g.elevated;
    }
    data.elevated_task_installed = elevated_task_installed();
    Ok(data)
}

#[tauri::command]
pub fn wm_config_save(
    app: AppHandle,
    mut data: WmConfigData,
    restart: Option<bool>,
) -> Result<WmConfigData, String> {
    let theme_colors = current_normalized_theme_colors();
    if data.focus_border_mode == "theme" {
        if let Some((_, _, ref primary)) = theme_colors {
            data.focus_border = primary.clone();
        } else if let Some(primary) = crate::theme::current_primary().and_then(|p| normalize_hex(&p))
        {
            if !primary.is_empty() {
                data.focus_border = primary;
            }
        }
    } else if data.focus_border_mode == "off" {
        data.focus_border.clear();
    }

    let (conf_opt, ign_opt) = ensure_wm_files(&app);
    let conf_path = conf_opt.ok_or_else(|| "cannot resolve vscroll-wm.conf path".to_string())?;
    let ign_path = ign_opt.ok_or_else(|| "cannot resolve vscroll-wm.ignore path".to_string())?;

    let existing = std::fs::read_to_string(&conf_path).unwrap_or_default();
    let rendered = render_wm_conf(
        &existing,
        &data,
        theme_colors
            .as_ref()
            .map(|(s, c, p)| (s.as_str(), c.as_str(), p.as_str())),
    );
    std::fs::write(&conf_path, rendered).map_err(|e| format!("failed writing config: {e}"))?;

    let mut ign_out = data.ignore_text.replace("\r\n", "\n");
    if !ign_out.is_empty() && !ign_out.ends_with('\n') {
        ign_out.push('\n');
    }
    std::fs::write(&ign_path, ign_out).map_err(|e| format!("failed writing ignore rules: {e}"))?;

    if restart.unwrap_or(false) {
        let _ = send_cmd("quit");
        for _ in 0..16 {
            thread::sleep(Duration::from_millis(40));
            if send_cmd("list").is_err() {
                break;
            }
        }
        maybe_spawn_vscroll(&app);
        thread::sleep(Duration::from_millis(120));
        refresh_snapshot_sync();
    } else if send_cmd("reload").is_ok() {
        refresh_snapshot_sync();
    }

    wm_config_get(app)
}

#[tauri::command]
pub fn wm_toggle_pause_cmd() -> Result<bool, String> {
    let _ = send_cmd("toggle_pause")?;
    refresh_snapshot_sync();
    Ok(WM_SNAPSHOT.read().is_ok_and(|g| g.paused))
}

#[tauri::command]
pub fn wm_reapply_cmd() -> Result<(), String> {
    let _ = send_cmd("reapply")?;
    refresh_snapshot_sync();
    Ok(())
}

fn current_user_sid() -> Result<String, String> {
    let out = Command::new("whoami.exe")
        .args(["/user", "/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed reading current user SID: {e}"))?;
    if !out.status.success() {
        return Err("whoami /user failed".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let sid = text
        .trim()
        .rsplit(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_string();
    if !sid.starts_with("S-1-") {
        return Err(format!("unexpected SID format: {sid}"));
    }
    Ok(sid)
}

fn ps_single_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn encode_ps_utf16(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

fn run_elevated_ps(script: &str) -> Result<(), String> {
    let encoded = encode_ps_utf16(script);
    let launcher = format!(
        "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand','{encoded}'); exit $p.ExitCode"
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &launcher])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed launching UAC prompt: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "UAC prompt cancelled or elevated setup failed".into()
        } else {
            stderr
        });
    }
    Ok(())
}

/// Install/update or remove the elevated Scheduled Task for `vscroll-wm.exe` (one UAC prompt)
/// and restart `vscroll-wm` cleanly so admin-window control activates immediately.
#[tauri::command]
pub async fn wm_setup_elevated(app: AppHandle, enable: bool) -> Result<WmConfigData, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sid = current_user_sid()?;
        let script = if enable {
            let src = find_vscroll_exe()
                .ok_or_else(|| "vscroll-wm.exe was not found; build it first.".to_string())?;
            let src_q = ps_single_quote(&src.to_string_lossy());
            let dir_q = ps_single_quote(ELEVATED_DIR);
            let dst_q = ps_single_quote(ELEVATED_EXE_PATH);
            let task_q = ps_single_quote(ELEVATED_TASK_NAME);
            let sid_q = ps_single_quote(&sid);
            format!(
                r#"$ErrorActionPreference = 'Stop'
$taskName = '{task_q}'
$targetDir = '{dir_q}'
$targetExe = '{dst_q}'
$srcExe = '{src_q}'
$sid = '{sid_q}'
try {{ & schtasks.exe /End /TN $taskName *> $null }} catch {{}}
Get-Process -Name 'vscroll-wm' -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq $targetExe }} | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 150
New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
Copy-Item -LiteralPath $srcExe -Destination $targetExe -Force
$escExe = [System.Security.SecurityElement]::Escape($targetExe)
$escSid = [System.Security.SecurityElement]::Escape($sid)
$xml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Elevated vscroll-wm helper for ORUMI.</Description></RegistrationInfo>
  <Triggers />
  <Principals>
    <Principal id="Author">
      <UserId>$escSid</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>$escExe</Command>
      <Arguments>--no-tray --no-osd --no-cheatsheet --no-overview</Arguments>
    </Exec>
  </Actions>
</Task>
"@
$sddl = "D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GRGX;;;$sid)"
$svc = New-Object -ComObject Schedule.Service
$svc.Connect()
$folder = $svc.GetFolder('\')
$null = $folder.RegisterTask($taskName, $xml, 6, $sid, $null, 3, $sddl)
"#
            )
        } else {
            let dir_q = ps_single_quote(ELEVATED_DIR);
            let dst_q = ps_single_quote(ELEVATED_EXE_PATH);
            let task_q = ps_single_quote(ELEVATED_TASK_NAME);
            format!(
                r#"$ErrorActionPreference = 'Stop'
$taskName = '{task_q}'
$targetDir = '{dir_q}'
$targetExe = '{dst_q}'
try {{ & schtasks.exe /End /TN $taskName *> $null }} catch {{}}
Get-Process -Name 'vscroll-wm' -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq $targetExe }} | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 150
try {{ & schtasks.exe /Delete /TN $taskName /F *> $null }} catch {{}}
if (Test-Path -LiteralPath $targetExe -PathType Leaf) {{ Remove-Item -LiteralPath $targetExe -Force }}
if ((Test-Path -LiteralPath $targetDir -PathType Container) -and ((Get-ChildItem -LiteralPath $targetDir -Force | Measure-Object).Count -eq 0)) {{
  Remove-Item -LiteralPath $targetDir -Force
}}
"#
            )
        };

        let _ = send_cmd("quit");
        for _ in 0..16 {
            thread::sleep(Duration::from_millis(40));
            if send_cmd("list").is_err() {
                break;
            }
        }

        let setup_res = run_elevated_ps(&script);
        maybe_spawn_vscroll(&app);
        for _ in 0..15 {
            if send_cmd("list").is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(80));
        }
        refresh_snapshot_sync();
        sync_shell_relay(&app);
        setup_res?;
        wm_config_get(app)
    })
    .await
    .map_err(|e| e.to_string())?
}
