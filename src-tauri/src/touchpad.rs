//! Precision Touchpad gesture recognizer via Win32 Raw Input (`0x0D / 0x05`) + HID parser.
//!
//! Recognizes 3-finger horizontal scrubbing (custom Alt+Tab), 3-finger swipe up/down
//! (Activities or WM Overview / dismiss), and 4-finger horizontal swipes (`vscroll-wm`
//! workspace switching) while Windows' built-in 3/4-finger gestures are set to "Nothing".
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    mem::size_of,
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    thread,
    time::Instant,
};

use tauri::{AppHandle, Manager};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
            TranslateMessage, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
        },
    },
};

use crate::edge::Panels;

// ---------- Raw Input & HID constants/FFI ----------

const WM_INPUT: u32 = 0x00FF;
const RIM_TYPEHID: u32 = 2;
const RID_INPUT: u32 = 0x1000_0003;
const RIDI_PREPARSEDDATA: u32 = 0x2000_0005;
const RIDEV_INPUTSINK: u32 = 0x0000_0100;

const USAGE_PAGE_GENERIC: u16 = 0x01;
const USAGE_GENERIC_X: u16 = 0x30;
const USAGE_GENERIC_Y: u16 = 0x31;

const USAGE_PAGE_DIGITIZER: u16 = 0x0D;
const USAGE_TOUCH_PAD: u16 = 0x05;
const USAGE_TIP_SWITCH: u16 = 0x42;
const USAGE_CONTACT_ID: u16 = 0x51;
const USAGE_CONTACT_COUNT: u16 = 0x54;

const HIDP_INPUT: i32 = 0;
const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;

