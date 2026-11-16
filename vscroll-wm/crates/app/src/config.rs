//! Konfigurasi: file teks `vscroll-wm.conf` berisi baris `kunci = nilai`.
//! Lokasi: env VSCROLL_CONFIG, atau di samping exe, atau di folder kerja.
//! Env VSCROLL_GAP / VSCROLL_ANIM_MS (kalau ada) menimpa isi file.
//!
//! Kunci yang dikenal:
//!   gap      = 16     jarak antar window dan ke tepi layar (piksel, 0-200)
//!   anim_ms  = 160    durasi animasi (ms, 0 = mati). Kosong = bawaan; otomatis
//!                     mati kalau "Animation effects" Windows dimatikan.
//!   focus_border = #4FA8F5   warna border window yang fokus (Windows 11).
//!                     Kosong / off = tidak diubah.
//!   overview_hint = on        petunjuk tombol di bagian bawah overview
//!   overview_thumbnails = on  tampilan mini tiap window di overview
//!   overview_label = title    isi label kotak: title (judul penuh), app (nama app
//!                     saja), reverse ("judul - Vivaldi" jadi "Vivaldi - judul")
//!   alias.code = VS Code      nama tampilan untuk mode `app` (nama exe tanpa .exe)
//!   cheatsheet_key = slash    tombol cheatsheet bersama Win: slash (/), f1-f12, tab,
//!                     space, grave, satu huruf/angka, atau off. Beda dari overview_key.
//!   resize_step = 80          langkah Win+[ dan Win+] (piksel, 10-400)
//!   height_mode = preset      Win+↑/↓: preset (1/3, 1/2, penuh) atau step (per langkah)
//!   height_step = 80          langkah Win+↑/↓ di mode step (piksel, 10-400)
//!   overview_size = 70        ukuran panel overview, persen dari area kerja (30-100)
//!   overview_key = tab        tombol overview bersama Win: tab, space, grave, satu
//!                     huruf/angka (bukan q/m), atau off. Win+Alt+Space selalu aktif.
//!   overview_color_selected / _active / _view / _background / _box = #RRGGBB
//!   mouse_drag = on           seret window ke window lain = tukar posisi; tarik tepi window yang
//!                     berbatasan dengan pembagi lajur = geser pembagi; di overview, seret
//!                     window ke kolom workspace lain = pindah workspace
//!   remember_layout = on      simpan susunan workspace; dipulihkan kalau vscroll-wm dijalankan
//!                     ulang selagi window-windownya masih terbuka
//!   workspace_osd = on        panel kecil sesaat di bagian bawah layar setiap pindah workspace
//!   overview_wrap = on        panah di overview berputar: dari ujung loncat ke ujung seberang
//!   bind.<aksi> = win+q       ubah pintasan (daftar aksi: bindings.rs / vscroll-wm.conf.example).
//!                     Beberapa tombol dipisah koma, off = dimatikan. overview_key dan
//!                     cheatsheet_key tetap berlaku sebagai tombol bawaan bind.overview /
//!                     bind.cheatsheet.
//!
//! Komentar hanya berlaku untuk baris yang diawali #.

use std::path::PathBuf;

use crate::bindings::{self, Bindings, Chord, ALT, WIN};

pub const DEFAULT_GAP: i32 = 16;

/// COLORREF (0x00BBGGRR) dari komponen RGB.
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

/// Warna di overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewColors {
    /// Kursor overview: window yang akan dituju.
    pub selected: u32,
    /// Window yang sedang aktif sekarang.
    pub active: u32,
    /// Bingkai bagian strip (baris) yang sedang tampil di layar.
    pub view: u32,
    pub background: u32,
    pub box_fill: u32,
}

impl Default for OverviewColors {
    fn default() -> Self {
        Self {
            selected: rgb(255, 190, 60),
            active: rgb(79, 168, 245),
            view: rgb(120, 200, 130),
            background: rgb(20, 22, 28),
            box_fill: rgb(45, 50, 64),
        }
    }
}

