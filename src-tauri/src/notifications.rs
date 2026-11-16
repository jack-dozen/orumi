//! Custom Windows 11 notifications backed by read-only inspection of `wpndatabase.db`.
//! Adapted from Astraeus (GPL-3.0): `sys_notifs.cpp` and `sys_notify.cpp`.
//!
//! Safety guarantees:
//! - Never writes to `wpndatabase.db`; unread/dismissed state lives in ORUMI's own `notif-state.json`.
//! - Validates toast deep-link URIs (`file:` and `shell:` schemes are blocked).
//! - Optional native Windows banner suppression (`ToastEnabled = 0`) uses a persistent restore
//!   marker (`notif-restore.json`) restored on normal exit and by `--guard` after a crash/kill.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::{HashMap, HashSet},
    ffi::{c_char, c_int, c_void, CString},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        },
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
            Registry::{
                RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD,
                RRF_RT_REG_DWORD,
            },
            Threading::WaitForSingleObject,
        },
        UI::{
            Shell::{
                IShellItem, SHCreateItemFromParsingName, ShellExecuteW, SIGDN_NORMALDISPLAY,
            },
            WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY, SW_SHOWNORMAL},
        },
    },
};

const BUSY_MS: c_int = 200;
const SETTLE_MS: u64 = 150;
const POLL_MS: u32 = 3000;
const READ_MAX: usize = 50;
const MAX_QUERY_ROWS: usize = 100;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const TITLE_CAP: usize = 200;
const BODY_CAP: usize = 400;
const NAME_CACHE_MAX: usize = 128;

const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x0000_0008;
const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x0000_0010;

const PUSH_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications");
const TOAST_VALUE: PCWSTR = w!("ToastEnabled");

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
const SQLITE_OPEN_URI: c_int = 0x0000_0040;
const SQLITE_OPEN_NOMUTEX: c_int = 0x0000_8000;

const NOTIF_QUERY: &[u8] = b"SELECT n.Id, h.PrimaryId, n.ArrivalTime, n.Payload \
FROM Notification n JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
WHERE n.Type = 'toast' AND (n.ExpiryTime IS NULL OR n.ExpiryTime = 0 OR n.ExpiryTime > ?1) \
ORDER BY n.ArrivalTime DESC, n.[Order] DESC LIMIT ?2\0";

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn FindFirstChangeNotificationW(
        path: *const u16,
        watch_subtree: i32,
        notify_filter: u32,
    ) -> *mut c_void;
    fn FindNextChangeNotification(change_handle: *mut c_void) -> i32;
    fn FindCloseChangeNotification(change_handle: *mut c_void) -> i32;
}

// ---------- Data types ----------

#[derive(Clone, Serialize)]
pub struct NotifEntry {
    pub id: i64,
    pub app: String,
    pub aumid: String,
    pub title: String,
    pub body: String,
    pub arrived_ms: u64,
    pub image: Option<String>,
    pub icon_target: Option<String>,
}

#[derive(Clone)]
struct RawNotif {
    id: i64,
    aumid: String,
    app: String,
    title: String,
    body: String,
    arrived_ms: u64,
    launch: String,
    image: Option<String>,
    icon_target: Option<String>,
}

impl RawNotif {
    fn to_entry(&self) -> NotifEntry {
        NotifEntry {
            id: self.id,
            app: self.app.clone(),
            aumid: self.aumid.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
            arrived_ms: self.arrived_ms,
            image: self.image.clone(),
            icon_target: self.icon_target.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedState {
    #[serde(default)]
    read_ids: Vec<i64>,
    #[serde(default)]
    orumi_dnd: bool,
}

#[derive(Serialize, Deserialize)]
struct RestoreMarker {
    original_toast_enabled: Option<u32>,
}

struct InnerState {
    items: Vec<NotifEntry>,
    active_ids: Vec<i64>,
    read_ids: HashSet<i64>,
    launch_map: HashMap<i64, (String, String)>,
    high_water_id: Option<i64>,
    orumi_dnd: bool,
    suppress_native: bool,
    popup_enabled: bool,
    enabled: bool,
}

impl InnerState {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            active_ids: Vec::new(),
            read_ids: HashSet::new(),
            launch_map: HashMap::new(),
            high_water_id: None,
            orumi_dnd: false,
            suppress_native: false,
            popup_enabled: true,
            enabled: true,
        }
    }
}

pub struct NotifState(Mutex<InnerState>);

impl NotifState {
    pub fn new() -> Self {
        Self(Mutex::new(InnerState::new()))
    }
}

static ACTIVE: AtomicBool = AtomicBool::new(true);
static SUPPRESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static ORUMI_DND: AtomicBool = AtomicBool::new(false);
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

// ---------- Dynamic winsqlite3.dll wrapper ----------

#[repr(C)]
struct Sqlite3 {
    _unused: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _unused: [u8; 0],
}

type OpenV2Fn = unsafe extern "C" fn(*const c_char, *mut *mut Sqlite3, c_int, *const c_char) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut Sqlite3) -> c_int;
type BusyTimeoutFn = unsafe extern "C" fn(*mut Sqlite3, c_int) -> c_int;
type PrepareV2Fn = unsafe extern "C" fn(
    *mut Sqlite3,
    *const c_char,
    c_int,
    *mut *mut Sqlite3Stmt,
    *mut *const c_char,
) -> c_int;
type FinalizeFn = unsafe extern "C" fn(*mut Sqlite3Stmt) -> c_int;
type BindInt64Fn = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int, i64) -> c_int;
type StepFn = unsafe extern "C" fn(*mut Sqlite3Stmt) -> c_int;
type ColumnInt64Fn = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> i64;
type ColumnTextFn = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> *const u8;
type ColumnBlobFn = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> *const c_void;
type ColumnBytesFn = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> c_int;

