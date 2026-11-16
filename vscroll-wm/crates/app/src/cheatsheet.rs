//! Cheatsheet: daftar pintasan keyboard dalam panel di tengah layar (Win+/).
//!
//! Daftarnya DIHASILKAN dari tabel aksi (bindings.rs) dan konfigurasi yang sedang
//! berlaku, jadi tombol hasil `bind.*` / overview_key / resize_step selalu tertulis benar.
//! Aksi baru cukup ditambah di `bindings::ACTIONS`; yang perlu diperbarui di sini hanya
//! baris tetap (yang tidak bisa diubah): `fixed_rows()`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use layout::Rect;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, HGDIOBJ, LOGFONTW, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bindings::{self, Chord};
use crate::config::{rgb, Config, HeightMode, OverviewColors};
use crate::overview::luminance;

const TITLE_TEXT: &str = "Keyboard shortcuts";
const FOOTER_TEXT: &str = "press any key, or click, to close";

const PAD: i32 = 28;
/// Lebar kolom tombol: minimal ini, melebar sesuai teks terpanjang sampai KEYS_W_MAX.
const KEYS_W: i32 = 250;
const KEYS_W_MAX: i32 = 300;
const DESC_W: i32 = 520;
const COL_GAP: i32 = 48;
const ROW_H: i32 = 34;
const HEADER_H: i32 = 46;
const GROUP_GAP: i32 = 12;
const TITLE_H: i32 = 66;
const FOOTER_H: i32 = 50;

pub struct Group {
    pub title: String,
    /// (tombol, keterangan)
    pub rows: Vec<(String, String)>,
}

static SUPPRESSED: AtomicBool = AtomicBool::new(false);

pub fn is_suppressed() -> bool {
    SUPPRESSED.load(Ordering::Relaxed)
}

pub unsafe fn set_suppressed(on: bool) {
    SUPPRESSED.store(on, Ordering::Relaxed);
    if on {
        close(false);
    }
}

pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn groups_json(cfg: &Config) -> String {
    let list = groups(cfg);
    let mut out = String::from("[");
    for (gi, g) in list.iter().enumerate() {
        if gi > 0 {
            out.push(',');
        }
        out.push_str(&format!(r#"{{"title":"{}","rows":["#, json_escape(&g.title)));
        for (ri, (keys, desc)) in g.rows.iter().enumerate() {
            if ri > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"keys":"{}","desc":"{}"}}"#,
                json_escape(keys),
                json_escape(desc)
            ));
        }
        out.push_str("]}");
    }
    out.push(']');
    out
}

/// Isi keterangan: placeholder diganti nilai konfigurasi.
fn describe(desc: &str, cfg: &Config) -> String {
    let height_note = if cfg.height_mode == HeightMode::Step {
        format!(" (±{} px)", cfg.height_step)
    } else {
        String::new()
    };
    desc.replace("{resize_step}", &cfg.resize_step.to_string())
        .replace("{height_note}", &height_note)
}

/// Tambah satu baris ke kelompok berjudul `group` (dibuat di akhir kalau belum ada).
fn push_row(out: &mut Vec<Group>, group: &str, keys: String, desc: String) {
    match out.iter_mut().find(|g| g.title == group) {
        Some(g) => g.rows.push((keys, desc)),
        None => out.push(Group {
            title: group.to_string(),
            rows: vec![(keys, desc)],
        }),
    }
}

