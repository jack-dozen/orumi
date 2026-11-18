//! Hook keyboard tingkat rendah dan event hook Windows, beserta tabel aksi tombolnya.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU16, AtomicU32, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThread, GetCurrentThreadId, SetEvent, SetThreadPriority,
    THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bindings::{self, ALT, CTRL, SHIFT, WIN};
use crate::consts::{
    DIRS, HOTKEY_OVERVIEW, HOTKEY_QUIT, HOTKEY_WS_GO, HOTKEY_WS_NEXT, HOTKEY_WS_PREV, WM_ACTION,
    WM_CANCEL_START, WM_MASK_ALT, WM_REINSTALL_HOOK, WM_SNAP_CANCEL, WM_WINEVENT,
};
use crate::{ipc, overview, tray, winutil};

pub(crate) static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

pub(crate) static WIN_DOWN: AtomicBool = AtomicBool::new(false);
pub(crate) static FG_ELEVATED: AtomicBool = AtomicBool::new(false);

static RELAY_ALT_TAB: AtomicBool = AtomicBool::new(false);
static RELAY_WINKEY: AtomicBool = AtomicBool::new(false);
static ELEVATED_ALTTAB_ACTIVE: AtomicBool = AtomicBool::new(false);
static ELEVATED_ALTTAB_SWALLOWED: AtomicU32 = AtomicU32::new(0);
static ELEVATED_WIN_ACTIVE: AtomicBool = AtomicBool::new(false);
static ANY_WIN_HELD: AtomicBool = AtomicBool::new(false);
static ANY_WIN_COMBO: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_shell_relay(alt_tab: bool, winkey: bool) {
    RELAY_ALT_TAB.store(alt_tab, Ordering::Relaxed);
    RELAY_WINKEY.store(winkey, Ordering::Relaxed);
    if !alt_tab {
        ELEVATED_ALTTAB_ACTIVE.store(false, Ordering::Relaxed);
        ELEVATED_ALTTAB_SWALLOWED.store(0, Ordering::Relaxed);
    }
    if !winkey {
        ELEVATED_WIN_ACTIVE.store(false, Ordering::Relaxed);
        ANY_WIN_HELD.store(false, Ordering::Relaxed);
        ANY_WIN_COMBO.store(false, Ordering::Relaxed);
    }
}

/// Tombol untuk berputar antar window saat overview terbuka dan Win ditahan (kode
/// virtual-key; 0 = mati). Diturunkan dari ikatan `overview` (lihat bindings.rs).
pub(crate) static OVERVIEW_VK: AtomicU16 = AtomicU16::new(0x09);

/// Tombol pintasan yang sedang ditahan, untuk mengenali event berulang (auto-repeat).
pub(crate) static HELD_VK: AtomicU16 = AtomicU16::new(0);

/// Overview sedang terbuka: tiling dimatikan, Win+tombol hanya mengendalikan overview.
pub(crate) static OV_OPEN: AtomicBool = AtomicBool::new(false);

/// Daftar window snap layouts sedang tampil: Esc membatalkannya (tombolnya ditelan).
pub(crate) static SNAP_OPEN: AtomicBool = AtomicBool::new(false);

/// Pilihan sudah digeser sejak overview dibuka. Kalau Win dilepas dalam keadaan ini,
/// overview langsung loncat ke window yang dipilih (gaya Alt+Tab).
pub(crate) static OV_MOVED: AtomicBool = AtomicBool::new(false);

/// Tagged `dwExtraInfo` on our neutral key injection so ORUMI's Win-key detector knows a WM
/// shortcut consumed the combo even when `vscroll-wm` swallowed the second key.
const VSCROLL_COMBO_TAG: usize = 0x5653_4352;
static COMBO_EVENT: AtomicIsize = AtomicIsize::new(0);

