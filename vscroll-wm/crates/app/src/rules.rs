//! Aturan window yang TIDAK boleh ditata (selalu mengambang).
//!
//! Selain daftar bawaan, pengguna bisa menambah aturan lewat file teks
//! `vscroll-wm.ignore` (di folder tempat app dijalankan, atau path dari env
//! VSCROLL_IGNORE). Satu aturan per baris:
//!
//!   class:NamaKelas     persis, tanpa peduli huruf besar/kecil
//!   exe:nama.exe        nama file program
//!   title:potongan      judul window mengandung teks ini
//!
//! Baris kosong dan yang diawali # diabaikan.

use std::sync::OnceLock;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowTextW, GetWindowThreadProcessId,
};

/// Kelas window milik shell / sistem yang bukan "app" untuk ditata.
const BUILTIN_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "ForegroundStaging",
    "Windows.UI.Core.CoreWindow",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Ghost",  // placeholder untuk app yang tidak merespons
    "#32770", // dialog standar (installer, "Run", dll)
];

#[derive(Debug, Default, PartialEq)]
pub struct Rules {
    classes: Vec<String>,
    exes: Vec<String>,
    titles: Vec<String>,
}

impl Rules {
    pub fn parse(text: &str) -> Rules {
        let mut r = Rules::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((kind, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().to_lowercase();
            if value.is_empty() {
                continue;
            }
            match kind.trim().to_lowercase().as_str() {
                "class" => r.classes.push(value),
                "exe" => r.exes.push(value),
                "title" => r.titles.push(value),
                _ => {}
            }
        }
        r
    }
}

/// Env VSCROLL_IGNORE, atau file di samping exe, atau di folder kerja.
/// (Saat dijalankan otomatis saat login, folder kerja bukan folder proyek.)
fn ignore_path() -> String {
    if let Ok(p) = std::env::var("VSCROLL_IGNORE") {
        return p;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("vscroll-wm.ignore");
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    "vscroll-wm.ignore".into()
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let path = ignore_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let r = Rules::parse(&text);
                println!(
                    "aturan abaikan dimuat dari {path}: {} class, {} exe, {} title",
                    r.classes.len(),
                    r.exes.len(),
                    r.titles.len()
                );
                r
            }
            Err(_) => Rules::default(),
        }
    })
}

pub unsafe fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub unsafe fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub unsafe fn exe_name(h: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    let ok =
        QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
    let _ = CloseHandle(proc);
    ok.ok()?;
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit('\\').next().map(|s| s.to_lowercase())
}

/// Alasan window ini harus dibiarkan mengambang, atau None kalau boleh ditata.
pub unsafe fn ignored(h: HWND) -> Option<&'static str> {
    let rules = rules();
    let class = class_name(h);
    if BUILTIN_CLASSES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&class))
    {
        return Some("kelas bawaan (shell / sistem / dialog standar)");
    }
    let class = class.to_lowercase();
    if rules.classes.iter().any(|c| *c == class) {
        return Some("cocok aturan class di file ignore");
    }
    if !rules.titles.is_empty() {
        let title = window_title(h).to_lowercase();
        if rules.titles.iter().any(|t| title.contains(t.as_str())) {
            return Some("cocok aturan title di file ignore");
        }
    }
    if !rules.exes.is_empty() {
        if let Some(exe) = exe_name(h) {
            if rules.exes.contains(&exe) {
                return Some("cocok aturan exe di file ignore");
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_ignoring_comments_blanks_and_unknown_kinds() {
        let r = Rules::parse(
            "# komentar\n\nclass: MyClass \nEXE:Calc.exe\ntitle:Picture in Picture\nfoo:bar\nnocolon\nclass:\n",
        );
        assert_eq!(r.classes, vec!["myclass"]);
        assert_eq!(r.exes, vec!["calc.exe"]);
        assert_eq!(r.titles, vec!["picture in picture"]);
    }

    #[test]
    fn empty_text_gives_empty_rules() {
        assert_eq!(Rules::parse(""), Rules::default());
    }
}