struct SqliteApi {
    open_v2: OpenV2Fn,
    close: CloseFn,
    busy_timeout: BusyTimeoutFn,
    prepare_v2: PrepareV2Fn,
    finalize: FinalizeFn,
    bind_int64: BindInt64Fn,
    step: StepFn,
    column_int64: ColumnInt64Fn,
    column_text: ColumnTextFn,
    column_blob: ColumnBlobFn,
    column_bytes: ColumnBytesFn,
}

static SQLITE: OnceLock<Option<SqliteApi>> = OnceLock::new();

unsafe fn resolve_fn<T: Copy>(module: *mut c_void, name: &[u8]) -> Option<T> {
    let ptr = GetProcAddress(module, name.as_ptr());
    (!ptr.is_null()).then(|| std::mem::transmute_copy(&ptr))
}

fn sqlite_api() -> Option<&'static SqliteApi> {
    SQLITE
        .get_or_init(|| unsafe {
            let mut sys_dir = [0u16; 260];
            let len = GetSystemDirectoryW(sys_dir.as_mut_ptr(), sys_dir.len() as u32) as usize;
            if len == 0 || len >= sys_dir.len() {
                return None;
            }
            let mut full = PathBuf::from(String::from_utf16_lossy(&sys_dir[..len]));
            full.push("winsqlite3.dll");
            let mut wide: Vec<u16> = full.as_os_str().to_string_lossy().encode_utf16().collect();
            wide.push(0);
            let module = LoadLibraryW(wide.as_ptr());
            if module.is_null() {
                return None;
            }
            Some(SqliteApi {
                open_v2: resolve_fn(module, b"sqlite3_open_v2\0")?,
                close: resolve_fn(module, b"sqlite3_close\0")?,
                busy_timeout: resolve_fn(module, b"sqlite3_busy_timeout\0")?,
                prepare_v2: resolve_fn(module, b"sqlite3_prepare_v2\0")?,
                finalize: resolve_fn(module, b"sqlite3_finalize\0")?,
                bind_int64: resolve_fn(module, b"sqlite3_bind_int64\0")?,
                step: resolve_fn(module, b"sqlite3_step\0")?,
                column_int64: resolve_fn(module, b"sqlite3_column_int64\0")?,
                column_text: resolve_fn(module, b"sqlite3_column_text\0")?,
                column_blob: resolve_fn(module, b"sqlite3_column_blob\0")?,
                column_bytes: resolve_fn(module, b"sqlite3_column_bytes\0")?,
            })
        })
        .as_ref()
}

struct Db {
    api: &'static SqliteApi,
    ptr: *mut Sqlite3,
}

impl Db {
    fn open_readonly(uri: &str) -> Option<Self> {
        let api = sqlite_api()?;
        let c_uri = CString::new(uri).ok()?;
        let mut ptr: *mut Sqlite3 = std::ptr::null_mut();
        let rc = unsafe {
            (api.open_v2)(
                c_uri.as_ptr(),
                &mut ptr,
                SQLITE_OPEN_READONLY | SQLITE_OPEN_URI | SQLITE_OPEN_NOMUTEX,
                std::ptr::null(),
            )
        };
        if rc != SQLITE_OK || ptr.is_null() {
            if !ptr.is_null() {
                unsafe { (api.close)(ptr) };
            }
            return None;
        }
        unsafe { (api.busy_timeout)(ptr, BUSY_MS) };
        Some(Self { api, ptr })
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { (self.api.close)(self.ptr) };
            self.ptr = std::ptr::null_mut();
        }
    }
}

struct Stmt<'a> {
    api: &'static SqliteApi,
    ptr: *mut Sqlite3Stmt,
    _db: &'a Db,
}