unsafe fn notify_combo() {
    if RELAY_WINKEY.load(Ordering::Relaxed) && !ANY_WIN_COMBO.swap(true, Ordering::Relaxed) {
        ipc::publish_shell_event(r#"{"shell_event":"win_combo"}"#);
    }
    let mut h = COMBO_EVENT.load(Ordering::Relaxed);
    if h == 0 {
        if let Ok(ev) = CreateEventW(None, true, false, w!("Local\\vscroll-wm-combo")) {
            h = ev.0 as isize;
            if COMBO_EVENT
                .compare_exchange(0, h, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
            {
                h = COMBO_EVENT.load(Ordering::Relaxed);
            }
        }
    }
    if h != 0 {
        let _ = SetEvent(HANDLE(h as *mut _));
    }
}

/// Aksi tombol Win+... saat overview terbuka: (pesan, wParam, menggeser pilihan?).
/// `bound` = id hotkey yang terikat pada kombinasi ini menurut konfigurasi (bindings.rs).
/// None = bukan tombol overview; dibiarkan lewat ke Windows (tiling tidak ikut jalan).
pub(crate) fn overview_hook_action(
    vk: u16,
    shift: bool,
    alt: bool,
    ctrl: bool,
    overview_vk: u16,
    bound: Option<i32>,
) -> Option<(u32, usize, bool)> {
    if vk == VK_ESCAPE.0 && ctrl {
        return Some((WM_ACTION, HOTKEY_QUIT as usize, false)); // jalan keluar darurat
    }
    // Pintasan workspace (sesuai ikatan sekarang): sama seperti di luar overview,
    // tapi menggeser kursor
    match bound {
        Some(HOTKEY_WS_PREV) => return Some((overview::WM_OV_WS, 0, true)),
        Some(HOTKEY_WS_NEXT) => return Some((overview::WM_OV_WS, 1, true)),
        Some(id) if (HOTKEY_WS_GO..HOTKEY_WS_GO + 9).contains(&id) => {
            return Some((overview::WM_OV_WS, 2 + (id - HOTKEY_WS_GO) as usize, true));
        }
        _ => {}
    }
    if let Some(i) = arrow_index(vk).filter(|_| !ctrl) {
        return Some((overview::WM_OV_MOVE, i as usize, true));
    }
    if vk == VK_RETURN.0 {
        return Some((overview::WM_OV_ACCEPT, 0, false));
    }
    if vk == VK_ESCAPE.0 {
        return Some((overview::WM_OV_CANCEL, 0, false));
    }
    if overview_vk != 0 && vk == overview_vk && !alt && !ctrl {
        return Some((overview::WM_OV_CYCLE, if shift { 0 } else { 1 }, true));
    }
    if bound == Some(HOTKEY_OVERVIEW) {
        return Some((WM_ACTION, HOTKEY_OVERVIEW as usize, false)); // tutup
    }
    None
}

pub(crate) fn arrow_index(vk: u16) -> Option<i32> {
    DIRS.iter().position(|(k, _)| k.0 == vk).map(|i| i as i32)
}

/// Selipkan tombol dummy supaya Windows tidak membuka Start Menu saat Win
/// dilepas (arrow-nya kita telan, jadi Windows mengira Win ditekan sendirian).
pub(crate) unsafe fn cancel_start_menu() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0xE8),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: VSCROLL_COMBO_TAG,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

/// Modifier selain Win yang sedang ditahan (bitmask SHIFT / ALT / CTRL dari bindings.rs).
unsafe fn held_mods() -> u8 {
    let mut m = 0u8;
    if GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 {
        m |= SHIFT;
    }
    if GetAsyncKeyState(VK_MENU.0 as i32) < 0 {
        m |= ALT;
    }
    if GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 {
        m |= CTRL;
    }
    m
}

unsafe fn post_cancel_start_menu() {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        WM_CANCEL_START,
        WPARAM(0),
        LPARAM(0),
    );
}

unsafe fn post_mask_alt_menu() {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        WM_MASK_ALT,
        WPARAM(0),
        LPARAM(0),
    );
}