/// Baris yang tidak bisa diubah lewat `bind.*`: tombol terkunci, tombol di dalam
/// overview, dan cadangan. (kelompok, tombol, keterangan)
fn fixed_rows(cfg: &Config) -> Vec<(&'static str, String, String)> {
    let row = |g: &'static str, k: &str, d: &str| (g, k.to_string(), d.to_string());
    let cycle = cfg.bindings.overview_cycle_vk();
    let cycle_keys = if cycle != 0 && cycle != 0x09 {
        format!("Tab / Win + {}", bindings::vk_label(cycle))
    } else {
        "Tab".to_string()
    };
    let select = if cfg.overview_wrap {
        "select a window (wraps around at the ends)"
    } else {
        "select a window"
    };
    let mut rows = vec![
        row(
            bindings::G_WORKSPACE,
            "Win + Ctrl + D / F4",
            "disabled (workspaces replace virtual desktops)",
        ),
        row(
            bindings::G_APP,
            "Win + Ctrl + Esc",
            "quit and restore all windows",
        ),
        row(
            bindings::G_APP,
            "Ctrl + Alt + …",
            "backup copies of snap, focus, Enter, F, I, Space",
        ),
        row(
            bindings::G_APP,
            "Ctrl + Alt + Shift + Q",
            "quit and restore all windows (backup)",
        ),
        row(
            bindings::G_APP,
            "tray icon",
            "pause, reload config, run at login, quit",
        ),
        row(bindings::G_OVERVIEW, "Arrow keys", select),
        row(
            bindings::G_OVERVIEW,
            "← / → at an edge",
            "continue into the next / previous workspace",
        ),
        row(
            bindings::G_OVERVIEW,
            &cycle_keys,
            "next window in this workspace (+ Shift: previous)",
        ),
        row(
            bindings::G_OVERVIEW,
            "Enter / release Win",
            "jump to the selected window",
        ),
        row(bindings::G_OVERVIEW, "Esc / click outside", "cancel"),
        row(bindings::G_OVERVIEW, "Click a window", "jump to it"),
        row(
            bindings::G_OVERVIEW,
            "Ctrl + ← / → , 1 … 9",
            "select a workspace",
        ),
    ];
    if cfg.mouse_drag {
        rows.push(row(
            bindings::G_LAYOUT,
            "Drag onto a window",
            "swap places with that window",
        ));
        rows.push(row(
            bindings::G_LAYOUT,
            "Drag an inner edge",
            "move the column divider (edge next to it)",
        ));
        rows.push(row(
            bindings::G_OVERVIEW,
            "Drag a window",
            "drop on a workspace column to move it",
        ));
        rows.push(row(
            bindings::G_OVERVIEW,
            "Drop onto a window",
            "split it in half (side by side, or stacked)",
        ));
        rows.push(row(
            bindings::G_OVERVIEW,
            "Drop at top / bottom",
            "full width, as a block separator",
        ));
    }
    rows
}

/// Semua pintasan, dikelompokkan, sesuai konfigurasi sekarang. Baris bisa diikat
/// dihasilkan dari `bindings::ACTIONS`; aksi se-kelompok dengan keterangan sama digabung
/// jadi satu baris, tombol ke-2 dst. dari satu aksi jadi baris "(alternate)".
pub fn groups(cfg: &Config) -> Vec<Group> {
    // (kelompok, keterangan, indeks aksi) dengan urutan kemunculan di tabel
    let mut specs: Vec<(&str, &str, Vec<usize>)> = Vec::new();
    for (i, a) in bindings::ACTIONS.iter().enumerate() {
        match specs.iter_mut().find(|s| s.0 == a.group && s.1 == a.desc) {
            Some(s) => s.2.push(i),
            None => specs.push((a.group, a.desc, vec![i])),
        }
    }

    let mut out: Vec<Group> = Vec::new();
    for (group, desc, idxs) in specs {
        let slots = idxs
            .iter()
            .map(|&i| cfg.bindings.chords(i).len())
            .max()
            .unwrap_or(0);
        let text = describe(desc, cfg);
        if slots == 0 {
            // tanpa tombol: hanya cheatsheet yang tetap ditampilkan (lewat menu tray)
            if idxs.iter().any(|&i| bindings::ACTIONS[i].name == "cheatsheet") {
                push_row(&mut out, group, "tray menu".to_string(), text);
            }
            continue;
        }
        for slot in 0..slots {
            let chords: Vec<Chord> = idxs
                .iter()
                .filter_map(|&i| cfg.bindings.chords(i).get(slot).copied())
                .collect();
            let shown = if slot == 0 {
                text.clone()
            } else {
                format!("{text} (alternate)")
            };
            push_row(&mut out, group, bindings::join_chords(&chords), shown);
        }
    }
    for (group, keys, desc) in fixed_rows(cfg) {
        push_row(&mut out, group, keys, desc);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Title,
    Header,
    /// Garis tipis di bawah judul kelompok.
    Rule,
    Keys,
    Desc,
    Footer,
}

pub struct Cell {
    pub kind: CellKind,
    pub text: String,
    /// Relatif terhadap sudut kiri atas panel.
    pub rect: RECT,
}

pub struct Sheet {
    pub panel: Rect,
    pub columns: i32,
    pub cells: Vec<Cell>,
}

fn group_height(g: &Group) -> i32 {
    HEADER_H + g.rows.len() as i32 * ROW_H + GROUP_GAP
}

/// Bagi kelompok ke `n` kolom (yang terpendek dulu). Mengembalikan tinggi tiap kolom
/// dan isinya (indeks kelompok).
fn assign(groups: &[Group], n: i32) -> (Vec<i32>, Vec<Vec<usize>>) {
    let n = n.max(1) as usize;
    let mut heights = vec![0; n];
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, g) in groups.iter().enumerate() {
        let col = (0..n).min_by_key(|&c| heights[c]).unwrap_or(0);
        heights[col] += group_height(g);
        members[col].push(i);
    }
    (heights, members)
}