/// Isi label kotak di overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelMode {
    /// Judul window apa adanya.
    Title,
    /// Nama app saja (dari nama exe).
    App,
    /// Bagian setelah " - " dipindah ke depan: "judul - Vivaldi" jadi "Vivaldi - judul".
    Reverse,
}

/// Cara Win+↑/↓ mengubah tinggi window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightMode {
    /// Lompat antar preset 1/3, 1/2, penuh.
    Preset,
    /// Geser `height_step` piksel per tekan.
    Step,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub gap: i32,
    /// None = bawaan (160 ms, atau mati bila animasi sistem dimatikan).
    pub anim_ms: Option<f32>,
    /// Warna border fokus sebagai COLORREF (0x00BBGGRR). None = tidak diubah.
    pub focus_border: Option<u32>,
    pub overview_hint: bool,
    pub overview_thumbnails: bool,
    pub overview_label: LabelMode,
    /// Nama tampilan untuk mode `app`: (nama exe tanpa .exe, huruf kecil) -> nama.
    pub aliases: Vec<(String, String)>,
    /// Langkah pergeseran pembagi lajur per tekan Win+[ / Win+] (piksel).
    pub resize_step: i32,
    pub height_mode: HeightMode,
    pub height_step: i32,
    /// Kode virtual-key tombol cheatsheet (bersama Win). 0 = dimatikan.
    pub cheatsheet_key: u16,
    /// Ukuran panel overview dalam persen area kerja.
    pub overview_size: i32,
    /// Kode virtual-key tombol overview (bersama Win). 0 = dimatikan.
    pub overview_key: u16,
    pub overview_colors: OverviewColors,
    /// Panah di overview berputar (dari ujung loncat ke ujung seberang).
    pub overview_wrap: bool,
    /// Panel kecil sesaat setiap pindah workspace.
    pub workspace_osd: bool,
    /// Simpan / pulihkan susunan workspace (layout.state).
    pub remember_layout: bool,
    /// Drag mouse: tukar posisi, geser pembagi (di layar) dan pindah workspace (di overview).
    pub mouse_drag: bool,
    /// Tombol tiap aksi (bawaan + `bind.*` + overview_key / cheatsheet_key).
    pub bindings: Bindings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gap: DEFAULT_GAP,
            anim_ms: None,
            focus_border: None,
            overview_hint: true,
            overview_thumbnails: true,
            overview_label: LabelMode::Title,
            aliases: Vec::new(),
            resize_step: 80,
            height_mode: HeightMode::Preset,
            height_step: 80,
            cheatsheet_key: 0xBF, // /
            overview_size: 70,
            overview_key: 0x09, // Tab
            overview_colors: OverviewColors::default(),
            overview_wrap: true,
            workspace_osd: true,
            remember_layout: true,
            mouse_drag: true,
            bindings: Bindings::defaults(),
        }
    }
}

