//! Named Pipe IPC server for external shell integration (ORUMI).
//!
//! - `\\.\pipe\vscroll-wm`        line-based request/response commands
//! - `\\.\pipe\vscroll-wm-events` newline-delimited JSON state snapshots pushed on change

use std::collections::VecDeque;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use layout::{InsertPosition, Span, WindowId};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WPARAM,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, PIPE_ACCESS_DUPLEX, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindow, PostMessageW, PostThreadMessageW, ShowWindow, SC_CLOSE, SW_MINIMIZE,
    SW_RESTORE, WM_SYSCOMMAND,
};

use crate::app::workspace::{slide_dir, Slide};
use crate::app::App;
use crate::consts::WM_IPC;
use crate::winutil::{title, work_area};
use crate::{cheatsheet, osd, overview, rules, tray};

pub enum Cmd {
    List {
        reply: SyncSender<String>,
    },
    Switch {
        index: usize,
        focus: bool,
        reply: SyncSender<String>,
    },
    Move {
        hwnd: i64,
        index: usize,
        reply: SyncSender<String>,
    },
    Ensure {
        hwnd: i64,
        reply: SyncSender<String>,
    },
    TogglePause {
        reply: SyncSender<String>,
    },
    Reload {
        reply: SyncSender<String>,
    },
    Reapply {
        reply: SyncSender<String>,
    },
    Cheatsheet {
        reply: SyncSender<String>,
    },
    CheatsheetData {
        reply: SyncSender<String>,
    },
    SetCheatsheetUi {
        enabled: bool,
        reply: SyncSender<String>,
    },
    OverviewData {
        reply: SyncSender<String>,
    },
    SetOverviewUi {
        enabled: bool,
        reply: SyncSender<String>,
    },
    OverviewDrop {
        src_hwnd: i64,
        target_ws: usize,
        kind: String,
        target_hwnd: i64,
        first: bool,
        reply: SyncSender<String>,
    },
    OverviewPick {
        hwnd: i64,
        reply: SyncSender<String>,
    },
    SetOsd {
        enabled: bool,
        reply: SyncSender<String>,
    },
    ReinstallHook {
        reply: SyncSender<String>,
    },
    Focus {
        hwnd: i64,
        reply: SyncSender<String>,
    },
    Minimize {
        hwnd: i64,
        reply: SyncSender<String>,
    },
    Close {
        hwnd: i64,
        reply: SyncSender<String>,
    },
    ShellRelay {
        alt_tab: bool,
        winkey: bool,
        reply: SyncSender<String>,
    },
    Quit {
        reply: SyncSender<String>,
    },
}

static CHEATSHEET_SEQ: AtomicU64 = AtomicU64::new(0);
static OVERVIEW_SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) fn bump_cheatsheet_seq() {
    CHEATSHEET_SEQ.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn bump_overview_seq() {
    OVERVIEW_SEQ.fetch_add(1, Ordering::Relaxed);
}

struct EventState {
    snapshot: String,
    next_seq: u64,
    events: VecDeque<(u64, String)>,
}

struct EventHub {
    state: Mutex<EventState>,
    cvar: Condvar,
}

static HUB: OnceLock<EventHub> = OnceLock::new();

fn hub() -> &'static EventHub {
    HUB.get_or_init(|| EventHub {
        state: Mutex::new(EventState {
            snapshot: String::new(),
            next_seq: 1,
            events: VecDeque::new(),
        }),
        cvar: Condvar::new(),
    })
}

/// Publish the latest workspace snapshot to any connected event subscriber.
pub(crate) fn publish(json: String) {
    let h = hub();
    if let Ok(mut g) = h.state.lock() {
        if g.snapshot != json {
            g.snapshot = json;
            h.cvar.notify_all();
        }
    }
}

