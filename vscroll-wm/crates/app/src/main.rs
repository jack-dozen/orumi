//! v1: satu monitor. Tracking window lewat event Windows (SetWinEventHook) plus
//! polling lambat 1 detik sebagai jaring pengaman.
//!
//! Win+Arrow             -> snap window aktif (hook, menelan Win+Arrow bawaan)
//! Win+Shift+Arrow       -> pindah fokus
//! Win+Enter             -> kembalikan window aktif ke penuh
//! Win+Alt+Arrow         -> tukar posisi dengan window tetangga
//! Win+Alt+F             -> float / tile window aktif
//! Win+Alt+I             -> cetak info window aktif (class / exe / title) di console
//! Win+Tab / Win+Alt+Space -> overview: peta strip (tombol: overview_key)
//! Win+] / Win+[         -> lebarkan / sempitkan lajur window aktif (geser pembagi)
//! Win+\                 -> kembalikan pembagi lajur ke 50/50
//! Win+/                 -> cheatsheet pintasan keyboard (tombol: cheatsheet_key)
//! Win+Q                 -> tutup window aktif
//! Win+M                 -> minimize window aktif
//! Win+Ctrl+Kiri/Kanan   -> workspace sebelumnya / berikutnya (+Shift: kirim window aktif)
//! Win+Ctrl+1..9         -> pindah ke workspace n (+Shift: kirim window aktif ke sana)
//! Win+Ctrl+Esc          -> keluar dan kembalikan semua window (juga Ctrl+C)
//! Ctrl+Alt+... (cadangan, aksi yang sama)
//!
//! Aslinya:
//! Ctrl+Alt+Arrow        -> snap window aktif
//! Ctrl+Alt+Shift+Arrow  -> pindah fokus
//! Ctrl+Alt+Enter        -> kembalikan window aktif ke penuh (lebar + tinggi)
//! Tutup dengan Ctrl+C di console.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bindings;
mod cheatsheet;
mod config;
mod consts;
mod hook;
mod ipc;
mod layout_state;
mod osd;
mod overview;
mod rules;
mod safety;
mod taskbar;
mod tray;
mod winutil;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::App;
use crate::consts::{
    DIRS, HOTKEY_FLOAT, HOTKEY_INSPECT, HOTKEY_MAXIMIZE, HOTKEY_OVERVIEW, HOTKEY_QUIT, RECHECK_MS,
    WM_ACTION, WM_CANCEL_START, WM_IPC, WM_WINEVENT,
};
use crate::hook::{win_event_proc, MAIN_THREAD};
use crate::winutil::{single_instance, work_area};