impl<'a> Stmt<'a> {
    fn prepare(db: &'a Db, sql: &[u8]) -> Option<Self> {
        let mut ptr: *mut Sqlite3Stmt = std::ptr::null_mut();
        let rc = unsafe {
            (db.api.prepare_v2)(
                db.ptr,
                sql.as_ptr() as *const c_char,
                -1,
                &mut ptr,
                std::ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || ptr.is_null() {
            return None;
        }
        Some(Self { api: db.api, ptr, _db: db })
    }

    fn col_text(&self, col: c_int) -> String {
        unsafe {
            let p = (self.api.column_text)(self.ptr, col);
            let n = (self.api.column_bytes)(self.ptr, col);
            if p.is_null() || n <= 0 {
                return String::new();
            }
            let slice = std::slice::from_raw_parts(p, n as usize);
            String::from_utf8_lossy(slice).into_owned()
        }
    }

    fn col_blob(&self, col: c_int) -> &[u8] {
        unsafe {
            let p = (self.api.column_blob)(self.ptr, col) as *const u8;
            let n = (self.api.column_bytes)(self.ptr, col);
            if p.is_null() || n <= 0 {
                return &[];
            }
            let len = (n as usize).min(MAX_PAYLOAD_BYTES);
            std::slice::from_raw_parts(p, len)
        }
    }
}

impl Drop for Stmt<'_> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { (self.api.finalize)(self.ptr) };
            self.ptr = std::ptr::null_mut();
        }
    }
}

// ---------- Paths & timestamps ----------

fn notif_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Microsoft").join("Windows").join("Notifications"))
}

fn db_path() -> Option<PathBuf> {
    Some(notif_dir()?.join("wpndatabase.db"))
}

fn file_uri(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut uri = String::from("file:///");
    for byte in path.to_string_lossy().as_bytes() {
        let c = *byte;
        if c == b'\\' {
            uri.push('/');
        } else if matches!(c, b'%' | b'?' | b'#' | b' ') || c < 0x20 || c >= 0x80 {
            uri.push('%');
            uri.push(HEX[(c >> 4) as usize] as char);
            uri.push(HEX[(c & 0x0F) as usize] as char);
        } else {
            uri.push(c as char);
        }
    }
    uri.push_str("?mode=ro");
    uri
}

const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;

fn now_filetime() -> i64 {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    (ns / 100) + EPOCH_DIFF_100NS
}

fn filetime_to_unix_ms(ticks: i64) -> u64 {
    if ticks <= EPOCH_DIFF_100NS {
        0
    } else {
        ((ticks - EPOCH_DIFF_100NS) / 10_000) as u64
    }
}