/// Push a transient shell event (e.g. elevated Alt+Tab / Win relay) to connected subscribers.
pub(crate) fn publish_shell_event(json: &str) {
    let h = hub();
    if let Ok(mut g) = h.state.lock() {
        let seq = g.next_seq;
        g.next_seq = g.next_seq.saturating_add(1);
        g.events.push_back((seq, json.to_string()));
        while g.events.len() > 64 {
            g.events.pop_front();
        }
        h.cvar.notify_all();
    }
}

pub(crate) fn snapshot_json(app: &App) -> String {
    let paused = tray::PAUSED.load(Ordering::Relaxed);
    let elevated = crate::winutil::is_elevated();
    let active = app.ws.active();
    let count = app.ws.count();
    let cheatsheet_seq = CHEATSHEET_SEQ.load(Ordering::Relaxed);
    let overview_seq = OVERVIEW_SEQ.load(Ordering::Relaxed);
    let mut out = format!(
        r#"{{"paused":{paused},"elevated":{elevated},"active":{active},"cheatsheet_seq":{cheatsheet_seq},"overview_seq":{overview_seq},"workspaces":["#
    );
    for idx in 0..count {
        if idx > 0 {
            out.push(',');
        }
        let is_active = idx == active;
        out.push_str(&format!(r#"{{"index":{idx},"active":{is_active},"windows":["#));
        let ids = app.ws.window_ids(idx);
        for (wi, wid) in ids.iter().filter(|w| !w.is_slot()).enumerate() {
            if wi > 0 {
                out.push(',');
            }
            let hwnd = wid.0 as usize as isize as i64;
            out.push_str(&hwnd.to_string());
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

unsafe fn overview_data_json(app: &App) -> String {
    let paused = tray::PAUSED.load(Ordering::Relaxed);
    let active = app.ws.active();
    let count = app.ws.count();
    let mut out = format!(r#"{{"paused":{paused},"active":{active},"workspaces":["#);
    for idx in 0..count {
        if idx > 0 {
            out.push(',');
        }
        let is_active = idx == active;
        let Some(strip) = app.ws.strip(idx) else {
            out.push_str(&format!(
                r#"{{"index":{idx},"active":{is_active},"width":1920,"height":1080,"view_y":0,"view_h":1080,"focused":0,"items":[]}}"#
            ));
            continue;
        };
        let ov = strip.overview();
        let focused = strip
            .focused()
            .filter(|f| !f.is_slot())
            .map_or(0, |f| f.0 as usize as isize as i64);
        let strip_items = strip.items();
        out.push_str(&format!(
            r#"{{"index":{idx},"active":{is_active},"width":{},"height":{},"view_y":{},"view_h":{},"focused":{focused},"items":["#,
            ov.width.max(1),
            ov.height.max(1),
            ov.view_y,
            ov.view_h.max(1)
        ));
        for (ii, it) in ov.items.iter().filter(|it| !it.id.is_slot()).enumerate() {
            if ii > 0 {
                out.push(',');
            }
            let hwnd = it.id.0 as usize as isize as i64;
            let span = strip_items
                .iter()
                .find(|s| s.id == it.id)
                .map_or(Span::Full, |s| s.span);
            let span_str = match span {
                Span::Left => "left",
                Span::Right => "right",
                _ => "full",
            };
            let (win_title, exe) = match app.map.get(&it.id) {
                Some(&h) => (
                    title(h),
                    rules::exe_name(h).unwrap_or_default(),
                ),
                None => (String::new(), String::new()),
            };
            out.push_str(&format!(
                r#"{{"hwnd":{hwnd},"x":{},"y":{},"w":{},"h":{},"span":"{span_str}","title":"{}","exe":"{}"}}"#,
                it.rect.x,
                it.rect.y,
                it.rect.w,
                it.rect.h,
                cheatsheet::json_escape(&win_title),
                cheatsheet::json_escape(&exe)
            ));
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

pub(crate) fn start(main_thread: u32) -> Receiver<Cmd> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("vscroll-ipc-cmd".into())
        .spawn(move || cmd_server(main_thread, tx));
    let _ = std::thread::Builder::new()
        .name("vscroll-ipc-events".into())
        .spawn(event_server);
    rx
}

unsafe fn connect_pipe(pipe: HANDLE) -> bool {
    match ConnectNamedPipe(pipe, None) {
        Ok(()) => true,
        Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
    }
}

fn parse_cmd(line: &str, reply: SyncSender<String>) -> Result<Cmd, String> {
    let mut parts = line.split_whitespace();
    let Some(op) = parts.next() else {
        return Err("empty command".into());
    };
    match op {
        "list" => Ok(Cmd::List { reply }),
        "switch" => {
            let index: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing workspace index".to_string())?;
            let focus = parts.next().is_none_or(|s| s != "0" && s != "false");
            Ok(Cmd::Switch { index, focus, reply })
        }
        "move" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            let index: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing workspace index".to_string())?;
            Ok(Cmd::Move { hwnd, index, reply })
        }
        "ensure" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            Ok(Cmd::Ensure { hwnd, reply })
        }
        "toggle_pause" => Ok(Cmd::TogglePause { reply }),
        "reload" => Ok(Cmd::Reload { reply }),
        "reapply" => Ok(Cmd::Reapply { reply }),
        "cheatsheet" => Ok(Cmd::Cheatsheet { reply }),
        "cheatsheet_data" => Ok(Cmd::CheatsheetData { reply }),
        "cheatsheet_ui" => {
            let enabled = parts.next().is_none_or(|s| s != "off" && s != "0" && s != "false");
            Ok(Cmd::SetCheatsheetUi { enabled, reply })
        }
        "overview_data" => Ok(Cmd::OverviewData { reply }),
        "overview_ui" => {
            let enabled = parts.next().is_none_or(|s| s != "off" && s != "0" && s != "false");
            Ok(Cmd::SetOverviewUi { enabled, reply })
        }
        "overview_drop" => {
            let src_hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing src_hwnd".to_string())?;
            let target_ws: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing target_ws".to_string())?;
            let kind = parts
                .next()
                .ok_or_else(|| "missing slot kind".to_string())?
                .to_string();
            let target_hwnd: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let first = parts.next().is_some_and(|s| s == "1" || s == "true");
            Ok(Cmd::OverviewDrop {
                src_hwnd,
                target_ws,
                kind,
                target_hwnd,
                first,
                reply,
            })
        }
        "overview_pick" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            Ok(Cmd::OverviewPick { hwnd, reply })
        }
        "osd" => {
            let enabled = parts.next().is_none_or(|s| s != "off" && s != "0" && s != "false");
            Ok(Cmd::SetOsd { enabled, reply })
        }
        "reinstall_hook" => Ok(Cmd::ReinstallHook { reply }),
        "focus" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            Ok(Cmd::Focus { hwnd, reply })
        }
        "minimize" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            Ok(Cmd::Minimize { hwnd, reply })
        }
        "close" => {
            let hwnd: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "missing hwnd".to_string())?;
            Ok(Cmd::Close { hwnd, reply })
        }
        "shell_relay" => {
            let alt_tab = parts.next().is_some_and(|s| s == "1" || s == "true" || s == "on");
            let winkey = parts.next().is_some_and(|s| s == "1" || s == "true" || s == "on");
            Ok(Cmd::ShellRelay { alt_tab, winkey, reply })
        }
        "quit" => Ok(Cmd::Quit { reply }),
        other => Err(format!("unknown command '{other}'")),
    }
}

struct PipeSd(*mut c_void);
unsafe impl Send for PipeSd {}
unsafe impl Sync for PipeSd {}

static PIPE_SD: OnceLock<Option<PipeSd>> = OnceLock::new();

/// Restrict Named Pipe access to SYSTEM, Administrators, and the current user SID with a
/// Medium Mandatory Integrity label so non-admin ORUMI (same user) can connect when elevated,
/// while Low-integrity / AppContainer processes and other users are denied.
unsafe fn pipe_security_attributes() -> Option<SECURITY_ATTRIBUTES> {
    let ptr = PIPE_SD.get_or_init(|| {
        let sddl = if let Some(sid) = crate::winutil::current_user_sid_string() {
            format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})S:(ML;;NW;;;ME)")
        } else {
            "D:(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)".to_string()
        };
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .is_ok()
            && !sd.0.is_null()
        {
            Some(PipeSd(sd.0))
        } else {
            None
        }
    });
    ptr.as_ref().map(|sd| SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    })
}