fn main() {
    unsafe {
        if !single_instance() {
            eprintln!("vscroll-wm sudah berjalan.");
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mut app = App::new();
        let report = config::load();
        app.apply_config(&report.cfg);

        for (i, (vk, _)) in DIRS.iter().enumerate() {
            let base = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
            if RegisterHotKey(None, i as i32 + 1, base, vk.0 as u32).is_err() {
                eprintln!("gagal daftar Ctrl+Alt+{:?} (dipakai app lain?)", vk);
            }
            if RegisterHotKey(None, i as i32 + 5, base | MOD_SHIFT, vk.0 as u32).is_err() {
                eprintln!("gagal daftar Ctrl+Alt+Shift+{:?} (dipakai app lain?)", vk);
            }
        }
        if RegisterHotKey(
            None,
            HOTKEY_MAXIMIZE,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_RETURN.0 as u32,
        )
        .is_err()
        {
            eprintln!("gagal daftar Ctrl+Alt+Enter (dipakai app lain?)");
        }
        if RegisterHotKey(
            None,
            HOTKEY_QUIT,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            VK_Q.0 as u32,
        )
        .is_err()
        {
            eprintln!("gagal daftar Ctrl+Alt+Shift+Q (dipakai app lain?)");
        }
        if RegisterHotKey(
            None,
            HOTKEY_FLOAT,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_F.0 as u32,
        )
        .is_err()
        {
            eprintln!("gagal daftar Ctrl+Alt+F (dipakai app lain?)");
        }
        if RegisterHotKey(
            None,
            HOTKEY_INSPECT,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_I.0 as u32,
        )
        .is_err()
        {
            eprintln!("gagal daftar Ctrl+Alt+I (dipakai app lain?)");
        }
        MAIN_THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
        safety::install(GetCurrentThreadId());
        taskbar::init();
        let no_overview = std::env::args().any(|a| a == "--no-overview");
        if no_overview {
            overview::set_suppressed(true);
        } else if !overview::init(GetCurrentThreadId()) {
            eprintln!("gagal membuat jendela overview");
        }
        let no_cheatsheet = std::env::args().any(|a| a == "--no-cheatsheet");
        if no_cheatsheet {
            cheatsheet::set_suppressed(true);
        } else if !cheatsheet::init() {
            eprintln!("gagal membuat jendela cheatsheet");
        }
        let no_osd = std::env::args().any(|a| a == "--no-osd");
        if no_osd {
            osd::set_suppressed(true);
        } else if !osd::init() {
            eprintln!("gagal membuat jendela osd workspace");
        }
        if RegisterHotKey(
            None,
            HOTKEY_OVERVIEW,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_SPACE.0 as u32,
        )
        .is_err()
        {
            eprintln!("gagal daftar Ctrl+Alt+Space (dipakai app lain?)");
        }
        let no_tray = std::env::args().any(|a| a == "--no-tray");
        if !no_tray && !tray::create(GetCurrentThreadId()) {
            eprintln!("gagal membuat ikon tray");
        }
        let ipc_rx = ipc::start(GetCurrentThreadId());
        hook::start_kb_hook();
        let mut event_hooks: Vec<HWINEVENTHOOK> = Vec::new();
        for (lo, hi) in [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ] {
            let h = SetWinEventHook(
                lo,
                hi,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if h.is_invalid() {
                eprintln!("gagal pasang event hook {lo:#x}..{hi:#x}");
            } else {
                event_hooks.push(h);
            }
        }
        // Dengan event hook, polling cuma jaring pengaman (mis. window yang baru
        // diberi judul setelah muncul). Tanpa hook, kembali ke polling cepat.
        let poll_ms = if event_hooks.is_empty() { 300 } else { 1000 };
        SetTimer(None, 1, poll_ms, None);

        println!("vscroll-wm jalan. Win+Arrow = snap, +Shift = fokus, Win+Enter = penuh, Win+Alt = tukar posisi, Win+Alt+F = float/tile, Win+Alt+I = info window, Win+Q = tutup, Win+M = minimize, Win+<overview_key> / Win+Alt+Space = overview, Win+<cheatsheet_key> = daftar pintasan (cadangan: Ctrl+Alt+...). Win+Ctrl+Esc atau Ctrl+C = keluar dan pulihkan semua window.");
        app.restore_layout(); // susunan dari sesi sebelumnya, untuk window yang masih terbuka
        let changed = app.sync();
        app.restore_all_tabs(); // tombol taskbar yang tertinggal dari proses sebelumnya yang mati paksa
        if changed {
            app.apply();
        }
        app.publish_ipc();

        app::show_config_problems(&report); // salah ketik di config langsung terlihat
        let mut msg = MSG::default();
        'msg_loop: while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                safety::WM_QUIT_REQUEST => break,
                WM_IPC => {
                    while let Ok(cmd) = ipc_rx.try_recv() {
                        if ipc::handle(&mut app, cmd) {
                            break 'msg_loop;
                        }
                    }
                }
                WM_HOTKEY | WM_ACTION => {
                    let id = msg.wParam.0 as i32;
                    if id == HOTKEY_QUIT {
                        break;
                    }
                    app.on_hotkey(id);
                }
                WM_CANCEL_START => hook::cancel_start_menu(),
                tray::WM_TRAY_PAUSE => app.toggle_pause(),
                tray::WM_TRAY_RELOAD => app.reload_config(),
                tray::WM_TRAY_HELP => {
                    if cheatsheet::is_suppressed() {
                        ipc::bump_cheatsheet_seq();
                        app.publish_ipc();
                    } else {
                        cheatsheet::toggle(work_area(), &app.cfg);
                    }
                }
                overview::WM_OV_MOVE => app.overview_move(msg.wParam.0),
                overview::WM_OV_CYCLE => app.overview_cycle(msg.wParam.0 != 0),
                overview::WM_OV_WS => app.overview_workspace(msg.wParam.0),
                overview::WM_OV_ACCEPT => app.close_overview(true),
                overview::WM_OV_CANCEL => app.close_overview(false),
                overview::WM_OV_PICK => app.overview_pick(msg.wParam.0 as u64),
                overview::WM_OV_DROP => app.overview_drop(),
                WM_WINEVENT => {
                    if !tray::PAUSED.load(Ordering::Relaxed) {
                        app.on_event(msg.wParam.0 as u32, HWND(msg.lParam.0 as *mut c_void));
                    }
                }
                WM_TIMER if !msg.hwnd.0.is_null() => {
                    // timer milik sebuah jendela (osd workspace): serahkan ke window proc-nya
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                WM_TIMER => {
                    let id = msg.wParam.0;
                    let paused = tray::PAUSED.load(Ordering::Relaxed);
                    if app.anim_timer != 0 && id == app.anim_timer {
                        app.tick_anim();
                    } else if app.sync_timer != 0 && id == app.sync_timer {
                        app.stop_sync_timer();
                        if !paused {
                            if app.sync() {
                                app.apply();
                            }
                            if std::mem::take(&mut app.recheck) {
                                app.schedule_sync(RECHECK_MS);
                            }
                        }
                    } else if !paused {
                        if app.sync() {
                            app.apply(); // jaring pengaman
                        }
                        app.save_layout(); // hanya menulis kalau susunan berubah
                    }
                }
                _ => {
                    // pesan untuk window tray (menu, dll)
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        // Keluar: simpan susunan terakhir, lalu kembalikan semua window ke posisi aslinya.
        app.save_layout();
        app.stop_anim_timer();
        app.stop_sync_timer();
        for h in event_hooks {
            let _ = UnhookWinEvent(h);
        }
        tray::remove();
        safety::restore_all();
        taskbar::shutdown();
        safety::EXIT_DONE.store(true, Ordering::Relaxed);
        println!("semua window dikembalikan. sampai jumpa.");
    }
}