fn db_stamp(db: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    for p in [db, wal.as_path()] {
        if let Ok(meta) = fs::metadata(p) {
            meta.len().hash(&mut hasher);
            if let Ok(m) = meta.modified() {
                if let Ok(d) = m.duration_since(UNIX_EPOCH) {
                    d.as_nanos().hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

// ---------- Minimal toast XML parser ----------

struct ParsedToast {
    title: String,
    body: String,
    launch: String,
    image: Option<PathBuf>,
}

fn decode_payload(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&u16s));
    }
    let slice = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { &bytes[3..] } else { bytes };
    Some(String::from_utf8_lossy(slice).into_owned())
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            if semi <= 12 {
                let ent = &after[..semi];
                let decoded = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if ent.starts_with('#') => parse_num_entity(&ent[1..]),
                    _ => None,
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    rest = &after[semi + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn parse_num_entity(s: &str) -> Option<char> {
    let (radix, digits) = if let Some(h) = s.strip_prefix('x').or_else(|| s.strip_prefix('X')) {
        (16, h)
    } else {
        (10, s)
    };
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let cp = u32::from_str_radix(digits, radix).ok()?;
    if cp == 0 {
        return None;
    }
    char::from_u32(cp)
}

fn is_tag(xml: &str, pos: usize, name: &str) -> bool {
    if !xml[pos..].starts_with(name) {
        return false;
    }
    match xml[pos + name.len()..].as_bytes().first() {
        Some(&b) => b == b'>' || b == b'/' || b.is_ascii_whitespace(),
        None => false,
    }
}

fn start_tag(xml: &str, pos: usize) -> &str {
    match xml[pos..].find('>') {
        Some(end) => &xml[pos..pos + end + 1],
        None => &xml[pos..],
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut offset = 0;
    while let Some(rel) = tag[offset..].find(name) {
        let p = offset + rel;
        let prev_ok = p > 0 && tag.as_bytes()[p - 1].is_ascii_whitespace();
        let after = p + name.len();
        offset = after;
        if !prev_ok {
            continue;
        }
        let rest = tag[after..].trim_start();
        let Some(after_eq) = rest.strip_prefix('=') else { continue };
        let after_ws = after_eq.trim_start();
        let quote = after_ws.as_bytes().first().copied()?;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let inner = &after_ws[1..];
        let end = inner.find(quote as char)?;
        return Some(&inner[..end]);
    }
    None
}

fn select_binding(xml: &str) -> &str {
    let mut first: Option<usize> = None;
    let mut offset = 0;
    while let Some(rel) = xml[offset..].find("<binding") {
        let p = offset + rel;
        offset = p + 8;
        if !is_tag(xml, p, "<binding") {
            continue;
        }
        if first.is_none() {
            first = Some(p);
        }
        if attr(start_tag(xml, p), "template")
            .is_some_and(|t| t.contains("ToastGeneric"))
        {
            first = Some(p);
            break;
        }
    }
    let Some(start) = first else { return xml };
    match xml[start..].find("</binding>") {
        Some(end) => &xml[start..start + end],
        None => &xml[start..],
    }
}

fn extract_texts(binding: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0;
    while let Some(rel) = binding[offset..].find("<text") {
        let p = offset + rel;
        offset = p + 5;
        if !is_tag(binding, p, "<text") {
            continue;
        }
        let tag = start_tag(binding, p);
        if tag.ends_with("/>") {
            continue;
        }
        let body_start = p + tag.len();
        let Some(rel_end) = binding[body_start..].find("</text>") else { break };
        let raw = &binding[body_start..body_start + rel_end];
        let text = if let Some(inner) = raw.strip_prefix("<![CDATA[").and_then(|s| s.strip_suffix("]]>")) {
            inner.trim().to_string()
        } else {
            decode_entities(raw).trim().to_string()
        };
        if !text.is_empty() {
            out.push(text);
        }
        offset = body_start + rel_end + 7;
    }
    out
}

fn cap_chars(s: String, cap: usize) -> String {
    let mut iter = s.char_indices();
    if let Some((idx, _)) = iter.nth(cap) {
        let mut out = s[..idx].to_string();
        out.push('…');
        out
    } else {
        s
    }
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve_image(src: &str, aumid: &str) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    let lower = src.to_ascii_lowercase();
    let candidate = if lower.starts_with("file:///") {
        let decoded = url_decode(&src[8..]).replace('/', "\\");
        PathBuf::from(decoded)
    } else if lower.starts_with("ms-appdata:///local/") {
        let family = aumid.split('!').next().filter(|f| !f.is_empty() && aumid.contains('!'))?;
        let rel_raw = &src[20..];
        let rel_cut = rel_raw.split(['?', '#']).next().unwrap_or(rel_raw);
        let rel = url_decode(rel_cut);
        if rel.contains("..") {
            return None;
        }
        let rel = rel.replace('/', "\\");
        let rel = rel.trim_start_matches('\\');
        if rel.is_empty() {
            return None;
        }
        let local = std::env::var_os("LOCALAPPDATA")?;
        PathBuf::from(local).join("Packages").join(family).join("LocalState").join(rel)
    } else if src.len() > 2 && src.as_bytes()[1] == b':' && matches!(src.as_bytes()[2], b'\\' | b'/') {
        PathBuf::from(src)
    } else {
        return None;
    };
    candidate.is_file().then_some(candidate)
}

fn logo_src(binding: &str) -> Option<&str> {
    let mut offset = 0;
    while let Some(rel) = binding[offset..].find("<image") {
        let p = offset + rel;
        offset = p + 6;
        if !is_tag(binding, p, "<image") {
            continue;
        }
        let tag = start_tag(binding, p);
        if attr(tag, "placement") == Some("appLogoOverride") {
            return attr(tag, "src");
        }
    }
    None
}

fn parse_toast_xml(xml: &str, aumid: &str) -> Option<ParsedToast> {
    let mut offset = 0;
    let toast_pos = loop {
        let rel = xml[offset..].find("<toast")?;
        let p = offset + rel;
        offset = p + 6;
        if is_tag(xml, p, "<toast") {
            break p;
        }
    };
    let launch = attr(start_tag(xml, toast_pos), "launch")
        .map(decode_entities)
        .unwrap_or_default();
    let binding = select_binding(&xml[toast_pos..]);
    let texts = extract_texts(binding);
    if texts.is_empty() {
        return None;
    }
    let title = cap_chars(texts[0].clone(), TITLE_CAP);
    let mut body = String::new();
    for t in &texts[1..] {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(t);
        if body.chars().count() >= BODY_CAP {
            break;
        }
    }
    let body = cap_chars(body, BODY_CAP);
    let image = logo_src(binding)
        .map(decode_entities)
        .and_then(|s| resolve_image(&s, aumid));
    Some(ParsedToast { title, body, launch, image })
}

// ---------- App name & launch helpers ----------

fn fallback_app_name(aumid: &str) -> String {
    if aumid.eq_ignore_ascii_case("Windows.Defender.SecurityCenter") {
        return "Windows Security".to_string();
    }
    if aumid.len() >= 20 && aumid[..20].eq_ignore_ascii_case("Windows.SystemToast.") {
        return "Windows".to_string();
    }
    let mut s = if let Some((pkg, _)) = aumid.split_once('!') {
        pkg.rsplit_once('_').map(|(left, _)| left).unwrap_or(pkg)
    } else if let Some((_, file)) = aumid.rsplit_once(['\\', '/']) {
        let stem = file.rsplit_once('.').map(|(st, _)| st).filter(|st| !st.is_empty()).unwrap_or(file);
        return if stem.is_empty() { aumid.to_string() } else { stem.to_string() };
    } else {
        aumid
    };
    if let Some((_, last)) = s.rsplit_once('.') {
        if !last.is_empty() {
            s = last;
        }
    }
    if s.is_empty() { aumid.to_string() } else { s.to_string() }
}

fn shell_display_name(aumid: &str) -> Option<String> {
    if aumid.is_empty() {
        return None;
    }
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let target = HSTRING::from(format!("shell:AppsFolder\\{aumid}"));
        let item: Option<IShellItem> = SHCreateItemFromParsingName(&target, None).ok();
        let name = item.and_then(|it| {
            let pwstr = it.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
            let s = String::from_utf16_lossy(pwstr.as_wide()).trim().to_string();
            CoTaskMemFree(Some(pwstr.0 as _));
            (!s.is_empty()).then_some(s)
        });
        if com {
            CoUninitialize();
        }
        name
    }
}

fn app_name_of(aumid: &str) -> String {
    if aumid.is_empty() {
        return String::new();
    }
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = cache.lock() {
        if let Some(hit) = g.get(aumid) {
            return hit.clone();
        }
    }
    let name = shell_display_name(aumid).unwrap_or_else(|| fallback_app_name(aumid));
    if let Ok(mut g) = cache.lock() {
        if g.len() >= NAME_CACHE_MAX {
            g.clear();
        }
        g.insert(aumid.to_string(), name.clone());
    }
    name
}

fn icon_target_of(aumid: &str) -> Option<String> {
    if aumid.is_empty() {
        return None;
    }
    if (aumid.contains('\\') || aumid.contains('/')) && Path::new(aumid).is_file() {
        return Some(format!("path:{aumid}"));
    }
    Some(format!("aumid:{aumid}"))
}

fn looks_like_safe_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else { return false };
    if scheme.len() < 2 || rest.is_empty() {
        return false;
    }
    if s.chars().any(char::is_whitespace) {
        return false;
    }
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return false;
    }
    let lower = scheme.to_ascii_lowercase();
    lower != "file" && lower != "shell"
}

fn shell_open(target: &str) -> bool {
    unsafe {
        let res = ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        (res.0 as isize) > 32
    }
}

fn open_target(aumid: &str, launch: &str) -> bool {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    if looks_like_safe_uri(launch) && shell_open(launch) {
        return true;
    }
    if aumid.is_empty() {
        return false;
    }
    if (aumid.contains('\\') || aumid.contains('/')) && Path::new(aumid).is_file() {
        return shell_open(aumid);
    }
    shell_open(&format!("shell:AppsFolder\\{aumid}"))
}

// ---------- SQLite query ----------

fn query_notifications(app: &AppHandle, max: usize) -> Option<Vec<RawNotif>> {
    let path = db_path()?;
    if !path.is_file() {
        return Some(Vec::new());
    }
    let uri = file_uri(&path);
    let db = Db::open_readonly(&uri)?;
    let stmt = Stmt::prepare(&db, NOTIF_QUERY)?;
    let limit = (max.saturating_mul(2)).min(MAX_QUERY_ROWS) as i64;
    unsafe {
        if (stmt.api.bind_int64)(stmt.ptr, 1, now_filetime()) != SQLITE_OK {
            return None;
        }
        if (stmt.api.bind_int64)(stmt.ptr, 2, limit) != SQLITE_OK {
            return None;
        }
    }

    let scope = app.asset_protocol_scope();
    let mut out = Vec::new();
    let mut last_rc = SQLITE_DONE;
    while out.len() < max {
        let rc = unsafe { (stmt.api.step)(stmt.ptr) };
        last_rc = rc;
        if rc != SQLITE_ROW {
            break;
        }
        let id = unsafe { (stmt.api.column_int64)(stmt.ptr, 0) };
        let aumid = stmt.col_text(1);
        let arrived_ticks = unsafe { (stmt.api.column_int64)(stmt.ptr, 2) };
        let payload = stmt.col_blob(3);
        let Some(xml) = decode_payload(payload) else { continue };
        let Some(parsed) = parse_toast_xml(&xml, &aumid) else { continue };
        let image = parsed.image.map(|p| {
            let _ = scope.allow_file(&p);
            p.to_string_lossy().into_owned()
        });
        out.push(RawNotif {
            id,
            app: app_name_of(&aumid),
            icon_target: icon_target_of(&aumid),
            aumid,
            title: parsed.title,
            body: parsed.body,
            arrived_ms: filetime_to_unix_ms(arrived_ticks),
            launch: parsed.launch,
            image,
        });
    }
    if last_rc != SQLITE_DONE && last_rc != SQLITE_ROW {
        return None;
    }
    Some(out)
}

// ---------- Persistence & crash-safe native banner suppression ----------

fn state_file_path(dir: &Path) -> PathBuf {
    dir.join("notif-state.json")
}

fn restore_marker_path(dir: &Path) -> PathBuf {
    dir.join("notif-restore.json")
}

fn load_persisted(dir: &Path) -> PersistedState {
    fs::read_to_string(state_file_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_persisted(dir: &Path, read_ids: &HashSet<i64>, orumi_dnd: bool) {
    let _ = fs::create_dir_all(dir);
    let mut ids: Vec<i64> = read_ids.iter().copied().collect();
    ids.sort_unstable();
    let payload = PersistedState { read_ids: ids, orumi_dnd };
    if let Ok(text) = serde_json::to_string(&payload) {
        let path = state_file_path(dir);
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn read_toast_enabled_reg() -> Option<u32> {
    let mut val = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PUSH_KEY,
            TOAST_VALUE,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut val as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(val)
}

fn write_toast_enabled_reg(val: Option<u32>) {
    unsafe {
        match val {
            Some(v) => {
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    PUSH_KEY,
                    TOAST_VALUE,
                    REG_DWORD.0,
                    Some(&v as *const u32 as *const c_void),
                    std::mem::size_of::<u32>() as u32,
                );
            }
            None => {
                let rc = RegDeleteKeyValueW(HKEY_CURRENT_USER, PUSH_KEY, TOAST_VALUE);
                let _ = rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND;
            }
        }
    }
}

fn read_restore_marker(path: &Path) -> Option<RestoreMarker> {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn restore_marker_at(dir: &Path) {
    SUPPRESSION_ACTIVE.store(false, Ordering::SeqCst);
    let path = restore_marker_path(dir);
    if let Some(marker) = read_restore_marker(&path) {
        write_toast_enabled_reg(marker.original_toast_enabled);
        let _ = fs::remove_file(&path);
    }
}

pub fn apply_suppression(app: &AppHandle, suppress: bool) {
    let Some(dir) = app.path().app_config_dir().ok() else { return };
    let _ = CONFIG_DIR.set(dir.clone());

    if let Some(state) = app.try_state::<NotifState>() {
        if let Ok(mut g) = state.0.lock() {
            g.suppress_native = suppress;
        }
    }

    if !suppress {
        restore_marker_at(&dir);
        return;
    }

    let marker_path = restore_marker_path(&dir);
    // Keep existing marker on crash recovery so the true original value is never overwritten with 0.
    if !marker_path.exists() {
        let original = read_toast_enabled_reg();
        let _ = fs::create_dir_all(&dir);
        if let Ok(text) = serde_json::to_string(&RestoreMarker {
            original_toast_enabled: original,
        }) {
            if fs::write(&marker_path, text).is_err() {
                return;
            }
        } else {
            return;
        }
    }
    SUPPRESSION_ACTIVE.store(true, Ordering::SeqCst);
    write_toast_enabled_reg(Some(0));
}

pub fn apply_config(app: &AppHandle, enabled: bool, popup: bool, suppress_native: bool) {
    if let Some(state) = app.try_state::<NotifState>() {
        if let Ok(mut g) = state.0.lock() {
            g.enabled = enabled;
            g.popup_enabled = popup;
        }
    }
    apply_suppression(app, enabled && suppress_native);
    if enabled {
        sync_once(app, false);
    } else if let Some(state) = app.try_state::<NotifState>() {
        if let Ok(mut g) = state.0.lock() {
            g.items.clear();
        }
        let _ = app.emit_to("notch", "notif-changed", Vec::<NotifEntry>::new());
    }
}

/// Called on normal app exit.
pub fn restore(app: &AppHandle) {
    ACTIVE.store(false, Ordering::SeqCst);
    if let Ok(dir) = app.path().app_config_dir() {
        restore_marker_at(&dir);
    }
}

/// Called by `--guard` when the parent process terminates unexpectedly.
pub fn restore_from_guard(config_dir: &Path) {
    restore_marker_at(config_dir);
}

// ---------- DND coordination with quick.rs ----------

pub fn suppression_active() -> bool {
    SUPPRESSION_ACTIVE.load(Ordering::Relaxed)
}

pub fn is_dnd() -> bool {
    ORUMI_DND.load(Ordering::Relaxed)
}

pub fn set_dnd(app: &AppHandle, on: bool) {
    ORUMI_DND.store(on, Ordering::Relaxed);
    if let Some(state) = app.try_state::<NotifState>() {
        if let Ok(mut g) = state.0.lock() {
            g.orumi_dnd = on;
            if let Some(dir) = CONFIG_DIR.get() {
                save_persisted(dir, &g.read_ids, g.orumi_dnd);
            }
        }
    }
}

fn effective_dnd() -> bool {
    if suppression_active() {
        is_dnd()
    } else {
        read_toast_enabled_reg() == Some(0)
    }
}

// ---------- Sync & Watcher ----------

fn sync_once(app: &AppHandle, emit_popups: bool) {
    let Some(state) = app.try_state::<NotifState>() else { return };
    let enabled = state.0.lock().map(|g| g.enabled).unwrap_or(true);
    if !enabled {
        return;
    }

    let Some(fresh) = query_notifications(app, READ_MAX) else { return };
    let mut new_toasts: Vec<NotifEntry> = Vec::new();
    let mut changed_items: Option<Vec<NotifEntry>> = None;
    let mut should_persist = false;

    if let Ok(mut g) = state.0.lock() {
        let active_set: HashSet<i64> = fresh.iter().map(|n| n.id).collect();
        let old_len = g.read_ids.len();
        g.read_ids.retain(|id| active_set.contains(id));
        if g.read_ids.len() != old_len {
            should_persist = true;
        }

        let max_id = fresh.iter().map(|n| n.id).max().unwrap_or(0);
        let prev_high = g.high_water_id;
        if prev_high.is_none() {
            g.high_water_id = Some(max_id);
        } else if let Some(hw) = prev_high {
            if max_id > hw {
                g.high_water_id = Some(max_id);
            }
        }

        if emit_popups && g.popup_enabled && !effective_dnd() {
            if let Some(hw) = prev_high {
                for n in fresh.iter().rev() {
                    if n.id > hw && !g.read_ids.contains(&n.id) {
                        new_toasts.push(n.to_entry());
                    }
                }
            }
        }

        g.active_ids = fresh.iter().map(|n| n.id).collect();
        g.launch_map.clear();
        let mut unread = Vec::new();
        for n in &fresh {
            g.launch_map.insert(n.id, (n.aumid.clone(), n.launch.clone()));
            if !g.read_ids.contains(&n.id) {
                unread.push(n.to_entry());
            }
        }

        let prev_ids: Vec<i64> = g.items.iter().map(|x| x.id).collect();
        let next_ids: Vec<i64> = unread.iter().map(|x| x.id).collect();
        if prev_ids != next_ids {
            g.items = unread.clone();
            changed_items = Some(unread);
        }

        if should_persist {
            if let Some(dir) = CONFIG_DIR.get() {
                save_persisted(dir, &g.read_ids, g.orumi_dnd);
            }
        }
    }

    if let Some(items) = changed_items {
        let _ = app.emit_to("notch", "notif-changed", &items);
    }
    for toast in new_toasts {
        let _ = app.emit_to("notch", "notif-toast", &toast);
    }
}

pub fn start(app: AppHandle) {
    let cfg = crate::config::current(&app);
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = CONFIG_DIR.set(dir.clone());
        let saved = load_persisted(&dir);
        ORUMI_DND.store(saved.orumi_dnd, Ordering::Relaxed);
        if let Some(state) = app.try_state::<NotifState>() {
            if let Ok(mut g) = state.0.lock() {
                g.read_ids = saved.read_ids.into_iter().collect();
                g.orumi_dnd = saved.orumi_dnd;
                g.enabled = cfg.notifications_enabled;
                g.popup_enabled = cfg.notifications_popup;
                g.suppress_native = cfg.notifications_suppress_native;
            }
        }
    }

    apply_suppression(&app, cfg.notifications_enabled && cfg.notifications_suppress_native);

    thread::spawn(move || {
        sync_once(&app, false);
        let Some(db) = db_path() else { return };
        let Some(dir) = db.parent().map(Path::to_path_buf) else { return };
        let mut last_stamp = db_stamp(&db);

        let mut wide: Vec<u16> = dir.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let raw_handle = unsafe {
            FindFirstChangeNotificationW(
                wide.as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
            )
        };
        let handle = if raw_handle.is_null() || raw_handle == INVALID_HANDLE_VALUE.0 {
            None
        } else {
            Some(HANDLE(raw_handle))
        };

        while ACTIVE.load(Ordering::Relaxed) {
            if let Some(h) = handle {
                let wait = unsafe { WaitForSingleObject(h, POLL_MS) };
                if !ACTIVE.load(Ordering::Relaxed) {
                    break;
                }
                if wait == WAIT_OBJECT_0 {
                    unsafe {
                        let _ = FindNextChangeNotification(h.0);
                    }
                    thread::sleep(Duration::from_millis(SETTLE_MS));
                }
            } else {
                thread::sleep(Duration::from_millis(POLL_MS as u64));
            }

            if !ACTIVE.load(Ordering::Relaxed) {
                break;
            }
            let stamp = db_stamp(&db);
            if stamp != last_stamp {
                last_stamp = stamp;
                sync_once(&app, true);
            }
        }

        if let Some(h) = handle {
            unsafe {
                let _ = FindCloseChangeNotification(h.0);
            }
        }
    });
}

// ---------- Tauri commands ----------

#[tauri::command]
pub fn notif_list(app: AppHandle, state: State<'_, NotifState>) -> Vec<NotifEntry> {
    if state.0.lock().map(|g| g.high_water_id.is_none()).unwrap_or(false) {
        sync_once(&app, false);
    }
    state.0.lock().map(|g| g.items.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn notif_mark_read(id: i64, app: AppHandle, state: State<'_, NotifState>) {
    let next = {
        let Ok(mut g) = state.0.lock() else { return };
        g.read_ids.insert(id);
        g.items.retain(|x| x.id != id);
        if let Some(dir) = CONFIG_DIR.get() {
            save_persisted(dir, &g.read_ids, g.orumi_dnd);
        }
        g.items.clone()
    };
    let _ = app.emit_to("notch", "notif-changed", &next);
}

#[tauri::command]
pub fn notif_clear_all(app: AppHandle, state: State<'_, NotifState>) {
    let next = {
        let Ok(mut g) = state.0.lock() else { return };
        for id in g.active_ids.clone() {
            g.read_ids.insert(id);
        }
        g.items.clear();
        if let Some(dir) = CONFIG_DIR.get() {
            save_persisted(dir, &g.read_ids, g.orumi_dnd);
        }
        g.items.clone()
    };
    let _ = app.emit_to("notch", "notif-changed", &next);
}

#[tauri::command]
pub fn notif_open(id: i64, app: AppHandle, state: State<'_, NotifState>) -> bool {
    let target = state
        .0
        .lock()
        .ok()
        .and_then(|g| g.launch_map.get(&id).cloned());
    let opened = target
        .map(|(aumid, launch)| open_target(&aumid, &launch))
        .unwrap_or(false);
    notif_mark_read(id, app, state);
    opened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toast_generic_with_entities_and_cdata() {
        let xml = r#"<toast launch="https://example.com/chat?id=4">
            <visual>
                <binding template="ToastGeneric">
                    <text>Alice &amp; Bob</text>
                    <text><![CDATA[Hello <world>]]></text>
                    <text>Line &#x1F600;</text>
                </binding>
            </visual>
        </toast>"#;
        let parsed = parse_toast_xml(xml, "Test.App!App").expect("should parse");
        assert_eq!(parsed.launch, "https://example.com/chat?id=4");
        assert_eq!(parsed.title, "Alice & Bob");
        assert_eq!(parsed.body, "Hello <world>\nLine 😀");
    }

    #[test]
    fn rejects_empty_toast() {
        let xml = r#"<toast><visual><binding template="ToastGeneric"><text>   </text></binding></visual></toast>"#;
        assert!(parse_toast_xml(xml, "Test.App!App").is_none());
    }

    #[test]
    fn blocks_unsafe_launch_schemes() {
        assert!(looks_like_safe_uri("https://example.com/a"));
        assert!(looks_like_safe_uri("ms-settings:notifications"));
        assert!(!looks_like_safe_uri("file:///C:/Windows/notepad.exe"));
        assert!(!looks_like_safe_uri("shell:AppsFolder\\Test"));
        assert!(!looks_like_safe_uri("C:\\Windows\\notepad.exe"));
        assert!(!looks_like_safe_uri("action=reply id=4"));
    }

    #[test]
    fn blocks_path_traversal_in_image_uri() {
        assert!(resolve_image("ms-appdata:///local/../../secret.png", "Pkg_123!App").is_none());
        assert!(resolve_image("https://example.com/avatar.png", "Pkg_123!App").is_none());
    }

    #[test]
    fn fallback_names_are_readable() {
        assert_eq!(fallback_app_name("Windows.Defender.SecurityCenter"), "Windows Security");
        assert_eq!(fallback_app_name("Windows.SystemToast.Bluetooth"), "Windows");
        assert_eq!(fallback_app_name("Microsoft.ScreenSketch_8wekyb3d8bbwe!App"), "ScreenSketch");
        assert_eq!(fallback_app_name(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"), "powershell");
    }
}