impl Config {
    /// Baca isi file. Mengembalikan konfigurasi dan peringatan untuk baris bermasalah.
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut cfg = Config::default();
        let mut warnings = Vec::new();
        let mut binds = Bindings::defaults();
        // aksi yang diatur lewat bind.* (urutan file = urutan prioritas saat bentrok)
        let mut explicit: Vec<usize> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let n = n + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {n}: not in 'key = value' form"));
                continue;
            };
            let key = key.trim().to_lowercase();
            let value = value.trim();
            match key.as_str() {
                "gap" => match value.parse::<i32>() {
                    Ok(v) => cfg.gap = v.clamp(0, 200),
                    Err(_) => warnings.push(format!("line {n}: gap must be a whole number")),
                },
                "anim_ms" => match value.parse::<f32>() {
                    Ok(v) if v.is_finite() => cfg.anim_ms = Some(v.clamp(0.0, 1000.0)),
                    _ => warnings.push(format!("line {n}: anim_ms must be a number")),
                },
                "focus_border" => match parse_color(value) {
                    Some(color) => cfg.focus_border = color,
                    None => warnings.push(format!("line {n}: focus_border must be #RRGGBB or off")),
                },
                "overview_hint" => match parse_bool(value) {
                    Some(b) => cfg.overview_hint = b,
                    None => warnings.push(format!("line {n}: overview_hint must be on/off")),
                },
                "overview_thumbnails" => match parse_bool(value) {
                    Some(b) => cfg.overview_thumbnails = b,
                    None => warnings.push(format!("line {n}: overview_thumbnails must be on/off")),
                },
                "overview_label" => match value.to_lowercase().as_str() {
                    "title" => cfg.overview_label = LabelMode::Title,
                    "app" => cfg.overview_label = LabelMode::App,
                    "reverse" => cfg.overview_label = LabelMode::Reverse,
                    _ => warnings.push(format!(
                        "line {n}: overview_label must be title, app or reverse"
                    )),
                },
                "resize_step" => match value.parse::<i32>() {
                    Ok(v) => cfg.resize_step = v.clamp(10, 400),
                    Err(_) => warnings.push(format!("line {n}: resize_step must be a whole number")),
                },
                "height_mode" => match value.to_lowercase().as_str() {
                    "preset" => cfg.height_mode = HeightMode::Preset,
                    "step" => cfg.height_mode = HeightMode::Step,
                    _ => warnings.push(format!("line {n}: height_mode must be preset or step")),
                },
                "height_step" => match value.parse::<i32>() {
                    Ok(v) => cfg.height_step = v.clamp(10, 400),
                    Err(_) => warnings.push(format!("line {n}: height_step must be a whole number")),
                },
                "overview_size" => match value.parse::<i32>() {
                    Ok(v) => cfg.overview_size = v.clamp(30, 100),
                    Err(_) => warnings.push(format!("line {n}: overview_size must be a whole number")),
                },
                "cheatsheet_key" => match parse_key(value) {
                    Some(k) => cfg.cheatsheet_key = k,
                    None => warnings.push(format!(
                        "line {n}: cheatsheet_key must be slash, f1-f12, tab, space, grave, off, or a single letter/digit (not q/m)"
                    )),
                },
                "overview_key" => match parse_key(value) {
                    Some(k) => cfg.overview_key = k,
                    None => warnings.push(format!(
                        "line {n}: overview_key must be tab, space, grave, off, or a single letter/digit (not q/m)"
                    )),
                },
                "overview_color_selected" => {
                    color_setting(value, &mut cfg.overview_colors.selected, &key, n, &mut warnings)
                }
                "overview_color_active" => {
                    color_setting(value, &mut cfg.overview_colors.active, &key, n, &mut warnings)
                }
                "overview_color_view" => {
                    color_setting(value, &mut cfg.overview_colors.view, &key, n, &mut warnings)
                }
                "overview_color_background" => {
                    color_setting(value, &mut cfg.overview_colors.background, &key, n, &mut warnings)
                }
                "overview_color_box" => {
                    color_setting(value, &mut cfg.overview_colors.box_fill, &key, n, &mut warnings)
                }
                "mouse_drag" => match parse_bool(value) {
                    Some(b) => cfg.mouse_drag = b,
                    None => warnings.push(format!("line {n}: mouse_drag must be on/off")),
                },
                "remember_layout" => match parse_bool(value) {
                    Some(b) => cfg.remember_layout = b,
                    None => warnings.push(format!("line {n}: remember_layout must be on/off")),
                },
                "workspace_osd" => match parse_bool(value) {
                    Some(b) => cfg.workspace_osd = b,
                    None => warnings.push(format!("line {n}: workspace_osd must be on/off")),
                },
                "overview_wrap" => match parse_bool(value) {
                    Some(b) => cfg.overview_wrap = b,
                    None => warnings.push(format!("line {n}: overview_wrap must be on/off")),
                },
                k if k.starts_with("bind.") => {
                    let name = k["bind.".len()..].trim();
                    match bindings::find_action(name) {
                        None => warnings.push(format!("line {n}: unknown action '{name}' in bind.{name}")),
                        Some(i) => match bindings::parse_list(i, value) {
                            Ok(list) => {
                                binds.set(i, list);
                                explicit.retain(|&e| e != i);
                                explicit.push(i);
                            }
                            Err(e) => warnings.push(format!("line {n}: {e}")),
                        },
                    }
                }
                k if k.starts_with("alias.") => {
                    let stem = k["alias.".len()..].trim().trim_end_matches(".exe").to_string();
                    if stem.is_empty() || value.is_empty() {
                        warnings.push(format!("line {n}: alias needs an exe name and a display name"));
                    } else {
                        cfg.aliases.retain(|(s, _)| *s != stem);
                        cfg.aliases.push((stem, value.to_string()));
                    }
                }
                _ => warnings.push(format!("line {n}: unknown key '{key}'")),
            }
        }
        if cfg.cheatsheet_key != 0 && cfg.cheatsheet_key == cfg.overview_key {
            warnings
                .push("cheatsheet_key equals overview_key: cheatsheet key disabled".to_string());
            cfg.cheatsheet_key = 0;
        }
        // overview_key / cheatsheet_key (cara lama) = tombol bawaan kedua aksi itu,
        // kecuali kalau sudah diatur lewat bind.*
        if let Some(i) = bindings::find_action("overview").filter(|i| !explicit.contains(i)) {
            let mut list = Vec::new();
            if cfg.overview_key != 0 {
                list.push(Chord::new(WIN, cfg.overview_key));
            }
            list.push(Chord::new(WIN | ALT, 0x20)); // Win+Alt+Space: selalu ada
            binds.set(i, list);
        }
        if let Some(i) = bindings::find_action("cheatsheet").filter(|i| !explicit.contains(i)) {
            let list = if cfg.cheatsheet_key != 0 {
                vec![Chord::new(WIN, cfg.cheatsheet_key)]
            } else {
                Vec::new()
            };
            binds.set(i, list);
        }
        warnings.extend(binds.resolve_conflicts(&explicit));
        cfg.bindings = binds;
        (cfg, warnings)
    }
}