fn cmd_server(main_thread: u32, tx: Sender<Cmd>) {
    loop {
        unsafe {
            let sa = pipe_security_attributes();
            let pipe = CreateNamedPipeW(
                w!(r"\\.\pipe\vscroll-wm"),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                sa.as_ref().map(|s| s as *const SECURITY_ATTRIBUTES),
            );
            if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if !connect_pipe(pipe) {
                let _ = File::from_raw_handle(pipe.0);
                continue;
            }
            let mut file = File::from_raw_handle(pipe.0);
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&mut file);
                let _ = reader.read_line(&mut line);
            }
            let trimmed = line.trim();
            let response = if trimmed.is_empty() {
                "err empty command".to_string()
            } else {
                let (rtx, rrx) = mpsc::sync_channel(1);
                match parse_cmd(trimmed, rtx) {
                    Ok(cmd) => {
                        if tx.send(cmd).is_ok() {
                            let _ = PostThreadMessageW(main_thread, WM_IPC, WPARAM(0), LPARAM(0));
                            rrx.recv_timeout(Duration::from_secs(2))
                                .unwrap_or_else(|_| "err timeout".to_string())
                        } else {
                            "err shutdown".to_string()
                        }
                    }
                    Err(e) => format!("err {e}"),
                }
            };
            let _ = writeln!(file, "{response}");
            let _ = FlushFileBuffers(HANDLE(file.as_raw_handle()));
            let _ = DisconnectNamedPipe(HANDLE(file.as_raw_handle()));
            drop(file);
        }
    }
}