/// Hitung panel dan posisi semua teks. Dua kolom kalau lebar layar cukup
/// (tampil lebih ringkas); satu kolom kalau tidak.
pub fn compute(groups: &[Group], work: Rect) -> Sheet {
    let longest = groups
        .iter()
        .flat_map(|g| g.rows.iter())
        .map(|(k, _)| k.chars().count() as i32)
        .max()
        .unwrap_or(0);
    let keys_w = (longest * 9 + 20).clamp(KEYS_W, KEYS_W_MAX);
    let col_w = keys_w + DESC_W;
    let max_w = work.w * 92 / 100;
    let max_h = work.h * 92 / 100;
    let by_width =
        ((max_w - 2 * PAD + COL_GAP) / (col_w + COL_GAP)).clamp(1, groups.len().max(1) as i32);

    let mut columns = by_width;
    for n in by_width.min(2)..=by_width {
        let (heights, _) = assign(groups, n);
        let h = TITLE_H + heights.iter().copied().max().unwrap_or(0) + FOOTER_H + PAD;
        if h <= max_h {
            columns = n;
            break;
        }
    }

    let (heights, members) = assign(groups, columns);
    let body_h = heights.iter().copied().max().unwrap_or(0);
    let panel_w = columns * col_w + (columns - 1) * COL_GAP + 2 * PAD;
    let panel_h = TITLE_H + body_h + FOOTER_H + PAD;
    let panel = Rect {
        x: work.x + (work.w - panel_w) / 2,
        y: work.y + (work.h - panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    let mut cells = vec![Cell {
        kind: CellKind::Title,
        text: TITLE_TEXT.to_string(),
        rect: RECT {
            left: PAD,
            top: 14,
            right: panel_w - PAD,
            bottom: TITLE_H - 6,
        },
    }];
    for (c, idxs) in members.iter().enumerate() {
        let x0 = PAD + c as i32 * (col_w + COL_GAP);
        let mut y = TITLE_H;
        for &gi in idxs {
            let g = &groups[gi];
            cells.push(Cell {
                kind: CellKind::Header,
                text: g.title.clone(),
                rect: RECT {
                    left: x0,
                    top: y,
                    right: x0 + col_w,
                    bottom: y + HEADER_H - 8,
                },
            });
            cells.push(Cell {
                kind: CellKind::Rule,
                text: String::new(),
                rect: RECT {
                    left: x0,
                    top: y + HEADER_H - 8,
                    right: x0 + col_w,
                    bottom: y + HEADER_H - 6,
                },
            });
            y += HEADER_H;
            for (keys, desc) in &g.rows {
                cells.push(Cell {
                    kind: CellKind::Keys,
                    text: keys.clone(),
                    rect: RECT {
                        left: x0,
                        top: y,
                        right: x0 + keys_w - 12,
                        bottom: y + ROW_H,
                    },
                });
                cells.push(Cell {
                    kind: CellKind::Desc,
                    text: desc.clone(),
                    rect: RECT {
                        left: x0 + keys_w,
                        top: y,
                        right: x0 + col_w,
                        bottom: y + ROW_H,
                    },
                });
                y += ROW_H;
            }
            y += GROUP_GAP;
        }
    }
    cells.push(Cell {
        kind: CellKind::Footer,
        text: FOOTER_TEXT.to_string(),
        rect: RECT {
            left: PAD,
            top: TITLE_H + body_h,
            right: panel_w - PAD,
            bottom: TITLE_H + body_h + FOOTER_H,
        },
    });
    Sheet {
        panel,
        columns,
        cells,
    }
}

// ------------------------------------------------------------------- jendela

static SHEET: Mutex<Option<(Sheet, OverviewColors)>> = Mutex::new(None);
static CS_HWND: AtomicIsize = AtomicIsize::new(0);
static FONT_NORMAL: AtomicIsize = AtomicIsize::new(0);
static FONT_BOLD: AtomicIsize = AtomicIsize::new(0);
static FONT_TITLE: AtomicIsize = AtomicIsize::new(0);
static PREV_FG: AtomicIsize = AtomicIsize::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(false);

fn hwnd() -> HWND {
    HWND(CS_HWND.load(Ordering::Relaxed) as *mut _)
}

pub fn available() -> bool {
    CS_HWND.load(Ordering::Relaxed) != 0
}

unsafe fn make_font(height: i32, weight: i32) -> isize {
    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = height;
    lf.lfWeight = weight;
    for (i, ch) in "Segoe UI".encode_utf16().enumerate() {
        lf.lfFaceName[i] = ch;
    }
    CreateFontIndirectW(&lf).0 as isize
}

unsafe fn close(restore_focus: bool) {
    if !VISIBLE.swap(false, Ordering::Relaxed) {
        return;
    }
    let _ = ShowWindow(hwnd(), SW_HIDE);
    if let Ok(mut g) = SHEET.lock() {
        *g = None;
    }
    if restore_focus {
        let prev = HWND(PREV_FG.load(Ordering::Relaxed) as *mut _);
        if !prev.0.is_null() {
            let _ = SetForegroundWindow(prev);
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    if let Ok(guard) = SHEET.lock() {
        if let Some((sheet, colors)) = guard.as_ref() {
            let bg = CreateSolidBrush(COLORREF(colors.background));
            let rule = CreateSolidBrush(COLORREF(colors.box_fill));
            FillRect(hdc, &client, bg);
            SetBkMode(hdc, TRANSPARENT);

            let dark_bg = luminance(colors.background) <= 150;
            let text = if dark_bg {
                rgb(232, 235, 243)
            } else {
                rgb(25, 28, 36)
            };
            let dim = if dark_bg {
                rgb(140, 148, 166)
            } else {
                rgb(100, 106, 120)
            };
            let font = |a: &AtomicIsize| HGDIOBJ(a.load(Ordering::Relaxed) as *mut _);

            for cell in &sheet.cells {
                if cell.kind == CellKind::Rule {
                    FillRect(hdc, &cell.rect, rule);
                    continue;
                }
                let (f, color, flags) = match cell.kind {
                    CellKind::Title => (&FONT_TITLE, text, DT_LEFT | DT_VCENTER | DT_SINGLELINE),
                    CellKind::Header => (
                        &FONT_BOLD,
                        colors.view,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
                    ),
                    CellKind::Keys => (
                        &FONT_BOLD,
                        colors.active,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                    ),
                    CellKind::Desc => (
                        &FONT_NORMAL,
                        text,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                    ),
                    _ => (&FONT_NORMAL, dim, DT_LEFT | DT_VCENTER | DT_SINGLELINE),
                };
                SelectObject(hdc, font(f));
                SetTextColor(hdc, COLORREF(color));
                let mut t: Vec<u16> = cell.text.encode_utf16().collect();
                let mut r = cell.rect;
                DrawTextW(hdc, &mut t, &mut r, flags | DT_NOPREFIX);
            }
            for b in [bg, rule] {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn cs_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam.0 as u16;
            let modifier = vk == VK_SHIFT.0
                || vk == VK_CONTROL.0
                || vk == VK_MENU.0
                || vk == VK_LWIN.0
                || vk == VK_RWIN.0;
            if !modifier {
                close(true);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            close(true);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // pindah ke window lain: tutup tanpa merebut fokus kembali
            if (wparam.0 & 0xFFFF) == 0 {
                close(false);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Buat jendela cheatsheet (tersembunyi) sekali di awal.
pub unsafe fn init() -> bool {
    let Ok(hmod) = GetModuleHandleW(None) else {
        return false;
    };
    let hinst = HINSTANCE(hmod.0);
    let class = w!("VScrollWmCheatsheet");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(cs_proc),
        hInstance: hinst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return false;
    }
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        w!("vscroll-wm cheatsheet"),
        WS_POPUP,
        0,
        0,
        100,
        100,
        None,
        None,
        Some(hinst),
        None,
    ) else {
        return false;
    };
    CS_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    let round: u32 = 2; // DWMWCP_ROUND (Windows 11)
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &round as *const u32 as *const c_void,
        4,
    );
    FONT_NORMAL.store(make_font(-20, 400), Ordering::Relaxed);
    FONT_BOLD.store(make_font(-20, 700), Ordering::Relaxed);
    FONT_TITLE.store(make_font(-30, 700), Ordering::Relaxed);
    true
}

/// Buka cheatsheet, atau tutup kalau sedang terbuka.
pub unsafe fn toggle(work: Rect, cfg: &Config) {
    if is_suppressed() {
        close(false);
        return;
    }
    if VISIBLE.load(Ordering::Relaxed) {
        close(true);
        return;
    }
    if !available() {
        return;
    }
    let sheet = compute(&groups(cfg), work);
    let p = sheet.panel;
    PREV_FG.store(GetForegroundWindow().0 as isize, Ordering::Relaxed);
    if let Ok(mut g) = SHEET.lock() {
        *g = Some((sheet, cfg.overview_colors));
    }
    VISIBLE.store(true, Ordering::Relaxed);
    let h = hwnd();
    let _ = SetWindowPos(h, Some(HWND_TOPMOST), p.x, p.y, p.w, p.h, SWP_SHOWWINDOW);
    let _ = SetForegroundWindow(h);
    let _ = InvalidateRect(Some(h), None, false);
}

/// Tutup tanpa mengembalikan fokus (mis. saat dijeda).
pub unsafe fn hide() {
    close(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    fn all_rows(cfg: &Config) -> Vec<(String, String)> {
        groups(cfg).into_iter().flat_map(|g| g.rows).collect()
    }

    #[test]
    fn rows_reflect_the_current_configuration() {
        let (cfg, _) = Config::parse("overview_key = z\nresize_step = 120\n");
        let rows = all_rows(&cfg);
        assert!(rows.iter().any(|(k, _)| k == "Win + Z"));
        assert!(rows.iter().any(|(_, d)| d.contains("120")));
        assert!(rows
            .iter()
            .any(|(k, d)| k == "Win + /" && d.contains("cheatsheet")));
    }

    #[test]
    fn custom_bindings_show_up_without_touching_the_cheatsheet() {
        let (cfg, _) = Config::parse("bind.close = ctrl+alt+x\nbind.minimize = off\n");
        let rows = all_rows(&cfg);
        assert!(rows
            .iter()
            .any(|(k, d)| k == "Ctrl + Alt + X" && d == "close the window"));
        assert!(!rows.iter().any(|(_, d)| d == "minimize the window")); // dimatikan: tidak tampil
        assert!(!rows.iter().any(|(k, _)| k == "Win + Q"));
    }

    #[test]
    fn every_bindable_action_appears_in_the_cheatsheet() {
        let cfg = Config::default();
        let rows = all_rows(&cfg);
        for (i, a) in bindings::ACTIONS.iter().enumerate() {
            let desc = describe(a.desc, &cfg);
            let hit = rows.iter().any(|(_, d)| d.starts_with(&desc));
            assert!(hit, "{} tidak muncul di cheatsheet", a.name);
            // dan tombol bawaannya tertulis di baris itu
            for c in cfg.bindings.chords(i) {
                let label = match c.key {
                    bindings::Key::Vk(v) => bindings::vk_label(v),
                    bindings::Key::Digits => "1 … 9".to_string(),
                };
                assert!(
                    rows.iter().any(|(k, d)| d.starts_with(&desc)
                        && (k.contains(&label) || k.ends_with("arrows"))),
                    "{} / {}",
                    a.name,
                    label
                );
            }
        }
    }

    #[test]
    fn default_labels_match_the_documented_shortcuts() {
        let rows = all_rows(&Config::default());
        for (k, d) in [
            ("Win + ↑ / ↓", "shorter / taller"),
            ("Win + ← / →", "move into the left / right column"),
            ("Win + Shift + arrows", "focus the neighbouring window"),
            ("Win + Alt + arrows", "swap places with the neighbouring window"),
            ("Win + Ctrl + ← / →", "previous / next workspace"),
            ("Win + Ctrl + 1 … 9", "go to workspace n"),
            ("Win + Ctrl + Shift + 1 … 9", "send window to workspace n"),
            ("Win + ] / [", "move the column divider (±80 px)"),
            ("Win + Tab", "overview: map of the whole strip"),
            ("Win + Alt + Space", "overview: map of the whole strip (alternate)"),
            ("Win + Q", "close the window"),
        ] {
            assert!(rows.iter().any(|(rk, rd)| rk == k && rd == d), "{k} / {d}");
        }
    }

    #[test]
    fn height_note_and_wrap_follow_the_configuration() {
        let (cfg, _) = Config::parse("height_mode = step\nheight_step = 120\noverview_wrap = off\n");
        let rows = all_rows(&cfg);
        assert!(rows
            .iter()
            .any(|(k, d)| k == "Win + ↑ / ↓" && d.contains("120")));
        assert!(rows.iter().any(|(k, d)| k == "Arrow keys" && d == "select a window"));
        assert!(!all_rows(&Config::default())
            .iter()
            .any(|(k, d)| k == "Win + ↑ / ↓" && d.contains("px")));
    }

    #[test]
    fn mouse_rows_follow_mouse_drag() {
        let on = all_rows(&Config::default());
        assert!(on.iter().any(|(k, _)| k == "Drag onto a window"));
        assert!(on.iter().any(|(k, _)| k == "Drag a window"));
        let (cfg, _) = Config::parse("mouse_drag = off\n");
        let off = all_rows(&cfg);
        assert!(!off.iter().any(|(k, _)| k.starts_with("Drag")));
    }

    #[test]
    fn disabled_cheatsheet_key_falls_back_to_the_tray_menu() {
        let (cfg, _) = Config::parse("cheatsheet_key = off\n");
        let rows = all_rows(&cfg);
        assert!(rows
            .iter()
            .any(|(k, d)| k == "tray menu" && d.contains("cheatsheet")));
    }

    #[test]
    fn no_duplicate_key_labels_for_defaults() {
        let rows = all_rows(&Config::default());
        let keys: HashSet<&String> = rows.iter().map(|(k, _)| k).collect();
        assert_eq!(keys.len(), rows.len());
    }

    #[test]
    fn wide_screens_get_two_columns_and_fit_inside_the_screen() {
        let groups = groups(&Config::default());
        let s = compute(&groups, WORK);
        assert_eq!(s.columns, 2);
        assert!(s.panel.w <= WORK.w * 92 / 100 && s.panel.h <= WORK.h * 92 / 100);
        let (cx, cy) = (s.panel.x + s.panel.w / 2, s.panel.y + s.panel.h / 2);
        assert!((cx - WORK.w / 2).abs() <= 1 && (cy - WORK.h / 2).abs() <= 1);
    }

    #[test]
    fn narrow_screens_fall_back_to_one_column() {
        let groups = groups(&Config::default());
        let s = compute(
            &groups,
            Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 1024,
            },
        );
        assert_eq!(s.columns, 1);
    }

    #[test]
    fn every_cell_stays_inside_the_panel() {
        let groups = groups(&Config::default());
        let s = compute(&groups, WORK);
        for c in &s.cells {
            assert!(c.rect.left >= 0 && c.rect.top >= 0, "{:?}", c.kind);
            assert!(
                c.rect.right <= s.panel.w && c.rect.bottom <= s.panel.h,
                "{:?}",
                c.kind
            );
            assert!(c.rect.right > c.rect.left && c.rect.bottom > c.rect.top);
        }
        let rows = groups.iter().map(|g| g.rows.len()).sum::<usize>();
        let keys = s.cells.iter().filter(|c| c.kind == CellKind::Keys).count();
        let descs = s.cells.iter().filter(|c| c.kind == CellKind::Desc).count();
        assert_eq!((keys, descs), (rows, rows)); // semua baris tampil
    }

    #[test]
    fn key_and_description_cells_of_a_row_do_not_overlap() {
        let groups = groups(&Config::default());
        let s = compute(&groups, WORK);
        let keys: Vec<&Cell> = s
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::Keys)
            .collect();
        let descs: Vec<&Cell> = s
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::Desc)
            .collect();
        for (k, d) in keys.iter().zip(descs.iter()) {
            assert!(k.rect.right <= d.rect.left);
            assert_eq!(k.rect.top, d.rect.top);
        }
    }
}