/// "#RRGGBB" / "RRGGBB" -> Some(Some(COLORREF)); "off"/"none"/kosong -> Some(None);
/// selain itu None (tidak valid).
fn parse_color(v: &str) -> Option<Option<u32>> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    let (r, g, b) = ((n >> 16) & 0xFF, (n >> 8) & 0xFF, n & 0xFF);
    Some(Some(r | (g << 8) | (b << 16)))
}

fn color_setting(value: &str, target: &mut u32, name: &str, n: usize, warnings: &mut Vec<String>) {
    match parse_color(value) {
        Some(Some(c)) => *target = c,
        _ => warnings.push(format!("line {n}: {name} must be #RRGGBB")),
    }
}

/// Nama tombol -> kode virtual-key. "off" = 0 (dimatikan). None = tidak valid
/// atau bentrok dengan tombol Win+... lain yang sudah dipakai (Q tutup, M minimize).
fn parse_key(v: &str) -> Option<u16> {
    let v = v.trim().to_lowercase();
    let code: u16 = match v.as_str() {
        "off" | "none" => return Some(0),
        "tab" => 0x09,
        "slash" => 0xBF,
        "space" => 0x20,
        s if s.len() >= 2
            && s.starts_with('f')
            && s[1..]
                .parse::<u16>()
                .map_or(false, |n| (1..=12).contains(&n)) =>
        {
            0x6F + s[1..].parse::<u16>().unwrap_or(1)
        }
        "grave" | "backtick" | "`" => 0xC0,
        s if s.len() == 1 && s.chars().all(|ch| ch.is_ascii_alphanumeric()) => {
            s.to_ascii_uppercase().as_bytes()[0] as u16
        }
        _ => return None,
    };
    if code == 0x51 || code == 0x4D {
        return None;
    }
    Some(code)
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Nama tampilan bawaan untuk exe yang namanya kurang enak dibaca.
const BUILTIN_ALIASES: &[(&str, &str)] = &[
    ("code", "VS Code"),
    ("windowsterminal", "Windows Terminal"),
    ("explorer", "File Explorer"),
    ("msedge", "Edge"),
    ("chrome", "Chrome"),
    ("firefox", "Firefox"),
    ("vivaldi", "Vivaldi"),
    ("notepad", "Notepad"),
    ("claude", "Claude"),
    ("discord", "Discord"),
    ("spotify", "Spotify"),
    ("slack", "Slack"),
    ("ms-teams", "Teams"),
    ("wezterm-gui", "WezTerm"),
];

/// Nama app dari nama exe (mis. "vivaldi.exe" -> "Vivaldi"). None kalau tidak
/// informatif (host UWP), supaya pemanggil memakai judul window.
pub fn app_name(exe: &str, aliases: &[(String, String)]) -> Option<String> {
    let lower = exe.trim().to_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower).to_string();
    if let Some((_, name)) = aliases.iter().find(|(s, _)| *s == stem) {
        return Some(name.clone());
    }
    if let Some((_, name)) = BUILTIN_ALIASES.iter().find(|(s, _)| *s == stem) {
        return Some((*name).to_string());
    }
    if stem.is_empty() || stem == "applicationframehost" {
        return None;
    }
    let mut chars = stem.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

/// "judul - bagian - Vivaldi" -> "Vivaldi - judul - bagian" (pemisah terakhir).
fn swap_around_separator(title: &str) -> String {
    let mut best: Option<(usize, &str)> = None;
    for sep in [" - ", " — ", " – "] {
        if let Some(i) = title.rfind(sep) {
            if best.map_or(true, |(b, _)| i > b) {
                best = Some((i, sep));
            }
        }
    }
    match best {
        Some((i, sep)) => {
            let (left, right) = (&title[..i], &title[i + sep.len()..]);
            if left.is_empty() || right.is_empty() {
                title.to_string()
            } else {
                format!("{right}{sep}{left}")
            }
        }
        None => title.to_string(),
    }
}

/// Label kotak overview menurut mode yang dipilih.
pub fn label(
    mode: LabelMode,
    title: &str,
    exe: Option<&str>,
    aliases: &[(String, String)],
) -> String {
    match mode {
        LabelMode::Title => title.to_string(),
        LabelMode::Reverse => swap_around_separator(title),
        LabelMode::App => exe
            .and_then(|e| app_name(e, aliases))
            .unwrap_or_else(|| title.to_string()),
    }
}

fn find_file() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VSCROLL_CONFIG") {
        return Some(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("vscroll-wm.conf");
            if p.exists() {
                return Some(p);
            }
        }
    }
    let p = PathBuf::from("vscroll-wm.conf");
    p.exists().then_some(p)
}

