//! "Win key by itself" trigger.
//!
//! The primary source is NOT the hook, but a poller that reads key states from Windows
//! (GetAsyncKeyState) every ~8 ms. The low-level keyboard hook can fail to receive keys for
//! hundreds of milliseconds or even seconds after Activities opens/closes, causing the Start
//! menu masking to fail during repeated Win presses.
//!
//! - Always pass all Win events and combinations through unchanged, so Win+E, Win+Arrow,
//!   etc. keep working and remain visible to other programs (e.g. the window manager).
//! - Prevent the Start menu through "masking": as soon as Win is pressed, inject a neutral key
//!   (VK 0xE8) WHILE Win remains held. Windows then treats Win as part of a combination and
//!   does not open Start when Win is released.
//! - If Activities is open when Win is pressed, hide it immediately on Win-down. Releasing Win
//!   without another key/click in between is a tap: toggle Activities.
//! - Keep the hook as a supplement: it injects masking immediately (if running) and catches
//!   other keys pressed very briefly while Win is held.
//!
//! Limitation: if the focused window is running as administrator, Windows may not report key
//! states to non-admin programs; run ORUMI as administrator if needed.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Sender},
        OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
            GetWindowTextW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
            KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
        },
    },
};

const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
const LLKHF_INJECTED: u32 = 0x10;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// Neutral key for masking (unused by Windows; same as in altswitch.rs).
const VK_NEUTRAL: u32 = 0xE8;
/// Polling interval.
const POLL_MS: u64 = 8;
/// Coalesce masking from the hook and poller for the same Win-down.
const MASK_DEDUPE_MS: u32 = 60;

/// true = print diagnostic logs to stderr (see the `npm run tauri dev` console).
const DEBUG: bool = true;

static ENABLED: AtomicBool = AtomicBool::new(true);
/// The hook saw another key (or another event) pressed while Win was held. Cleared by the poller
/// each time a Win press ends.
static COMBO: AtomicBool = AtomicBool::new(false);
/// Time (ms since start) of the last masking event.
static LAST_MASK: AtomicU32 = AtomicU32::new(0);
static T0: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u32 {
    T0.get_or_init(Instant::now).elapsed().as_millis() as u32
}

