//! Konstanta: id hotkey, pesan internal, dan durasi.

use layout::Dir;

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const DIRS: [(VIRTUAL_KEY, Dir); 4] = [
    (VK_LEFT, Dir::Left),
    (VK_RIGHT, Dir::Right),
    (VK_UP, Dir::Up),
    (VK_DOWN, Dir::Down),
];

pub(crate) const HOTKEY_MAXIMIZE: i32 = 9;

pub(crate) const HOTKEY_QUIT: i32 = 14;

pub(crate) const HOTKEY_FLOAT: i32 = 15;

pub(crate) const HOTKEY_INSPECT: i32 = 16;

pub(crate) const HOTKEY_MINIMIZE: i32 = 17;

pub(crate) const HOTKEY_CLOSE: i32 = 18;

pub(crate) const HOTKEY_OVERVIEW: i32 = 19;

pub(crate) const HOTKEY_SPLIT_RIGHT: i32 = 20;

pub(crate) const HOTKEY_SPLIT_LEFT: i32 = 21;

pub(crate) const HOTKEY_SPLIT_RESET: i32 = 22;

pub(crate) const HOTKEY_CHEATSHEET: i32 = 23;

/// Pesan dari callback event Windows ke thread utama.
pub(crate) const WM_WINEVENT: u32 = WM_APP + 3;

/// Jeda (ms) mengumpulkan rentetan event sebelum menata ulang.
pub(crate) const EVENT_DEBOUNCE_MS: u32 = 40;

/// Cek ulang setelah window baru ditata (sebagian app mengubah ukurannya sendiri
/// beberapa saat setelah muncul).
pub(crate) const RECHECK_MS: u32 = 250;

/// Durasi animasi bawaan (ms). Diubah lewat vscroll-wm.conf (anim_ms).
pub(crate) const DEFAULT_ANIM_MS: f32 = 160.0;

pub(crate) const WM_ACTION: u32 = WM_APP + 1;

/// Win+Ctrl+Kiri / Kanan: workspace sebelumnya / berikutnya.
pub(crate) const HOTKEY_WS_PREV: i32 = 24;

pub(crate) const HOTKEY_WS_NEXT: i32 = 25;

/// Win+Ctrl+Shift+Kiri / Kanan: kirim window aktif ke workspace sebelumnya / berikutnya.
pub(crate) const HOTKEY_WS_SEND_PREV: i32 = 26;

pub(crate) const HOTKEY_WS_SEND_NEXT: i32 = 27;

/// Win+Ctrl+D / Win+Ctrl+F4 (buat / tutup desktop virtual Windows): ditelan saja.
pub(crate) const HOTKEY_WS_NOOP: i32 = 28;

/// Snap layouts: window dipilih di daftar slot (wParam = id window, lParam = id slot).
pub(crate) const WM_SNAP_PICK: u32 = WM_APP + 20;

/// Snap layouts: daftar slot dibatalkan (Esc, tombol tutup, klik kanan).
pub(crate) const WM_SNAP_CANCEL: u32 = WM_APP + 21;

/// Interval (ms) memantau kursor selama window diseret untuk menu snap layouts.
pub(crate) const SNAP_TICK_MS: u32 = 16;

/// Win+Ctrl+1..9 -> HOTKEY_WS_GO + 0..8 (pindah ke workspace n).
pub(crate) const HOTKEY_WS_GO: i32 = 30;

/// Win+Ctrl+Shift+1..9 -> HOTKEY_WS_SEND + 0..8 (kirim window aktif ke workspace n).
pub(crate) const HOTKEY_WS_SEND: i32 = 40;

/// Posisi x parkir window workspace lain: batas bawah USER32 (i16::MIN); nilai yang
/// lebih kecil dijepit ke sini. Sengaja bukan -32000 (posisi "minimized" lama).
pub(crate) const PARK_X: i32 = i16::MIN as i32;

/// Posted to the main thread when a Named Pipe IPC command arrives.
pub(crate) const WM_IPC: u32 = WM_APP + 15;

/// Posted from `kb_hook` to `MAIN_THREAD` to mask the Start menu outside the hook callback.
pub(crate) const WM_CANCEL_START: u32 = WM_APP + 16;

/// Posted to the dedicated hook thread to reinstall `WH_KEYBOARD_LL`.
pub(crate) const WM_REINSTALL_HOOK: u32 = WM_APP + 17;