/// Hasil memuat konfigurasi: isinya, masalah yang ditemukan (untuk ditampilkan ke
/// pengguna), dan file yang dibaca.
pub struct LoadReport {
    pub cfg: Config,
    pub problems: Vec<String>,
    pub path: Option<PathBuf>,
}

/// Muat konfigurasi: bawaan, lalu file, lalu environment variable.
pub fn load() -> LoadReport {
    let mut cfg = Config::default();
    let mut problems = Vec::new();
    let path = find_file();
    if let Some(path) = &path {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (c, warnings) = Config::parse(&text);
                cfg = c;
                println!("konfigurasi dimuat dari {}", path.display());
                for w in &warnings {
                    eprintln!("konfigurasi: {w}");
                }
                problems = warnings;
            }
            Err(e) => {
                eprintln!("gagal membaca {}: {e}", path.display());
                problems.push(format!("could not read the file: {e}"));
            }
        }
    }
    if let Some(v) = std::env::var("VSCROLL_GAP")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        cfg.gap = v.clamp(0, 200);
    }
    if let Some(v) = std::env::var("VSCROLL_ANIM_MS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        if v.is_finite() {
            cfg.anim_ms = Some(v.clamp(0.0, 1000.0));
        }
    }
    LoadReport {
        cfg,
        problems,
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_gives_defaults() {
        let (cfg, warnings) = Config::parse("");
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_values_comments_and_case() {
        let (cfg, w) = Config::parse("# komentar\nGap = 24\nanim_ms=200\nfocus_border = #4FA8F5\n");
        assert_eq!(cfg.gap, 24);
        assert_eq!(cfg.anim_ms, Some(200.0));
        assert_eq!(cfg.focus_border, Some(0x00F5_A84F)); // BGR untuk RGB #4FA8F5
        assert!(w.is_empty());
    }

    #[test]
    fn focus_border_accepts_plain_hex_and_off() {
        let (a, _) = Config::parse("focus_border = FF0000");
        assert_eq!(a.focus_border, Some(0x0000_00FF));
        let (b, _) = Config::parse("focus_border = #FF0000\nfocus_border = off");
        assert_eq!(b.focus_border, None);
    }

    #[test]
    fn focus_border_rejects_bad_colors() {
        let (cfg, w) = Config::parse("focus_border = merah\nfocus_border = #12345");
        assert_eq!(cfg.focus_border, None);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn clamps_out_of_range_values() {
        let (cfg, _) = Config::parse("gap = 9999\nanim_ms = -5\n");
        assert_eq!(cfg.gap, 200);
        assert_eq!(cfg.anim_ms, Some(0.0));
    }

    #[test]
    fn bad_lines_warn_and_keep_defaults() {
        let (cfg, w) = Config::parse("gap = banyak\nanim_ms = nan\nwarna = biru\nbukan kunci\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(w.len(), 4);
    }

    #[test]
    fn parses_overview_options_and_aliases() {
        let (cfg, w) = Config::parse(
            "overview_hint = off\noverview_thumbnails = no\noverview_label = Reverse\nalias.code = Kode\nalias.Foo.exe = Fu\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert!(!cfg.overview_hint);
        assert!(!cfg.overview_thumbnails);
        assert_eq!(cfg.overview_label, LabelMode::Reverse);
        assert_eq!(
            cfg.aliases,
            vec![
                ("code".to_string(), "Kode".to_string()),
                ("foo".to_string(), "Fu".to_string())
            ]
        );
    }

    #[test]
    fn bad_overview_values_warn() {
        let (cfg, w) =
            Config::parse("overview_hint = mungkin\noverview_label = hias\nalias. = x\n");
        assert_eq!(w.len(), 3);
        assert!(cfg.overview_hint);
        assert_eq!(cfg.overview_label, LabelMode::Title);
    }

    #[test]
    fn reverse_label_moves_the_last_segment_to_front() {
        let al: Vec<(String, String)> = vec![];
        assert_eq!(
            label(LabelMode::Reverse, "Percakapan baru - Vivaldi", None, &al),
            "Vivaldi - Percakapan baru"
        );
        assert_eq!(
            label(LabelMode::Reverse, "a - b - Vivaldi", None, &al),
            "Vivaldi - a - b"
        );
        assert_eq!(
            label(LabelMode::Reverse, "Tanpa pemisah", None, &al),
            "Tanpa pemisah"
        );
        assert_eq!(
            label(LabelMode::Reverse, "Judul — Edge", None, &al),
            "Edge — Judul"
        );
    }

    #[test]
    fn app_label_uses_exe_name_and_falls_back_to_title() {
        let al = vec![("vivaldi".to_string(), "Viv".to_string())];
        assert_eq!(
            label(LabelMode::App, "x - Vivaldi", Some("vivaldi.exe"), &al),
            "Viv"
        );
        assert_eq!(label(LabelMode::App, "x", Some("code.exe"), &[]), "VS Code");
        assert_eq!(label(LabelMode::App, "x", Some("foo.exe"), &[]), "Foo");
        assert_eq!(
            label(
                LabelMode::App,
                "Kalkulator",
                Some("applicationframehost.exe"),
                &[]
            ),
            "Kalkulator"
        );
        assert_eq!(label(LabelMode::App, "judul", None, &[]), "judul");
    }

    #[test]
    fn parses_overview_size_key_and_colors() {
        let (cfg, w) = Config::parse(
            "overview_size = 55\noverview_key = z\noverview_color_selected = #FF0000\noverview_color_view = 00FF00\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(cfg.overview_size, 55);
        assert_eq!(cfg.overview_key, 0x5A);
        assert_eq!(cfg.overview_colors.selected, 0x0000_00FF);
        assert_eq!(cfg.overview_colors.view, 0x0000_FF00);
        assert_eq!(cfg.overview_colors.active, OverviewColors::default().active);
    }

    #[test]
    fn overview_key_names_and_rejections() {
        assert_eq!(parse_key("tab"), Some(0x09));
        assert_eq!(parse_key("Space"), Some(0x20));
        assert_eq!(parse_key("grave"), Some(0xC0));
        assert_eq!(parse_key("Z"), Some(0x5A));
        assert_eq!(parse_key("3"), Some(0x33));
        assert_eq!(parse_key("off"), Some(0));
        assert_eq!(parse_key("q"), None); // dipakai: tutup window
        assert_eq!(parse_key("m"), None); // dipakai: minimize
        assert_eq!(parse_key("enter"), None);
    }

    #[test]
    fn overview_size_is_clamped_and_bad_colors_warn() {
        let (cfg, w) = Config::parse(
            "overview_size = 5\noverview_color_box = biru\noverview_color_active = off\n",
        );
        assert_eq!(cfg.overview_size, 30);
        assert_eq!(w.len(), 2);
        assert_eq!(cfg.overview_colors, OverviewColors::default());
    }

    #[test]
    fn resize_step_is_parsed_and_clamped() {
        let (cfg, w) = Config::parse("resize_step = 120");
        assert!(w.is_empty());
        assert_eq!(cfg.resize_step, 120);
        assert_eq!(Config::parse("resize_step = 1").0.resize_step, 10);
        assert_eq!(Config::parse("resize_step = 9999").0.resize_step, 400);
        assert_eq!(Config::default().resize_step, 80);
        let (cfg, w) = Config::parse("resize_step = lebar");
        assert_eq!((cfg.resize_step, w.len()), (80, 1));
    }

    #[test]
    fn cheatsheet_key_parsing_and_conflict_with_overview_key() {
        assert_eq!(Config::default().cheatsheet_key, 0xBF);
        assert_eq!(Config::parse("cheatsheet_key = f1").0.cheatsheet_key, 0x70);
        assert_eq!(Config::parse("cheatsheet_key = F12").0.cheatsheet_key, 0x7B);
        assert_eq!(Config::parse("cheatsheet_key = h").0.cheatsheet_key, 0x48);
        assert_eq!(Config::parse("cheatsheet_key = off").0.cheatsheet_key, 0);
        assert_eq!(parse_key("f13"), None);
        assert_eq!(parse_key("f"), Some(0x46)); // huruf F, bukan F-key
        let (cfg, w) = Config::parse("overview_key = z\ncheatsheet_key = z\n");
        assert_eq!(cfg.cheatsheet_key, 0); // bentrok: cheatsheet mengalah
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn height_mode_and_step_are_parsed() {
        let d = Config::default();
        assert_eq!((d.height_mode, d.height_step), (HeightMode::Preset, 80));
        let (cfg, w) = Config::parse("height_mode = Step\nheight_step = 120");
        assert!(w.is_empty());
        assert_eq!((cfg.height_mode, cfg.height_step), (HeightMode::Step, 120));
        assert_eq!(Config::parse("height_step = 1").0.height_step, 10);
        assert_eq!(Config::parse("height_step = 9999").0.height_step, 400);
        let (cfg, w) = Config::parse("height_mode = pixel");
        assert_eq!((cfg.height_mode, w.len()), (HeightMode::Preset, 1));
    }

    #[test]
    fn mouse_drag_defaults_to_on() {
        assert!(Config::default().mouse_drag);
        let (cfg, w) = Config::parse("mouse_drag = off");
        assert!(!cfg.mouse_drag && w.is_empty());
        let (cfg, w) = Config::parse("mouse_drag = sering");
        assert!(cfg.mouse_drag);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn remember_layout_defaults_to_on() {
        assert!(Config::default().remember_layout);
        let (cfg, w) = Config::parse("remember_layout = off");
        assert!(!cfg.remember_layout && w.is_empty());
        let (cfg, w) = Config::parse("remember_layout = mungkin");
        assert!(cfg.remember_layout);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn workspace_osd_defaults_to_on() {
        assert!(Config::default().workspace_osd);
        let (cfg, w) = Config::parse("workspace_osd = off");
        assert!(!cfg.workspace_osd && w.is_empty());
        let (cfg, w) = Config::parse("workspace_osd = ya");
        assert!(cfg.workspace_osd);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn overview_wrap_defaults_to_on() {
        assert!(Config::default().overview_wrap);
        let (cfg, w) = Config::parse("overview_wrap = off");
        assert!(!cfg.overview_wrap && w.is_empty());
        let (cfg, w) = Config::parse("overview_wrap = kadang");
        assert!(cfg.overview_wrap);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn bind_lines_change_shortcuts() {
        let (cfg, w) = Config::parse("bind.close = win+x\nbind.minimize = off\nbind.overview = ctrl+alt+o\n");
        assert!(w.is_empty(), "{w:?}");
        let b = &cfg.bindings;
        assert_eq!(b.lookup(WIN, 0x58), Some(crate::consts::HOTKEY_CLOSE));
        assert_eq!(b.lookup(WIN, 0x51), None); // Win+Q tidak lagi menutup
        assert_eq!(b.lookup(WIN, 0x4D), None); // minimize dimatikan
        assert_eq!(
            b.lookup(bindings::CTRL | ALT, 0x4F),
            Some(crate::consts::HOTKEY_OVERVIEW)
        );
        assert_eq!(b.lookup(WIN, 0x09), None); // Win+Tab dilepas (bind.overview menggantikan semuanya)
    }

    #[test]
    fn bad_bind_lines_warn_and_keep_defaults() {
        let (cfg, w) = Config::parse("bind.nope = win+x\nbind.close = win+\nbind.float = hyper+f\n");
        assert_eq!(w.len(), 3, "{w:?}");
        assert_eq!(cfg.bindings, Bindings::defaults());
    }

    #[test]
    fn bind_conflicts_are_resolved_in_favour_of_the_explicit_line() {
        let (cfg, w) = Config::parse("bind.close = win+m\n");
        assert_eq!(w.len(), 1, "{w:?}"); // minimize kehilangan Win+M
        assert_eq!(cfg.bindings.lookup(WIN, 0x4D), Some(crate::consts::HOTKEY_CLOSE));
    }

    #[test]
    fn legacy_keys_still_set_the_default_chords() {
        let (cfg, w) = Config::parse("overview_key = z\ncheatsheet_key = f1\n");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(cfg.bindings.lookup(WIN, 0x5A), Some(crate::consts::HOTKEY_OVERVIEW));
        assert_eq!(cfg.bindings.lookup(WIN, 0x70), Some(crate::consts::HOTKEY_CHEATSHEET));
        assert_eq!(cfg.bindings.lookup(WIN | ALT, 0x20), Some(crate::consts::HOTKEY_OVERVIEW));
        assert_eq!(cfg.bindings.lookup(WIN, 0x09), None);
        // bind.* menang atas overview_key
        let (cfg, _) = Config::parse("overview_key = z\nbind.overview = win+o\n");
        assert_eq!(cfg.bindings.lookup(WIN, 0x4F), Some(crate::consts::HOTKEY_OVERVIEW));
        assert_eq!(cfg.bindings.lookup(WIN, 0x5A), None);
        // overview_key = off: Win+Alt+Space tetap ada
        let (cfg, _) = Config::parse("overview_key = off\n");
        assert_eq!(cfg.bindings.lookup(WIN | ALT, 0x20), Some(crate::consts::HOTKEY_OVERVIEW));
        assert_eq!(cfg.bindings.overview_cycle_vk(), 0);
    }
}