enum Msg {
    /// Win pressed by itself: toggle Activities.
    Tap,
    /// Win was just pressed: inject a neutral key so the Start menu does not open.
    Mask,
    /// Win pressed while Activities is open: close it immediately.
    Dismiss,
    /// Log line (printed by the worker; I/O inside the hook callback can cause a timeout).
    Log(String),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(m: Msg) -> bool {
    TX.get().map(|tx| tx.send(m).is_ok()).unwrap_or(false)
}

fn log(line: String) {
    if DEBUG {
        send(Msg::Log(line));
    }
}

/// Physical key state according to Windows. Inside the hook callback, this is the state
/// BEFORE the event being processed.
unsafe fn async_down(vk: u32) -> bool {
    (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

// ---------- Hook (supplement) ----------

unsafe fn handle(msg: usize, kb: &KBDLLHOOKSTRUCT) {
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !(down || up) || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // Ignore injected events (including our own and those from altswitch).
    if kb.flags.0 & LLKHF_INJECTED != 0 {
        return;
    }

    let vk = kb.vkCode;
    if vk == VK_LWIN || vk == VK_RWIN {
        // A new press (not auto-repeat) means Win was not already held according to Windows.
        if down && !async_down(vk) {
            send(Msg::Mask);
            log(format!("t={} hook: win-down vk={vk:#x}", kb.time));
        }
        return;
    }
    if down && (async_down(VK_LWIN) || async_down(VK_RWIN)) && !COMBO.swap(true, Ordering::Relaxed)
    {
        log(format!("t={} hook: tombol lain vk={vk:#x} saat Win ditahan", kb.time));
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        handle(wparam.0, &*(lparam.0 as *const KBDLLHOOKSTRUCT));
    }
    CallNextHookEx(None, code, wparam, lparam) // never swallow the event
}

// ---------- Poller (primary source) ----------

fn poller() {
    unsafe {
        // Must keep running even when the CPU is busy (e.g. while Activities captures thumbnails).
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let mut was = false;
    let mut dismissed = false;
    let mut polled_combo = false;
    let mut base = [false; 256];

    loop {
        thread::sleep(Duration::from_millis(POLL_MS));
        if !ENABLED.load(Ordering::Relaxed) {
            was = false;
            continue;
        }
        let win = unsafe { async_down(VK_LWIN) || async_down(VK_RWIN) };

        if win && !was {
            // New Win press. Record any keys/mouse buttons that were already held.
            for vk in 1..256u32 {
                base[vk as usize] = unsafe { async_down(vk) };
            }
            polled_combo = false;
            dismissed = super::is_open();
            send(Msg::Mask);
            if dismissed {
                send(Msg::Dismiss);
            }
            log(format!("pemantau: win-down, activities_terbuka={dismissed}"));
        } else if win && !polled_combo {
            // While Win is held, a new key press or mouse click means a combination.
            for vk in 1..256u32 {
                if vk == VK_LWIN || vk == VK_RWIN || vk == VK_NEUTRAL {
                    continue;
                }
                let d = unsafe { async_down(vk) };
                if d && !base[vk as usize] {
                    polled_combo = true;
                    log(format!("pemantau: tombol/klik lain vk={vk:#x} saat Win ditahan"));
                    break;
                }
                if !d {
                    base[vk as usize] = false;
                }
            }
        } else if !win && was {
            let combo = COMBO.swap(false, Ordering::Relaxed) || polled_combo;
            let tap = !combo && !dismissed;
            log(format!(
                "pemantau: win-up, kombinasi={combo}, ditutup_saat_down={dismissed} -> {}",
                if tap { "TAP (toggle)" } else { "tidak ada aksi" }
            ));
            if tap {
                send(Msg::Tap);
            }
        }
        was = win;
    }
}

// ---------- Masking, logging, worker ----------

/// Inject a neutral key (down, then up) while Win is still held.
unsafe fn inject_mask() {
    let t = now_ms();
    let last = LAST_MASK.load(Ordering::Relaxed);
    if last != 0 && t.wrapping_sub(last) < MASK_DEDUPE_MS {
        return;
    }
    LAST_MASK.store(t.max(1), Ordering::Relaxed);

    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_NEUTRAL as u16),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    let sent = SendInput(&inputs, size_of::<INPUT>() as i32);
    if DEBUG {
        log(format!("penyamaran: SendInput mengirim {sent} dari 2 event"));
    }
}

unsafe fn foreground_desc() -> String {
    let h = GetForegroundWindow();
    let mut class = [0u16; 128];
    let mut title = [0u16; 128];
    let c = (GetClassNameW(h, &mut class).max(0)) as usize;
    let t = (GetWindowTextW(h, &mut title).max(0)) as usize;
    format!(
        "{} | {}",
        String::from_utf16_lossy(&class[..c]),
        String::from_utf16_lossy(&title[..t])
    )
}

/// Install the hook and start the poller and worker thread.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let _ = TX.set(tx);
    let _ = T0.get_or_init(Instant::now);

    thread::spawn(move || {
        for msg in rx {
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match msg {
                Msg::Log(line) => eprintln!("[winkey] {line}"),
                Msg::Mask => unsafe { inject_mask() },
                Msg::Dismiss => super::dismiss(&app),
                Msg::Tap => {
                    super::toggle(&app);
                    if DEBUG {
                        // If the Start menu slips through, the foreground window after the tap belongs to it.
                        thread::spawn(|| {
                            thread::sleep(Duration::from_millis(450));
                            eprintln!("[winkey] window depan 450 ms sesudah ketukan: {}", unsafe {
                                foreground_desc()
                            });
                        });
                    }
                }
            }));
            if done.is_err() {
                eprintln!("[winkey] pekerja panic pada satu pesan; dilanjutkan");
            }
        }
    });

    thread::spawn(poller);

    thread::spawn(|| unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) else {
            eprintln!("[activities] gagal memasang hook (pemantau tetap berjalan)");
            return;
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    });
}