/// Inject a neutral key while Alt is held so an elevated app's menu bar does not activate
/// when we swallow Alt+Tab.
pub(crate) unsafe fn mask_alt_menu() {
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
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

fn alttab_swallow_bit(vk: u16) -> u32 {
    match VIRTUAL_KEY(vk) {
        VK_TAB => 1,
        VK_ESCAPE => 2,
        VK_LEFT => 4,
        VK_RIGHT => 8,
        VK_UP => 16,
        VK_DOWN => 32,
        _ => 0,
    }
}

/// Relay Alt+Tab and Win-tap to non-admin ORUMI when an elevated window holds foreground focus
/// (UIPI blocks ORUMI's own Medium-integrity hooks on elevated windows), and track Win combos
/// across elevated/normal focus transitions so ORUMI never misreads Win+Shift+Arrow as a Win tap.
unsafe fn handle_elevated_shell_relay(m: u32, k: &KBDLLHOOKSTRUCT) -> bool {
    let down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
    let up = m == WM_KEYUP || m == WM_SYSKEYUP;
    if !down && !up {
        return false;
    }
    let vk = k.vkCode as u16;
    let injected = (k.flags.0 & LLKHF_INJECTED.0) != 0;
    let is_win = vk == VK_LWIN.0 || vk == VK_RWIN.0;
    let is_alt = matches!(VIRTUAL_KEY(vk), VK_MENU | VK_LMENU | VK_RMENU);
    let fg_elevated = FG_ELEVATED.load(Ordering::Relaxed);

    if RELAY_WINKEY.load(Ordering::Relaxed) && !injected {
        if is_win {
            if down {
                if !ANY_WIN_HELD.swap(true, Ordering::Relaxed) {
                    ANY_WIN_COMBO.store(false, Ordering::Relaxed);
                    if winutil::is_elevated()
                        && fg_elevated
                        && !ELEVATED_WIN_ACTIVE.swap(true, Ordering::Relaxed)
                    {
                        post_cancel_start_menu();
                        ipc::publish_shell_event(r#"{"shell_event":"win_down"}"#);
                    }
                }
            } else if ANY_WIN_HELD.swap(false, Ordering::Relaxed) {
                let combo = ANY_WIN_COMBO.swap(false, Ordering::Relaxed);
                if ELEVATED_WIN_ACTIVE.swap(false, Ordering::Relaxed) {
                    ipc::publish_shell_event(&format!(
                        r#"{{"shell_event":"win_up","combo":{combo}}}"#
                    ));
                } else if combo {
                    ipc::publish_shell_event(r#"{"shell_event":"win_release"}"#);
                }
            }
        } else if down && ANY_WIN_HELD.load(Ordering::Relaxed) && vk != 0xE8 {
            if !ANY_WIN_COMBO.swap(true, Ordering::Relaxed) {
                ipc::publish_shell_event(r#"{"shell_event":"win_combo"}"#);
            }
        }
    }

    if !winutil::is_elevated() {
        return false;
    }

    if RELAY_ALT_TAB.load(Ordering::Relaxed) {
        if is_alt && up {
            ELEVATED_ALTTAB_SWALLOWED.store(0, Ordering::Relaxed);
            if ELEVATED_ALTTAB_ACTIVE.swap(false, Ordering::Relaxed) {
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_commit"}"#);
            }
            return false;
        }
        if injected {
            return false;
        }
        if up {
            let bit = alttab_swallow_bit(vk);
            if bit != 0 && (ELEVATED_ALTTAB_SWALLOWED.fetch_and(!bit, Ordering::Relaxed) & bit) != 0
            {
                return true;
            }
            return false;
        }

        let alt_down =
            (k.flags.0 & LLKHF_ALTDOWN.0) != 0 || GetAsyncKeyState(VK_MENU.0 as i32) < 0;
        let active = ELEVATED_ALTTAB_ACTIVE.load(Ordering::Relaxed);
        if active && !alt_down && !is_alt {
            ELEVATED_ALTTAB_ACTIVE.store(false, Ordering::Relaxed);
            ELEVATED_ALTTAB_SWALLOWED.store(0, Ordering::Relaxed);
            ipc::publish_shell_event(r#"{"shell_event":"alt_tab_commit"}"#);
            return false;
        }
        if !active && !fg_elevated {
            return false;
        }

        let held = held_mods();
        let ctrl = (held & CTRL) != 0;
        let shift = (held & SHIFT) != 0;
        let win_down =
            GetAsyncKeyState(VK_LWIN.0 as i32) < 0 || GetAsyncKeyState(VK_RWIN.0 as i32) < 0;

        let swallow = match VIRTUAL_KEY(vk) {
            VK_TAB if alt_down && !ctrl && !win_down => {
                if active {
                    ipc::publish_shell_event(&format!(
                        r#"{{"shell_event":"alt_tab_next","reverse":{shift}}}"#
                    ));
                } else {
                    ELEVATED_ALTTAB_ACTIVE.store(true, Ordering::Relaxed);
                    post_mask_alt_menu();
                    ipc::publish_shell_event(&format!(
                        r#"{{"shell_event":"alt_tab_start","reverse":{shift}}}"#
                    ));
                }
                true
            }
            VK_ESCAPE if active => {
                ELEVATED_ALTTAB_ACTIVE.store(false, Ordering::Relaxed);
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_cancel"}"#);
                true
            }
            VK_LEFT if active => {
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_step","delta":-1}"#);
                true
            }
            VK_RIGHT if active => {
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_step","delta":1}"#);
                true
            }
            VK_UP if active => {
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_row","delta":-1}"#);
                true
            }
            VK_DOWN if active => {
                ipc::publish_shell_event(r#"{"shell_event":"alt_tab_row","delta":1}"#);
                true
            }
            _ => false,
        };
        if swallow {
            let bit = alttab_swallow_bit(vk);
            if bit != 0 {
                ELEVATED_ALTTAB_SWALLOWED.fetch_or(bit, Ordering::Relaxed);
            }
            return true;
        }
    }
    false
}

pub(crate) fn start_kb_hook() {
    std::thread::Builder::new()
        .name("vscroll-kb-hook".into())
        .spawn(|| unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            HOOK_THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
            let mut hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), None, 0).ok();
            if hook.is_none() {
                eprintln!("gagal pasang keyboard hook (Win+Arrow tidak aktif)");
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_REINSTALL_HOOK {
                    if let Some(h) = hook.take() {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), None, 0).ok();
                } else {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if let Some(h) = hook {
                let _ = UnhookWindowsHookEx(h);
            }
        })
        .ok();
}

pub(crate) fn reinstall_kb_hook() {
    let tid = HOOK_THREAD.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_REINSTALL_HOOK, WPARAM(0), LPARAM(0));
        }
    }
}

/// Kirim aksi `id` ke thread utama dan telan tombolnya. Pengulangan karena tombol
/// ditahan diabaikan kecuali aksinya memang boleh berulang.
unsafe fn fire(vk: u16, id: i32, down: bool) -> LRESULT {
    if down {
        let repeat = HELD_VK.swap(vk, Ordering::Relaxed) == vk;
        if repeat && !bindings::repeat_allowed(id) {
            return LRESULT(1); // tombol ditahan: abaikan pengulangan
        }
        if id == HOTKEY_OVERVIEW
            && !crate::overview::is_suppressed()
            && !OV_OPEN.load(Ordering::Relaxed)
        {
            // optimistis: tombol berikutnya (mis. Tab kedua) harus sudah
            // dianggap "overview terbuka" sebelum thread utama selesai membukanya
            OV_OPEN.store(true, Ordering::Relaxed);
            OV_MOVED.store(false, Ordering::Relaxed);
        }
        notify_combo();
        post_cancel_start_menu();
        let _ = PostThreadMessageW(
            MAIN_THREAD.load(Ordering::Relaxed),
            WM_ACTION,
            WPARAM(id as usize),
            LPARAM(0),
        );
    }
    LRESULT(1) // telan tombolnya
}

/// Low-level keyboard hook. Harus cepat: kerjanya cuma menelan tombol dan
/// mengirim pesan ke thread utama, eksekusinya di message loop.
pub(crate) unsafe extern "system" fn kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if handle_elevated_shell_relay(wparam.0 as u32, k) {
            return LRESULT(1);
        }
    }
    if code == HC_ACTION as i32 && !tray::PAUSED.load(Ordering::Relaxed) {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if k.flags.0 & LLKHF_INJECTED.0 == 0 {
            let m = wparam.0 as u32;
            let down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
            let vk = k.vkCode as u16;
            if !down && HELD_VK.load(Ordering::Relaxed) == vk {
                HELD_VK.store(0, Ordering::Relaxed);
            }
            // Esc tanpa modifier menutup daftar window snap layouts
            if down
                && vk == VK_ESCAPE.0
                && SNAP_OPEN.load(Ordering::Relaxed)
                && GetAsyncKeyState(VK_LWIN.0 as i32) >= 0
                && GetAsyncKeyState(VK_RWIN.0 as i32) >= 0
                && held_mods() == 0
            {
                let _ = PostThreadMessageW(
                    MAIN_THREAD.load(Ordering::Relaxed),
                    WM_SNAP_CANCEL,
                    WPARAM(0),
                    LPARAM(0),
                );
                return LRESULT(1);
            }
            if vk == VK_LWIN.0 || vk == VK_RWIN.0 {
                WIN_DOWN.store(down, Ordering::Relaxed);
                if !down {
                    HELD_VK.store(0, Ordering::Relaxed);
                }
                // Gaya Alt+Tab: Win dilepas setelah pilihan digeser = loncat ke window itu
                if !down
                    && OV_OPEN.load(Ordering::Relaxed)
                    && OV_MOVED.swap(false, Ordering::Relaxed)
                {
                    let _ = PostThreadMessageW(
                        MAIN_THREAD.load(Ordering::Relaxed),
                        overview::WM_OV_ACCEPT,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            } else if GetAsyncKeyState(VK_LWIN.0 as i32) < 0
                || GetAsyncKeyState(VK_RWIN.0 as i32) < 0
            {
                WIN_DOWN.store(true, Ordering::Relaxed);
                let held = held_mods();
                let (shift, alt, ctrl) = (held & SHIFT != 0, held & ALT != 0, held & CTRL != 0);
                // aksi yang terikat pada kombinasi ini (tombol terkunci dulu, lalu konfigurasi)
                let bound = bindings::lookup_active(WIN | held, vk);
                if OV_OPEN.load(Ordering::Relaxed) {
                    // Overview terbuka: tiling mati. Panah menggeser pilihan, tombol overview
                    // berputar, Enter / Esc memilih / membatalkan. Selain itu dibiarkan ke Windows.
                    let ov_vk = OVERVIEW_VK.load(Ordering::Relaxed);
                    return match overview_hook_action(vk, shift, alt, ctrl, ov_vk, bound) {
                        Some((msg, wp, moves)) => {
                            if down {
                                let repeat = HELD_VK.swap(vk, Ordering::Relaxed) == vk;
                                if repeat && msg == WM_ACTION {
                                    return LRESULT(1); // toggle / keluar: sekali per tekan
                                }
                                if moves {
                                    OV_MOVED.store(true, Ordering::Relaxed);
                                }
                                let _ = PostThreadMessageW(
                                    MAIN_THREAD.load(Ordering::Relaxed),
                                    msg,
                                    WPARAM(wp),
                                    LPARAM(0),
                                );
                                notify_combo();
                                post_cancel_start_menu();
                            }
                            LRESULT(1) // telan tombolnya
                        }
                        None if bound.is_some_and(bindings::is_workspace_id) => {
                            // desktop virtual Windows dimatikan: jangan diteruskan
                            if down {
                                notify_combo();
                                post_cancel_start_menu();
                            }
                            LRESULT(1)
                        }
                        None => CallNextHookEx(None, code, wparam, lparam),
                    };
                }
                if let Some(id) = bound {
                    return fire(vk, id, down);
                }
            } else {
                // Pintasan tanpa Win (mis. Alt+H atau Ctrl+Alt+O): hanya kalau ada modifier
                // yang ditahan. Saat overview terbuka hanya tombol overview-nya sendiri
                // yang berlaku (menutupnya); sisanya untuk jendela overview.
                let held = held_mods();
                if held != 0 {
                    if let Some(id) = bindings::lookup_active(held, vk) {
                        if !OV_OPEN.load(Ordering::Relaxed) || id == HOTKEY_OVERVIEW {
                            return fire(vk, id, down);
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Callback event Windows. Dipanggil di thread utama lewat message loop;
/// kerjanya sengaja minimal: teruskan sebagai pesan, penyaringan di `on_event`.
pub(crate) unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.0.is_null() {
        return; // hanya window itu sendiri, bukan elemen di dalamnya
    }
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::Relaxed),
        WM_WINEVENT,
        WPARAM(event as usize),
        LPARAM(hwnd.0 as isize),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::Bindings;

    const TAB: u16 = 0x09;

    /// Kombinasi Win + modifier yang diberikan, lewat ikatan bawaan.
    fn act(vk: u16, shift: bool, alt: bool, ctrl: bool) -> Option<(u32, usize, bool)> {
        let mods = WIN
            | if shift { SHIFT } else { 0 }
            | if alt { ALT } else { 0 }
            | if ctrl { CTRL } else { 0 };
        let bound = Bindings::defaults().lookup(mods, vk);
        overview_hook_action(vk, shift, alt, ctrl, TAB, bound)
    }

    #[test]
    fn arrows_move_the_overview_cursor_instead_of_tiling() {
        assert_eq!(
            act(VK_LEFT.0, false, false, false),
            Some((overview::WM_OV_MOVE, 0, true))
        );
        assert_eq!(
            act(VK_RIGHT.0, false, false, false),
            Some((overview::WM_OV_MOVE, 1, true))
        );
        assert_eq!(
            act(VK_UP.0, true, false, false),
            Some((overview::WM_OV_MOVE, 2, true))
        );
        assert_eq!(
            act(VK_DOWN.0, false, true, false),
            Some((overview::WM_OV_MOVE, 3, true))
        );
        // Win+Ctrl+Kiri/Kanan / angka = lompat workspace (kursor)
        assert_eq!(act(VK_LEFT.0, false, false, true), Some((overview::WM_OV_WS, 0, true)));
        assert_eq!(act(VK_RIGHT.0, false, false, true), Some((overview::WM_OV_WS, 1, true)));
        assert_eq!(act(0x31, false, false, true), Some((overview::WM_OV_WS, 2, true)));
        assert_eq!(act(0x39, false, false, true), Some((overview::WM_OV_WS, 10, true)));
        assert_eq!(act(VK_LEFT.0, true, false, true), None); // kirim window dari overview: belum ada
    }

    #[test]
    fn workspace_keys_in_the_overview_follow_custom_bindings() {
        // workspace_next dipindah ke Win+Ctrl+L: di overview ikut menggeser kursor
        let mut b = Bindings::defaults();
        let next = bindings::find_action("workspace_next").unwrap();
        b.set(next, vec![bindings::Chord::new(WIN | CTRL, 0x4C)]);
        let bound = b.lookup(WIN | CTRL, 0x4C);
        assert_eq!(
            overview_hook_action(0x4C, false, false, true, TAB, bound),
            Some((overview::WM_OV_WS, 1, true))
        );
        // tombol lamanya tidak lagi berarti workspace berikutnya
        let old = b.lookup(WIN | CTRL, VK_RIGHT.0);
        assert_eq!(overview_hook_action(VK_RIGHT.0, false, false, true, TAB, old), None);
    }

    #[test]
    fn tab_cycles_forward_and_shift_tab_backward() {
        assert_eq!(
            act(TAB, false, false, false),
            Some((overview::WM_OV_CYCLE, 1, true))
        );
        assert_eq!(
            act(TAB, true, false, false),
            Some((overview::WM_OV_CYCLE, 0, true))
        );
        // overview_key off: tidak ada tombol putar
        assert_eq!(overview_hook_action(TAB, false, false, false, 0, None), None);
    }

    #[test]
    fn enter_escape_and_toggle_keys() {
        assert_eq!(
            act(VK_RETURN.0, false, false, false),
            Some((overview::WM_OV_ACCEPT, 0, false))
        );
        assert_eq!(
            act(VK_ESCAPE.0, false, false, false),
            Some((overview::WM_OV_CANCEL, 0, false))
        );
        assert_eq!(
            act(VK_SPACE.0, false, true, false),
            Some((WM_ACTION, HOTKEY_OVERVIEW as usize, false))
        );
    }

    #[test]
    fn emergency_quit_still_works_and_other_wm_keys_pass_through() {
        assert_eq!(
            act(VK_ESCAPE.0, false, false, true),
            Some((WM_ACTION, HOTKEY_QUIT as usize, false))
        );
        assert_eq!(act(VK_Q.0, false, false, false), None); // tutup window: mati
        assert_eq!(act(VK_M.0, false, false, false), None); // minimize: mati
        assert_eq!(act(VK_F.0, false, true, false), None); // float: mati
    }
}