#[repr(C)]
struct RawInputDevice {
    us_usage_page: u16,
    us_usage: u16,
    dw_flags: u32,
    hwnd_target: HWND,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputHeader {
    dw_type: u32,
    dw_size: u32,
    h_device: isize,
    w_param: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawHidHeader {
    dw_size_hid: u32,
    dw_count: u32,
}

#[repr(C)]
#[derive(Default)]
struct HidpCaps {
    usage: u16,
    usage_page: u16,
    input_report_byte_length: u16,
    output_report_byte_length: u16,
    feature_report_byte_length: u16,
    reserved: [u16; 17],
    number_link_collection_nodes: u16,
    number_input_button_caps: u16,
    number_input_value_caps: u16,
    number_input_data_indices: u16,
    number_output_button_caps: u16,
    number_output_value_caps: u16,
    number_output_data_indices: u16,
    number_feature_button_caps: u16,
    number_feature_value_caps: u16,
    number_feature_data_indices: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HidpValueCaps {
    usage_page: u16,
    report_id: u8,
    is_alias: u8,
    bit_field: u16,
    link_collection: u16,
    link_usage: u16,
    link_usage_page: u16,
    is_range: u8,
    is_string_range: u8,
    is_designator_range: u8,
    is_absolute: u8,
    has_null: u8,
    reserved: u8,
    bit_size: u16,
    report_count: u16,
    reserved2: [u16; 5],
    units_exp: u32,
    units: u32,
    logical_min: i32,
    logical_max: i32,
    physical_min: i32,
    physical_max: i32,
    usage_min: u16,
    usage_max: u16,
    string_min: u16,
    string_max: u16,
    designator_min: u16,
    designator_max: u16,
    data_index_min: u16,
    data_index_max: u16,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterRawInputDevices(
        p_raw_input_devices: *const RawInputDevice,
        ui_num_devices: u32,
        cb_size: u32,
    ) -> i32;
    fn GetRawInputData(
        h_raw_input: isize,
        ui_command: u32,
        p_data: *mut c_void,
        pcb_size: *mut u32,
        cb_size_header: u32,
    ) -> u32;
    fn GetRawInputDeviceInfoW(
        h_device: isize,
        ui_command: u32,
        p_data: *mut c_void,
        pcb_size: *mut u32,
    ) -> u32;
}

#[link(name = "hid")]
unsafe extern "system" {
    fn HidP_GetCaps(preparsed_data: *const c_void, capabilities: *mut HidpCaps) -> i32;
    fn HidP_GetValueCaps(
        report_type: i32,
        value_caps: *mut HidpValueCaps,
        value_caps_length: *mut u16,
        preparsed_data: *const c_void,
    ) -> i32;
    fn HidP_GetUsageValue(
        report_type: i32,
        usage_page: u16,
        link_collection: u16,
        usage: u16,
        usage_value: *mut u32,
        preparsed_data: *const c_void,
        report: *const u8,
        report_length: u32,
    ) -> i32;
    fn HidP_GetUsages(
        report_type: i32,
        usage_page: u16,
        link_collection: u16,
        usage_list: *mut u16,
        usage_length: *mut u32,
        preparsed_data: *const c_void,
        report: *const u8,
        report_length: u32,
    ) -> i32;
}

// ---------- Runtime configuration ----------

static ENABLED: AtomicBool = AtomicBool::new(true);
static SWIPE_UP_OVERVIEW: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn set_config(enabled: bool, swipe_up: &str) {
    ENABLED.store(enabled, Ordering::Relaxed);
    SWIPE_UP_OVERVIEW.store(swipe_up == "overview", Ordering::Relaxed);
}

// ---------- HID device layout & packet parsing ----------

#[derive(Clone, Copy)]
struct ContactCol {
    col: u16,
    x_min: i32,
    x_max: i32,
    y_min: i32,
    y_max: i32,
}

struct DeviceInfo {
    preparsed: Vec<u8>,
    report_len: usize,
    contact_count_col: Option<u16>,
    contacts: Vec<ContactCol>,
    expected_contacts: u32,
    seen_slots: u32,
    pending_contacts: Vec<(f64, f64)>,
}

impl DeviceInfo {
    unsafe fn from_handle(h_device: isize) -> Option<Self> {
        let mut size: u32 = 0;
        let rc = GetRawInputDeviceInfoW(h_device, RIDI_PREPARSEDDATA, std::ptr::null_mut(), &mut size);
        if rc == u32::MAX || size == 0 {
            return None;
        }
        let mut preparsed = vec![0u8; size as usize];
        let got = GetRawInputDeviceInfoW(
            h_device,
            RIDI_PREPARSEDDATA,
            preparsed.as_mut_ptr() as *mut c_void,
            &mut size,
        );
        if got == u32::MAX {
            return None;
        }

        let mut caps = HidpCaps::default();
        if HidP_GetCaps(preparsed.as_ptr() as *const c_void, &mut caps) != HIDP_STATUS_SUCCESS {
            return None;
        }
        let mut val_len = caps.number_input_value_caps;
        if val_len == 0 {
            return None;
        }
        let mut val_caps = vec![HidpValueCaps::default(); val_len as usize];
        if HidP_GetValueCaps(
            HIDP_INPUT,
            val_caps.as_mut_ptr(),
            &mut val_len,
            preparsed.as_ptr() as *const c_void,
        ) != HIDP_STATUS_SUCCESS
        {
            return None;
        }

        let mut contact_count_col: Option<u16> = None;
        let mut per_col: HashMap<u16, (Option<(i32, i32)>, Option<(i32, i32)>, bool)> =
            HashMap::new();

        for vc in &val_caps[..val_len as usize] {
            let u_min = vc.usage_min;
            let u_max = if vc.is_range != 0 { vc.usage_max } else { vc.usage_min };
            if vc.usage_page == USAGE_PAGE_DIGITIZER
                && u_min <= USAGE_CONTACT_COUNT
                && USAGE_CONTACT_COUNT <= u_max
            {
                contact_count_col = Some(vc.link_collection);
            }
            if vc.usage_page == USAGE_PAGE_GENERIC {
                let entry = per_col.entry(vc.link_collection).or_insert((None, None, false));
                if u_min <= USAGE_GENERIC_X && USAGE_GENERIC_X <= u_max {
                    entry.0 = Some((vc.logical_min, vc.logical_max));
                }
                if u_min <= USAGE_GENERIC_Y && USAGE_GENERIC_Y <= u_max {
                    entry.1 = Some((vc.logical_min, vc.logical_max));
                }
            } else if vc.usage_page == USAGE_PAGE_DIGITIZER
                && u_min <= USAGE_CONTACT_ID
                && USAGE_CONTACT_ID <= u_max
            {
                let entry = per_col.entry(vc.link_collection).or_insert((None, None, false));
                entry.2 = true;
            }
        }

        let mut contacts: Vec<ContactCol> = per_col
            .into_iter()
            .filter_map(|(col, (x, y, _has_id))| {
                let ((x_min, x_max), (y_min, y_max)) = (x?, y?);
                if x_max > x_min && y_max > y_min {
                    Some(ContactCol { col, x_min, x_max, y_min, y_max })
                } else {
                    None
                }
            })
            .collect();
        contacts.sort_by_key(|c| c.col);
        if contacts.is_empty() {
            return None;
        }

        Some(Self {
            preparsed,
            report_len: caps.input_report_byte_length as usize,
            contact_count_col,
            contacts,
            expected_contacts: 0,
            seen_slots: 0,
            pending_contacts: Vec::with_capacity(8),
        })
    }

    unsafe fn tip_down(&self, report: &[u8], col: u16) -> bool {
        let mut usages = [0u16; 16];
        let mut len = usages.len() as u32;
        let status = HidP_GetUsages(
            HIDP_INPUT,
            USAGE_PAGE_DIGITIZER,
            col,
            usages.as_mut_ptr(),
            &mut len,
            self.preparsed.as_ptr() as *const c_void,
            report.as_ptr(),
            report.len() as u32,
        );
        status == HIDP_STATUS_SUCCESS && usages[..len as usize].contains(&USAGE_TIP_SWITCH)
    }

    unsafe fn usage_norm(&self, report: &[u8], col: u16, usage: u16, min: i32, max: i32) -> Option<f64> {
        let mut raw: u32 = 0;
        let status = HidP_GetUsageValue(
            HIDP_INPUT,
            USAGE_PAGE_GENERIC,
            col,
            usage,
            &mut raw,
            self.preparsed.as_ptr() as *const c_void,
            report.as_ptr(),
            report.len() as u32,
        );
        if status != HIDP_STATUS_SUCCESS {
            return None;
        }
        let span = (max - min).max(1) as f64;
        let val = if min < 0 {
            (raw as i32 - min) as f64
        } else {
            (raw as i64 - min as i64) as f64
        };
        Some((val / span).clamp(0.0, 1.0))
    }

    /// Parse one HID report and return `Some((finger_count, centroid_x, centroid_y))` when a
    /// complete multi-touch frame (parallel or hybrid) is ready.
    unsafe fn process_report(&mut self, report: &[u8]) -> Option<(usize, f64, f64)> {
        if self.report_len > 0 && report.len() != self.report_len {
            return None;
        }

        let mut count_val: u32 = 0;
        let has_count = if let Some(col) = self.contact_count_col {
            HidP_GetUsageValue(
                HIDP_INPUT,
                USAGE_PAGE_DIGITIZER,
                col,
                USAGE_CONTACT_COUNT,
                &mut count_val,
                self.preparsed.as_ptr() as *const c_void,
                report.as_ptr(),
                report.len() as u32,
            ) == HIDP_STATUS_SUCCESS
        } else {
            false
        };

        let mut frame_ready: Option<(usize, f64, f64)> = None;
        if has_count && count_val > 0 {
            if self.expected_contacts > 0 && !self.pending_contacts.is_empty() {
                frame_ready = Some(centroid_of(&self.pending_contacts));
            }
            self.pending_contacts.clear();
            self.expected_contacts = count_val;
            self.seen_slots = 0;
        }

        for c in &self.contacts {
            if !self.tip_down(report, c.col) {
                continue;
            }
            if let (Some(nx), Some(ny)) = (
                self.usage_norm(report, c.col, USAGE_GENERIC_X, c.x_min, c.x_max),
                self.usage_norm(report, c.col, USAGE_GENERIC_Y, c.y_min, c.y_max),
            ) {
                self.pending_contacts.push((nx, ny));
            }
        }
        if self.expected_contacts > 0 {
            self.seen_slots += self.contacts.len() as u32;
            if self.seen_slots >= self.expected_contacts
                || self.pending_contacts.len() as u32 >= self.expected_contacts
            {
                let res = centroid_of(&self.pending_contacts);
                self.pending_contacts.clear();
                self.expected_contacts = 0;
                self.seen_slots = 0;
                return Some(res);
            }
            return frame_ready;
        }

        if has_count && count_val == 0 {
            self.pending_contacts.clear();
            self.seen_slots = 0;
            return Some((0, 0.0, 0.0));
        }

        if !has_count {
            let res = if self.pending_contacts.is_empty() {
                (0, 0.0, 0.0)
            } else {
                centroid_of(&self.pending_contacts)
            };
            self.pending_contacts.clear();
            return Some(res);
        }

        frame_ready
    }
}

fn centroid_of(pts: &[(f64, f64)]) -> (usize, f64, f64) {
    let n = pts.len();
    if n == 0 {
        return (0, 0.0, 0.0);
    }
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(ax, ay), &(x, y)| (ax + x, ay + y));
    (n, sx / n as f64, sy / n as f64)
}

// ---------- Gesture state machine ----------

const SETTLE_MS: u128 = 70;
const LATE_4F_UPGRADE_MS: u128 = 120;
const H_START: f64 = 0.045;
const V_START: f64 = 0.075;
const SCRUB_STEP: f64 = 0.055;
const AXIS_RATIO: f64 = 1.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureAction {
    AltTabStart(bool),
    AltTabStep(i32),
    AltTabCommit,
    AltTabCancel,
    SwipeUp,
    SwipeDown,
    WorkspaceStep(i32),
}

enum State {
    Idle,
    Tracking {
        started: Instant,
        peak_fingers: usize,
        start_x: f64,
        start_y: f64,
    },
    AltTabScrub {
        started: Instant,
        last_step_x: f64,
    },
    Fired,
}

struct GestureEngine {
    state: State,
}

impl GestureEngine {
    fn new() -> Self {
        Self { state: State::Idle }
    }

    fn reset(&mut self) -> Option<GestureAction> {
        let act = match self.state {
            State::AltTabScrub { .. } => Some(GestureAction::AltTabCommit),
            _ => None,
        };
        self.state = State::Idle;
        act
    }

    fn update(&mut self, n: usize, cx: f64, cy: f64, now: Instant) -> Vec<GestureAction> {
        let mut actions = Vec::new();
        if n == 0 {
            if let Some(a) = self.reset() {
                actions.push(a);
            }
            return actions;
        }

        match self.state {
            State::Idle => {
                if n >= 3 {
                    self.state = State::Tracking {
                        started: now,
                        peak_fingers: n,
                        start_x: cx,
                        start_y: cy,
                    };
                }
            }
            State::Tracking {
                started,
                ref mut peak_fingers,
                ref mut start_x,
                ref mut start_y,
            } => {
                if n < 3 {
                    self.state = State::Idle;
                    return actions;
                }
                if n > *peak_fingers {
                    *peak_fingers = n;
                    *start_x = cx;
                    *start_y = cy;
                    return actions;
                }
                if now.duration_since(started).as_millis() < SETTLE_MS {
                    return actions;
                }
                let dx = cx - *start_x;
                let dy = cy - *start_y;
                let adx = dx.abs();
                let ady = dy.abs();

                if *peak_fingers == 3 {
                    if adx >= H_START && adx > ady * AXIS_RATIO {
                        self.state = State::AltTabScrub { started: now, last_step_x: cx };
                        actions.push(GestureAction::AltTabStart(dx < 0.0));
                    } else if ady >= V_START && ady > adx * AXIS_RATIO {
                        self.state = State::Fired;
                        actions.push(if dy < 0.0 {
                            GestureAction::SwipeUp
                        } else {
                            GestureAction::SwipeDown
                        });
                    }
                } else if *peak_fingers >= 4 && adx >= H_START && adx > ady * AXIS_RATIO {
                    self.state = State::Fired;
                    actions.push(GestureAction::WorkspaceStep(if dx < 0.0 { 1 } else { -1 }));
                }
            }
            State::AltTabScrub { started, ref mut last_step_x } => {
                // If a 4th finger lands slightly after Alt+Tab started, upgrade cleanly to 4-finger tracking.
                if n >= 4 && now.duration_since(started).as_millis() <= LATE_4F_UPGRADE_MS {
                    actions.push(GestureAction::AltTabCancel);
                    self.state = State::Tracking {
                        started: now,
                        peak_fingers: n,
                        start_x: cx,
                        start_y: cy,
                    };
                    return actions;
                }
                // Freeze tracking when fewer than 3 fingers remain so lifting fingers one-by-one
                // cannot shift the centroid and jump cards right before commit.
                if n < 3 {
                    return actions;
                }
                while cx - *last_step_x >= SCRUB_STEP {
                    *last_step_x += SCRUB_STEP;
                    actions.push(GestureAction::AltTabStep(1));
                }
                while *last_step_x - cx >= SCRUB_STEP {
                    *last_step_x -= SCRUB_STEP;
                    actions.push(GestureAction::AltTabStep(-1));
                }
            }
            State::Fired => {}
        }

        actions
    }
}

fn dispatch(app: &AppHandle, action: GestureAction) {
    match action {
        GestureAction::AltTabStart(rev) => crate::altswitch::gesture_start(rev),
        GestureAction::AltTabStep(d) => crate::altswitch::gesture_step(d),
        GestureAction::AltTabCommit => crate::altswitch::gesture_commit(),
        GestureAction::AltTabCancel => crate::altswitch::gesture_cancel(),
        GestureAction::SwipeUp => {
            if SWIPE_UP_OVERVIEW.load(Ordering::Relaxed) {
                crate::wm_ipc::toggle_overview_ui(app, "gesture");
            } else {
                let panels = app.state::<Panels>();
                if panels.is_open("overview") {
                    panels.set("overview", false);
                }
                if !crate::activities::is_open() {
                    crate::activities::show(app);
                }
            }
        }
        GestureAction::SwipeDown => {
            let panels = app.state::<Panels>();
            if crate::activities::is_open() {
                crate::activities::hide(app);
            } else if panels.is_open("overview") {
                panels.set("overview", false);
            } else if panels.is_open("launcher") {
                panels.set("launcher", false);
            } else if panels.is_open("notch") {
                panels.set("notch", false);
            }
        }
        GestureAction::WorkspaceStep(d) => crate::wm_ipc::step_workspace(d),
    }
}

// ---------- Window message loop ----------

struct TouchpadRuntime {
    devices: HashMap<isize, Option<DeviceInfo>>,
    engine: GestureEngine,
    buf: Vec<u8>,
}

thread_local! {
    static RUNTIME: RefCell<Option<TouchpadRuntime>> = const { RefCell::new(None) };
}

unsafe fn handle_raw_input(h_raw: isize) {
    let Some(app) = APP.get() else { return };
    RUNTIME.with(|cell| {
        let mut slot = cell.borrow_mut();
        let rt = slot.get_or_insert_with(|| TouchpadRuntime {
            devices: HashMap::new(),
            engine: GestureEngine::new(),
            buf: Vec::with_capacity(256),
        });

        if !ENABLED.load(Ordering::Relaxed) {
            if let Some(a) = rt.engine.reset() {
                dispatch(app, a);
            }
            return;
        }

        let mut size: u32 = 0;
        let hdr_size = size_of::<RawInputHeader>() as u32;
        if GetRawInputData(h_raw, RID_INPUT, std::ptr::null_mut(), &mut size, hdr_size) == u32::MAX
            || size < hdr_size + size_of::<RawHidHeader>() as u32
        {
            return;
        }
        if rt.buf.len() < size as usize {
            rt.buf.resize(size as usize, 0);
        }
        let got = GetRawInputData(
            h_raw,
            RID_INPUT,
            rt.buf.as_mut_ptr() as *mut c_void,
            &mut size,
            hdr_size,
        );
        if got == u32::MAX || (got as usize) < size_of::<RawInputHeader>() + size_of::<RawHidHeader>() {
            return;
        }

        let header = std::ptr::read_unaligned(rt.buf.as_ptr() as *const RawInputHeader);
        if header.dw_type != RIM_TYPEHID {
            return;
        }
        let hid_hdr = std::ptr::read_unaligned(
            rt.buf.as_ptr().add(size_of::<RawInputHeader>()) as *const RawHidHeader,
        );
        let data_offset = size_of::<RawInputHeader>() + size_of::<RawHidHeader>();
        let report_len = hid_hdr.dw_size_hid as usize;
        let report_count = hid_hdr.dw_count as usize;
        if report_len == 0 || report_count == 0 || data_offset + report_len * report_count > got as usize {
            return;
        }

        let dev_entry = rt
            .devices
            .entry(header.h_device)
            .or_insert_with(|| DeviceInfo::from_handle(header.h_device));
        let Some(dev) = dev_entry.as_mut() else {
            return;
        };

        let now = Instant::now();
        for i in 0..report_count {
            let start = data_offset + i * report_len;
            let report = &rt.buf[start..start + report_len];
            if let Some((n, cx, cy)) = dev.process_report(report) {
                for action in rt.engine.update(n, cx, cy, now) {
                    dispatch(app, action);
                }
            }
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        handle_raw_input(lparam.0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Start the Precision Touchpad Raw Input listener on a dedicated background thread.
pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    thread::Builder::new()
        .name("orumi-touchpad".into())
        .spawn(|| unsafe {
            let Ok(h_mod) = GetModuleHandleW(None) else {
                eprintln!("[touchpad] GetModuleHandleW failed");
                return;
            };
            let class_name = w!("OrumiTouchpadSink");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: h_mod.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&wc);

            let hwnd_message = HWND(-3isize as *mut c_void);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("orumi-touchpad"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(hwnd_message),
                None,
                Some(h_mod.into()),
                None,
            ) else {
                eprintln!("[touchpad] failed to create message-only window");
                return;
            };

            let rid = RawInputDevice {
                us_usage_page: USAGE_PAGE_DIGITIZER,
                us_usage: USAGE_TOUCH_PAD,
                dw_flags: RIDEV_INPUTSINK,
                hwnd_target: hwnd,
            };
            if RegisterRawInputDevices(&rid, 1, size_of::<RawInputDevice>() as u32) == 0 {
                eprintln!("[touchpad] RegisterRawInputDevices failed");
                return;
            }

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
