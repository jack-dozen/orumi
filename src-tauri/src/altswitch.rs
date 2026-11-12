//! Alt+Tab replacement. The low-level keyboard hook only decides whether to "swallow or pass through"
//! and sends messages to the worker thread; all heavy work runs there because Windows removes slow
//! hooks. A watchdog reinstalls the hook if it stops receiving keystrokes.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::{
        GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW,
            SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
            MSG, WH_KEYBOARD_LL,
        },
    },
};

use crate::{
    edge::Panels,
    winlist::{self, WindowEntry},
};

// ---------- Constants ----------

const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
/// Message to the hook thread: reinstall the hook (WM_APP + 1).
const WM_REINSTALL: u32 = 0x8001;

const LLKHF_INJECTED: u32 = 0x10;
/// Windows marks keyboard events that occur while Alt is held with this bit.
const LLKHF_ALTDOWN: u32 = 0x20;

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

/// The panel appears only if Alt+Tab is held this long; a quick tap does not show it.
const SHOW_DELAY: Duration = Duration::from_millis(150);
/// After this, if Alt is no longer held, the session is considered finished.
const ALT_POLL_GRACE: Duration = Duration::from_millis(300);
/// Windows taking longer than this are logged in the terminal.
const SLOW_LIST_MS: u128 = 40;
/// Minimum interval between hook reinstalls by the watchdog.
const REINSTALL_COOLDOWN_MS: u64 = 5000;

// ---------- Shared state (atomics only; safe to use from the hook) ----------

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether an Alt+Tab session is in progress.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Keys whose keydown we swallowed; their keyup must also be swallowed.
static SWALLOWED: AtomicU8 = AtomicU8::new(0);
/// When the hook last saw a Tab keydown (ms since START). Used by the watchdog.
static LAST_TAB_SEEN: AtomicU64 = AtomicU64::new(0);
/// ID of the thread holding the hook (0 = not running yet).
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Key state read directly from Windows. More reliable than tracking it ourselves,
/// because a missed key-up event cannot leave the state stuck.
fn async_down(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0 }
}

enum Msg {
    Start { reverse: bool },
    Next { reverse: bool },
    Step(i32),
    Cancel,
    Commit,
    Pick(usize),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(m: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(m);
    }
}

/// Ask the hook thread to reinstall its hook.
fn reinstall() {
    let tid = HOOK_THREAD.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }
}

/// Enable/disable the Alt+Tab replacement. Disabling also cancels any active session;
/// enabling reinstalls the hook in case Windows has removed it.
pub fn set_enabled(on: bool) {
    let was = ENABLED.swap(on, Ordering::Relaxed);
    if !on {
        ACTIVE.store(false, Ordering::Relaxed);
        send(Msg::Cancel);
    } else if !was {
        reinstall();
    }
}

// ---------- Hook ----------

fn swallow_bit(vk: u32) -> u8 {
    match vk {
        VK_TAB => 1,
        VK_ESCAPE => 2,
        VK_LEFT => 4,
        VK_RIGHT => 8,
        VK_UP => 16,
        VK_DOWN => 32,
        _ => 0,
    }
}

fn mark_swallowed(vk: u32) {
    SWALLOWED.fetch_or(swallow_bit(vk), Ordering::Relaxed);
}

/// A keydown that gets through means the old marker is stale: remove it so the next
/// key-up is not swallowed too.
fn clear_swallowed(vk: u32) {
    let bit = swallow_bit(vk);
    if bit != 0 {
        SWALLOWED.fetch_and(!bit, Ordering::Relaxed);
    }
}

/// true if we swallowed this key's keydown (and removed its marker).
fn take_swallowed(vk: u32) -> bool {
    let bit = swallow_bit(vk);
    bit != 0 && SWALLOWED.fetch_and(!bit, Ordering::Relaxed) & bit != 0
}

/// Inject one neutral key (VK 0xE8, unused by Windows). Without it, the active app
/// sees "Alt pressed and then released" (because we swallowed Tab) and opens its menu bar.
/// Called from the worker thread, NOT from the hook callback: injected events pass through all
/// other program hooks, and doing this inside the callback could cause Windows to remove our hook.
unsafe fn mask_alt_menu() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0xE8),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    SendInput(&inputs, size_of::<INPUT>() as i32);
}