fn event_server() {
    let h = hub();
    loop {
        unsafe {
            let sa = pipe_security_attributes();
            let pipe = CreateNamedPipeW(
                w!(r"\\.\pipe\vscroll-wm-events"),
                PIPE_ACCESS_OUTBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                8192,
                0,
                0,
                sa.as_ref().map(|s| s as *const SECURITY_ATTRIBUTES),
            );
            if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if !connect_pipe(pipe) {
                let _ = File::from_raw_handle(pipe.0);
                continue;
            }
            let mut file = File::from_raw_handle(pipe.0);
            let mut last_sent = String::new();
            let mut last_seq = h
                .state
                .lock()
                .map(|g| g.next_seq.saturating_sub(1))
                .unwrap_or(0);

            'conn: loop {
                let (next_snapshot, pending_events) = {
                    let Ok(mut g) = h.state.lock() else { break 'conn };
                    while (g.snapshot.is_empty() || g.snapshot == last_sent)
                        && g.events.back().is_none_or(|(s, _)| *s <= last_seq)
                    {
                        g = match h.cvar.wait(g) {
                            Ok(g) => g,
                            Err(_) => break 'conn,
                        };
                    }
                    let snap = if !g.snapshot.is_empty() && g.snapshot != last_sent {
                        Some(g.snapshot.clone())
                    } else {
                        None
                    };
                    let mut evs = Vec::new();
                    for (seq, line) in &g.events {
                        if *seq > last_seq {
                            evs.push((*seq, line.clone()));
                        }
                    }
                    (snap, evs)
                };
                if let Some(next) = next_snapshot {
                    if writeln!(file, "{next}").is_err() {
                        break;
                    }
                    last_sent = next;
                }
                for (seq, ev) in pending_events {
                    if writeln!(file, "{ev}").is_err() {
                        break 'conn;
                    }
                    last_seq = seq;
                }
                let _ = FlushFileBuffers(HANDLE(file.as_raw_handle()));
            }

            let _ = DisconnectNamedPipe(HANDLE(file.as_raw_handle()));
            drop(file);
        }
    }
}

