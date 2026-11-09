//! Pengganti Alt+Tab. Hook keyboard tingkat rendah hanya memutuskan "telan atau teruskan"
//! dan mengirim pesan ke thread pekerja; semua kerja berat ada di thread pekerja, karena
//! Windows mencabut hook yang lambat. Ada penjaga yang memasang ulang hook kalau tidak
//! lagi menerima tombol.
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

// ---------- Konstanta ----------

const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
/// Pesan ke thread hook: pasang ulang hook (WM_APP + 1).
const WM_REINSTALL: u32 = 0x8001;

const LLKHF_INJECTED: u32 = 0x10;
/// Windows menandai event keyboard yang terjadi saat Alt sedang ditekan dengan bit ini.
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

/// Panel baru muncul kalau Alt+Tab ditahan selama ini; ketukan cepat tidak menampilkan panel.
const SHOW_DELAY: Duration = Duration::from_millis(150);
/// Setelah ini, kalau Alt ternyata sudah tidak ditekan, sesi dianggap selesai.
const ALT_POLL_GRACE: Duration = Duration::from_millis(300);
/// Daftar window yang lebih lambat dari ini dicatat di terminal.
const SLOW_LIST_MS: u128 = 40;
/// Jarak minimum antar pemasangan ulang hook oleh penjaga.
const REINSTALL_COOLDOWN_MS: u64 = 5000;

// ---------- Status bersama (hanya atomik; aman dipakai dari hook) ----------

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Sesi Alt+Tab sedang berjalan.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Tombol yang keydown-nya kita telan; keyup-nya harus ikut ditelan.
static SWALLOWED: AtomicU8 = AtomicU8::new(0);
/// Kapan hook terakhir melihat keydown Tab (ms sejak START). Dipakai penjaga.
static LAST_TAB_SEEN: AtomicU64 = AtomicU64::new(0);
/// Id thread yang memegang hook (0 = belum jalan).
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Status tombol langsung dari Windows. Lebih tepercaya daripada menghitung sendiri,
/// karena event pelepasan yang terlewat tidak bisa membuat statusnya tersangkut.
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

/// Minta thread hook memasang ulang hook-nya.
fn reinstall() {
    let tid = HOOK_THREAD.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }
}

/// Nyalakan/matikan penggantian Alt+Tab. Mematikan juga membatalkan sesi yang sedang berjalan;
/// menyalakan lagi memasang ulang hook (jaga-jaga kalau Windows sudah mencabutnya).
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

/// Keydown yang lolos berarti penanda lama sudah basi: hapus, supaya pelepasan tombol
/// berikutnya tidak ikut tertelan.
fn clear_swallowed(vk: u32) {
    let bit = swallow_bit(vk);
    if bit != 0 {
        SWALLOWED.fetch_and(!bit, Ordering::Relaxed);
    }
}

/// true kalau keydown tombol ini kita telan (dan tandanya dihapus).
fn take_swallowed(vk: u32) -> bool {
    let bit = swallow_bit(vk);
    bit != 0 && SWALLOWED.fetch_and(!bit, Ordering::Relaxed) & bit != 0
}

/// Suntikkan satu tombol netral (VK 0xE8, tidak dipakai Windows). Tanpa ini, aplikasi aktif
/// melihat "Alt ditekan lalu dilepas" (karena Tab kita telan) dan memunculkan menu bar-nya.
/// Dipanggil dari thread pekerja, BUKAN dari callback hook: event suntikan melewati semua hook
/// program lain, dan kalau itu terjadi di dalam callback, hook kita bisa dicabut Windows.
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

/// true = telan event ini (jangan teruskan ke aplikasi). Harus cepat dan tanpa panggilan berat.
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

    // Alt dilepas: sesi selesai. Event Alt-up sendiri selalu diteruskan.
    if is_alt_key && up {
        if ACTIVE.swap(false, Ordering::Relaxed) {
            send(Msg::Commit);
        }
        return false;
    }
    if injected {
        return false; // termasuk tombol netral milik kita sendiri
    }

    // Lepas tombol yang keydown-nya sudah kita telan.
    if up {
        return take_swallowed(vk);
    }

    let active = ACTIVE.load(Ordering::Relaxed);

    // Tombol apa pun tanpa Alt saat sesi aktif berarti Alt-up terlewat: akhiri sesi,
    // lalu teruskan tombolnya apa adanya.
    if active && !alt_flag && !is_alt_key {
        ACTIVE.store(false, Ordering::Relaxed);
        send(Msg::Commit);
        return false;
    }

    let swallow = match vk {
        // Hanya kalau Windows sendiri menandai Alt sedang ditekan, dan tanpa Ctrl/Win
        // (Ctrl+Alt+Tab bawaan dibiarkan). Status Ctrl/Win/Shift dibaca langsung dari Windows.
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

// ---------- Thread pekerja ----------

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
    // Isi dikirim dulu, baru panel ditampilkan, supaya tidak ada kedipan panel kosong.
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
        show(app, s); // pengguna sedang berputar-putar: tampilkan sekarang
    }
}

/// Akhiri sesi: sembunyikan panel dan pindah ke window yang dipilih.
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
            // Pertama-tama tahan menu bar aplikasi aktif, selagi Alt masih ditekan.
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
            // Item pertama adalah window yang sedang aktif, jadi pilihan awal window berikutnya.
            // Kalau window aktif tidak ada di daftar (mis. desktop), mulai dari yang pertama.
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
        // Tanpa sesi: tidur sampai ada pesan (tidak ada polling saat idle).
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
            // Alt-up terlewat (hook dibuang, desktop aman, dst.): anggap selesai.
            ACTIVE.store(false, Ordering::Relaxed);
            finish(&app, &mut session);
        } else if !s.shown && age >= SHOW_DELAY {
            show(&app, s);
        }
    }
}

/// Penjaga: kalau Alt+Tab ditekan secara fisik tapi hook tidak pernah melihat Tab, hook
/// kemungkinan sudah dicabut Windows (atau window aktif berjalan sebagai administrator).
/// Pasang ulang, paling sering sekali per REINSTALL_COOLDOWN_MS.
fn watchdog() {
    let mut last_reinstall: u64 = 0;
    loop {
        thread::sleep(Duration::from_millis(150));
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

/// Klik item di panel: pilih dan aktifkan window itu.
#[tauri::command]
pub fn switcher_pick(index: usize) {
    send(Msg::Pick(index));
}

/// Pasang hook dan jalankan thread pekerja serta penjaga.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let _ = TX.set(tx);
    thread::spawn(move || worker(app, rx));
    thread::spawn(watchdog);

    thread::spawn(|| unsafe {
        // Thread hook harus segera dijadwalkan; kalau terlambat, callback melewati batas waktu
        // dan Windows mencabut hook-nya.
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