/// true = swallow this event (do not pass it to the app). Must be fast and avoid heavy calls.
unsafe fn handle(msg: usize, kb: &KBDLLHOOKSTRUCT) -> bool {
    let vk = kb.vkCode;
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !down && !up {
        return false;
    }

    if vk == VK_TAB && down {
        LAST_TAB_SEEN.store(now_ms(), Ordering::Relaxed);
    }
    if !ENABLED.load(Ordering::Relaxed) {
        return false;
    }

    let injected = kb.flags.0 & LLKHF_INJECTED != 0;
    let alt_flag = kb.flags.0 & LLKHF_ALTDOWN != 0;
    let is_alt_key = matches!(vk, VK_MENU | VK_LMENU | VK_RMENU);

    // Alt released: session ends. Always pass the Alt-up event through.
    if is_alt_key && up {
        if ACTIVE.swap(false, Ordering::Relaxed) {
            send(Msg::Commit);
        }
        return false;
    }
    if injected {
        return false; // including our own neutral key
    }

    // Release keys whose keydown we already swallowed.
    if up {
        return take_swallowed(vk);
    }

    let active = ACTIVE.load(Ordering::Relaxed);

    // Any key without Alt during an active session means Alt-up was missed: end the session,
    // then pass the key through unchanged.
    if active && !alt_flag && !is_alt_key {
        ACTIVE.store(false, Ordering::Relaxed);
        send(Msg::Commit);
        return false;
    }

    let swallow = match vk {
        // Only when Windows itself reports Alt as held, and without Ctrl/Win
        // (leave built-in Ctrl+Alt+Tab alone). Read Ctrl/Win/Shift state directly from Windows.
        VK_TAB
            if alt_flag
                && !async_down(VK_CONTROL)
                && !async_down(VK_LWIN)
                && !async_down(VK_RWIN) =>
        {
            let reverse = async_down(VK_SHIFT);
            if active {
                send(Msg::Next { reverse });
            } else {
                ACTIVE.store(true, Ordering::Relaxed);
                send(Msg::Start { reverse });
            }
            true
        }
        VK_ESCAPE if active => {
            ACTIVE.store(false, Ordering::Relaxed);
            send(Msg::Cancel);
            true
        }
        VK_LEFT | VK_UP if active => {
            send(Msg::Step(-1));
            true
        }
        VK_RIGHT | VK_DOWN if active => {
            send(Msg::Step(1));
            true
        }
        _ => false,
    };

    if swallow {
        mark_swallowed(vk);
    } else {
        clear_swallowed(vk);
    }
    swallow
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && handle(wparam.0, &*(lparam.0 as *const KBDLLHOOKSTRUCT)) {
        return LRESULT(1);
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn install_hook() -> Option<HHOOK> {
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0).ok() }
}

// ---------- Worker thread ----------

struct Session {
    items: Vec<WindowEntry>,
    index: usize,
    shown: bool,
    started: Instant,
}

#[derive(Clone, Serialize)]
struct ItemsPayload<'a> {
    items: &'a [WindowEntry],
    index: usize,
}

fn show(app: &AppHandle, s: &mut Session) {
    s.shown = true;
    // Send the contents before showing the panel to avoid flashing an empty panel.
    let _ = app.emit(
        "switcher-items",
        ItemsPayload { items: &s.items, index: s.index },
    );
    app.state::<Panels>().set("switcher", true);
}

fn step(app: &AppHandle, session: &mut Option<Session>, delta: i32) {
    let Some(s) = session.as_mut() else { return };
    let n = s.items.len() as i32;
    if n == 0 {
        return;
    }
    s.index = (((s.index as i32 + delta) % n + n) % n) as usize;
    if s.shown {
        let _ = app.emit("switcher-select", s.index);
    } else {
        show(app, s); // user is cycling through items: show it now
    }
}

/// End the session: hide the panel and switch to the selected window.
fn finish(app: &AppHandle, session: &mut Option<Session>) {
    let Some(s) = session.take() else { return };
    if s.shown {
        app.state::<Panels>().set("switcher", false);
    }
    if let Some(w) = s.items.get(s.index) {
        winlist::activate_window(w.hwnd);
    }
}

fn cancel(app: &AppHandle, session: &mut Option<Session>) {
    if let Some(s) = session.take() {
        if s.shown {
            app.state::<Panels>().set("switcher", false);
        }
    }
}