/// Execute an IPC command on the WM main thread. Returns `true` if the WM should exit.
pub(crate) unsafe fn handle(app: &mut App, cmd: Cmd) -> bool {
    match cmd {
        Cmd::List { reply } => {
            let _ = reply.send(snapshot_json(app));
        }
        Cmd::Switch { index, focus, reply } => {
            if tray::PAUSED.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = reply.send("err Window manager is paused".to_string());
                return false;
            }
            if index >= app.ws.count() {
                let _ = reply.send(format!("err workspace '{}' does not exist", index + 1));
                return false;
            }
            if index != app.ws.active() {
                app.snap_reset(true);
                app.switch_workspace_with_focus(index, focus);
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::Move { hwnd, index, reply } => {
            if tray::PAUSED.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = reply.send("err Window manager is paused".to_string());
                return false;
            }
            if index >= app.ws.count() {
                let _ = reply.send(format!("err workspace '{}' does not exist", index + 1));
                return false;
            }
            let wid = WindowId(hwnd as isize as usize as u64);
            let Some(from) = app.ws.workspace_of(wid) else {
                let _ = reply.send("err Window is not managed by vscroll-wm".to_string());
                return false;
            };
            if from != index {
                app.ws.move_window_to(wid, index);
                app.apply();
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::Ensure { hwnd, reply } => {
            if !tray::PAUSED.load(std::sync::atomic::Ordering::Relaxed) {
                let wid = WindowId(hwnd as isize as usize as u64);
                let old = app.ws.active();
                if app.ws.focus_window(wid) {
                    app.slide = Some(Slide::switching(slide_dir(old, app.ws.active())));
                    app.update_workspace_tip();
                    app.apply();
                }
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::TogglePause { reply } => {
            app.toggle_pause();
            let _ = reply.send("ok".to_string());
        }
        Cmd::Reload { reply } => {
            app.reload_config();
            let _ = reply.send("ok".to_string());
        }
        Cmd::Reapply { reply } => {
            if !tray::PAUSED.load(std::sync::atomic::Ordering::Relaxed) {
                app.sync();
                app.apply();
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::Cheatsheet { reply } => {
            if cheatsheet::is_suppressed() {
                bump_cheatsheet_seq();
                app.publish_ipc();
            } else {
                cheatsheet::toggle(work_area(), &app.cfg);
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::CheatsheetData { reply } => {
            let _ = reply.send(cheatsheet::groups_json(&app.cfg));
        }
        Cmd::SetCheatsheetUi { enabled, reply } => {
            cheatsheet::set_suppressed(!enabled);
            let _ = reply.send("ok".to_string());
        }
        Cmd::OverviewData { reply } => {
            app.snap_reset(true);
            let _ = reply.send(overview_data_json(app));
        }
        Cmd::SetOverviewUi { enabled, reply } => {
            overview::set_suppressed(!enabled);
            let _ = reply.send("ok".to_string());
        }
        Cmd::OverviewDrop {
            src_hwnd,
            target_ws,
            kind,
            target_hwnd,
            first,
            reply,
        } => {
            if tray::PAUSED.load(Ordering::Relaxed) {
                let _ = reply.send("err Window manager is paused".to_string());
                return false;
            }
            if target_ws >= app.ws.count() {
                let _ = reply.send(format!("err workspace '{}' does not exist", target_ws + 1));
                return false;
            }
            let src = WindowId(src_hwnd as isize as usize as u64);
            let Some(from) = app.ws.workspace_of(src) else {
                let _ = reply.send("err Window is not managed by vscroll-wm".to_string());
                return false;
            };
            let dst_win = WindowId(target_hwnd as isize as usize as u64);
            let changed = match kind.as_str() {
                "empty" => {
                    if target_ws != from {
                        app.ws.move_window_to(src, target_ws)
                    } else {
                        false
                    }
                }
                "beside" | "before" | "after" | "split" => {
                    let at = match kind.as_str() {
                        "beside" => InsertPosition::Beside(dst_win),
                        "before" => InsertPosition::FullBefore(dst_win),
                        "after" => InsertPosition::FullAfter(dst_win),
                        _ => InsertPosition::Split(dst_win, first),
                    };
                    if target_ws != from {
                        app.ws.move_window_at(src, target_ws, at)
                    } else {
                        app.ws.reposition_window(src, at)
                    }
                }
                other => {
                    let _ = reply.send(format!("err unknown slot kind '{other}'"));
                    return false;
                }
            };
            if changed {
                app.apply();
                app.update_workspace_tip();
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::OverviewPick { hwnd, reply } => {
            if !tray::PAUSED.load(Ordering::Relaxed) {
                let target = WindowId(hwnd as isize as usize as u64);
                let old = app.ws.active();
                if app.ws.focus_window(target) {
                    app.slide = Some(Slide::switching(slide_dir(old, app.ws.active())));
                    app.update_workspace_tip();
                }
                app.apply();
                if let Some(&h) = app.map.get(&target) {
                    app.focus_hwnd(h);
                }
            }
            let _ = reply.send("ok".to_string());
        }
        Cmd::SetOsd { enabled, reply } => {
            osd::set_suppressed(!enabled);
            let _ = reply.send("ok".to_string());
        }
        Cmd::ReinstallHook { reply } => {
            crate::hook::reinstall_kb_hook();
            let _ = reply.send("ok".to_string());
        }
        Cmd::Focus { hwnd, reply } => {
            let h = HWND(hwnd as isize as *mut c_void);
            if !IsWindow(Some(h)).as_bool() {
                let _ = reply.send("err invalid window".to_string());
                return false;
            }
            if !tray::PAUSED.load(Ordering::Relaxed) {
                let target = WindowId(hwnd as isize as usize as u64);
                let old = app.ws.active();
                let switched = app.ws.focus_window(target);
                if switched {
                    app.slide = Some(Slide::switching(slide_dir(old, app.ws.active())));
                    app.update_workspace_tip();
                }
                if switched || app.map.contains_key(&target) {
                    app.apply();
                }
            }
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            app.focus_hwnd(h);
            let _ = reply.send("ok".to_string());
        }
        Cmd::Minimize { hwnd, reply } => {
            let h = HWND(hwnd as isize as *mut c_void);
            if !IsWindow(Some(h)).as_bool() {
                let _ = reply.send("err invalid window".to_string());
                return false;
            }
            let id = WindowId(hwnd as isize as usize as u64);
            if !tray::PAUSED.load(Ordering::Relaxed) && app.map.contains_key(&id) {
                app.ws.set_hidden(id, true);
                app.hidden.insert(id);
                app.anim.remove(&id);
                app.apply();
            }
            let _ = ShowWindow(h, SW_MINIMIZE);
            let _ = reply.send("ok".to_string());
        }
        Cmd::Close { hwnd, reply } => {
            let h = HWND(hwnd as isize as *mut c_void);
            let id = WindowId(hwnd as isize as usize as u64);
            let allowed = IsWindow(Some(h)).as_bool()
                && (app.map.contains_key(&id)
                    || app.floating.contains_key(&id)
                    || crate::winutil::manageable(h));
            if !allowed {
                let _ = reply.send("err window not eligible".to_string());
                return false;
            }
            let _ = PostMessageW(Some(h), WM_SYSCOMMAND, WPARAM(SC_CLOSE as usize), LPARAM(0));
            let _ = reply.send("ok".to_string());
        }
        Cmd::ShellRelay { alt_tab, winkey, reply } => {
            crate::hook::set_shell_relay(alt_tab, winkey);
            let _ = reply.send("ok".to_string());
        }
        Cmd::Quit { reply } => {
            let _ = reply.send("ok".to_string());
            return true;
        }
    }
    false
}
