//! Penanganan hotkey, event Windows, dan timer debounce.

use std::sync::atomic::Ordering;

use windows::Win32::Foundation::HWND;

#[allow(unused_imports)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[allow(unused_imports)]
use windows::Win32::UI::WindowsAndMessaging::*;

use super::App;
use crate::consts::{
    DIRS, EVENT_DEBOUNCE_MS, HOTKEY_CHEATSHEET, HOTKEY_CLOSE, HOTKEY_FLOAT, HOTKEY_INSPECT,
    HOTKEY_MAXIMIZE, HOTKEY_MINIMIZE, HOTKEY_OVERVIEW, HOTKEY_SPLIT_LEFT, HOTKEY_SPLIT_RESET,
    HOTKEY_SPLIT_RIGHT, HOTKEY_WS_GO, HOTKEY_WS_NEXT, HOTKEY_WS_NOOP, HOTKEY_WS_PREV, HOTKEY_WS_SEND,
    HOTKEY_WS_SEND_NEXT, HOTKEY_WS_SEND_PREV,
};
use crate::winutil::{manageable, wid, work_area};
use crate::{cheatsheet, tray};
use crate::config::HeightMode;

impl App {
    /// Jadwalkan sync + apply dalam `ms` milidetik (digabung kalau sudah terjadwal).
    pub(crate) unsafe fn schedule_sync(&mut self, ms: u32) {
        if self.sync_timer == 0 {
            self.sync_timer = SetTimer(None, 0, ms, None);
        }
    }

    pub(crate) unsafe fn stop_sync_timer(&mut self) {
        if self.sync_timer != 0 {
            let _ = KillTimer(None, self.sync_timer);
            self.sync_timer = 0;
        }
    }

    /// Event Windows: hanya yang menyangkut window yang kita kelola (atau
    /// kandidat baru) yang memicu penataan ulang.
    pub(crate) unsafe fn on_event(&mut self, event: u32, hwnd: HWND) {
        let known = self.map.contains_key(&wid(hwnd));
        let relevant = match event {
            EVENT_SYSTEM_FOREGROUND => true,
            EVENT_SYSTEM_MOVESIZEEND
            | EVENT_SYSTEM_MINIMIZESTART
            | EVENT_OBJECT_DESTROY
            | EVENT_OBJECT_HIDE
            | EVENT_OBJECT_CLOAKED => known,
            EVENT_SYSTEM_MINIMIZEEND | EVENT_OBJECT_SHOW | EVENT_OBJECT_UNCLOAKED => {
                known || manageable(hwnd)
            }
            _ => false,
        };
        if event == EVENT_SYSTEM_MOVESIZEEND && known {
            // drag / tarik tepi window selesai: tukar posisi atau geser pembagi (mouse.rs)
            self.on_drag_end(hwnd);
        }
        if relevant {
            self.schedule_sync(EVENT_DEBOUNCE_MS);
        }
    }

    pub(crate) unsafe fn on_hotkey(&mut self, id: i32) {
        if tray::PAUSED.load(Ordering::Relaxed) {
            return;
        }
        self.sync();
        match id {
            // mode step: Win+↑/↓ menggeser tinggi per langkah, bukan lompat preset
            3 | 4 if self.cfg.height_mode == HeightMode::Step => {
                let step = self.cfg.height_step;
                self.ws.cur_mut().adjust_height(if id == 3 { -step } else { step });
            }
            1..=4 => self.ws.cur_mut().snap(DIRS[(id - 1) as usize].1),
            5..=8 => {
                let fg = wid(GetForegroundWindow());
                if self.map.contains_key(&fg) && self.ws.workspace_of(fg) == Some(self.ws.active())
                {
                    self.ws.cur_mut().set_focus(fg);
                }
                self.ws.cur_mut().focus(DIRS[(id - 5) as usize].1);
            }
            HOTKEY_MAXIMIZE => self.ws.cur_mut().maximize(),
            HOTKEY_INSPECT => {
                self.inspect();
                return;
            }
            HOTKEY_OVERVIEW => {
                if crate::overview::is_suppressed() {
                    crate::ipc::bump_overview_seq();
                    self.publish_ipc();
                } else {
                    self.toggle_overview();
                }
                return;
            }
            // pembagi lajur digeser; penataan ulang oleh apply() di bawah
            HOTKEY_SPLIT_RIGHT => {
                self.ws.cur_mut().adjust_split(self.cfg.resize_step);
            }
            HOTKEY_SPLIT_LEFT => {
                self.ws.cur_mut().adjust_split(-self.cfg.resize_step);
            }
            HOTKEY_SPLIT_RESET => {
                self.ws.cur_mut().reset_split();
            }
            HOTKEY_CHEATSHEET => {
                if cheatsheet::is_suppressed() {
                    crate::ipc::bump_cheatsheet_seq();
                    self.publish_ipc();
                } else {
                    cheatsheet::toggle(work_area(), &self.cfg);
                }
                return;
            }
            HOTKEY_CLOSE => {
                self.close_active();
                return;
            }
            HOTKEY_MINIMIZE => {
                self.minimize_active();
                return;
            }
            HOTKEY_FLOAT => {
                self.toggle_float();
                self.sync(); // window yang baru di-tile masuk strip
            }
            10..=13 => self.ws.cur_mut().move_window(DIRS[(id - 10) as usize].1),
            HOTKEY_WS_PREV => {
                self.switch_workspace_rel(-1);
                return;
            }
            HOTKEY_WS_NEXT => {
                self.switch_workspace_rel(1);
                return;
            }
            HOTKEY_WS_SEND_PREV => {
                self.send_to_workspace_rel(-1);
                return;
            }
            HOTKEY_WS_SEND_NEXT => {
                self.send_to_workspace_rel(1);
                return;
            }
            n if (HOTKEY_WS_GO..HOTKEY_WS_GO + 9).contains(&n) => {
                self.switch_workspace((n - HOTKEY_WS_GO) as usize);
                return;
            }
            n if (HOTKEY_WS_SEND..HOTKEY_WS_SEND + 9).contains(&n) => {
                self.send_to_workspace((n - HOTKEY_WS_SEND) as usize);
                return;
            }
            HOTKEY_WS_NOOP => return,
            _ => return,
        }
        self.apply();
        if (5..=8).contains(&id) {
            if let Some(h) = self.ws.cur().focused().and_then(|f| self.map.get(&f).copied()) {
                self.focus_hwnd(h);
            }
        }
    }
}