fn handle_msg(app: &AppHandle, session: &mut Option<Session>, msg: Msg) {
    match msg {
        Msg::Start { reverse } => {
            // First, suppress the active app's menu bar while Alt is still held.
            unsafe { mask_alt_menu() };

            let t = Instant::now();
            let items = winlist::collect_windows();
            let ms = t.elapsed().as_millis();
            if ms >= SLOW_LIST_MS {
                eprintln!("[alt+tab] daftar window lambat: {} item dalam {ms} ms", items.len());
            }
            if items.is_empty() {
                ACTIVE.store(false, Ordering::Relaxed);
                *session = None;
                return;
            }
            let n = items.len();
            // The first item is the active window, so the initial selection is the next window.
            // If the active window is not in the list (e.g. the desktop), start with the first item.
            let base = if items[0].focused { 1 } else { 0 };
            let index = if n == 1 {
                0
            } else if reverse {
                n - 1
            } else {
                base.min(n - 1)
            };
            *session = Some(Session { items, index, shown: false, started: Instant::now() });
        }
        Msg::Next { reverse } => step(app, session, if reverse { -1 } else { 1 }),
        Msg::Step(d) => step(app, session, d),
        Msg::Cancel => cancel(app, session),
        Msg::Commit => finish(app, session),
        Msg::Pick(i) => {
            ACTIVE.store(false, Ordering::Relaxed);
            if let Some(s) = session.as_mut() {
                s.index = i.min(s.items.len().saturating_sub(1));
            }
            finish(app, session);
        }
    }
}

fn worker(app: AppHandle, rx: Receiver<Msg>) {
    let mut session: Option<Session> = None;
    loop {
        // No session: sleep until a message arrives (no polling while idle).
        let msg = if session.is_some() {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        if let Some(m) = msg {
            handle_msg(&app, &mut session, m);
        }

        let Some(s) = session.as_mut() else { continue };
        let age = s.started.elapsed();
        if age >= ALT_POLL_GRACE && !async_down(VK_MENU) {
            // Alt-up was missed (hook removed, desktop is safe, etc.): consider the session finished.
            ACTIVE.store(false, Ordering::Relaxed);
            finish(&app, &mut session);
        } else if !s.shown && age >= SHOW_DELAY {
            show(&app, s);
        }
    }
}

/// Watchdog: if Alt+Tab is physically pressed but the hook never sees Tab, Windows may have
/// removed the hook (or the active window may be running as administrator).
/// Reinstall it no more than once per REINSTALL_COOLDOWN_MS.
fn watchdog() {
    let mut last_reinstall: u64 = 0;
    loop {
        thread::sleep(Duration::from_millis(300));
        if !ENABLED.load(Ordering::Relaxed) {
            continue;
        }
        if !(async_down(VK_MENU) && async_down(VK_TAB)) {
            continue;
        }
        let now = now_ms();
        let tab_age = now.saturating_sub(LAST_TAB_SEEN.load(Ordering::Relaxed));
        if tab_age > 500 && now.saturating_sub(last_reinstall) > REINSTALL_COOLDOWN_MS {
            last_reinstall = now;
            eprintln!(
                "[alt+tab] hook tidak menerima Tab (dicabut Windows, atau window aktif berjalan \
                 sebagai administrator); dipasang ulang"
            );
            reinstall();
        }
    }
}

/// Click a panel item to select and activate that window.
#[tauri::command]
pub fn switcher_pick(index: usize) {
    send(Msg::Pick(index));
}

/// Install the hook and start the worker and watchdog threads.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let _ = TX.set(tx);
    thread::spawn(move || worker(app, rx));
    thread::spawn(watchdog);

    thread::spawn(|| unsafe {
        // The hook thread must be scheduled immediately; if delayed, the callback times out
        // and Windows removes the hook.
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        HOOK_THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);

        let mut hook = install_hook();
        if hook.is_none() {
            eprintln!("[alt+tab] gagal memasang hook (Alt+Tab bawaan tetap aktif)");
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_REINSTALL {
                if let Some(h) = hook.take() {
                    let _ = UnhookWindowsHookEx(h);
                }
                hook = install_hook();
                eprintln!(
                    "[alt+tab] hook dipasang ulang: {}",
                    if hook.is_some() { "ok" } else { "gagal" }
                );
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
