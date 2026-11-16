//! File layout.state: susunan workspace yang disimpan supaya, kalau vscroll-wm dijalankan
//! ulang selagi window-window itu masih terbuka, urutan / lebar / tinggi / workspace-nya
//! kembali seperti semula. Teks biasa, satu baris per item:
//!
//!   vscroll-layout 1
//!   active 1
//!   strip
//!   focus 197634
//!   item 197634 left half full 500 chrome.exe|Chrome_WidgetWin_1
//!
//! Window dikenali lewat handle-nya (valid selama window itu terbuka) ditambah identitas
//! `exe|class`, supaya handle yang dipakai ulang oleh window lain (mis. setelah komputer
//! di-restart) tidak salah tempat.

use std::collections::HashMap;
use std::path::PathBuf;

use layout::{Height, Item, SavedLayout, SavedStrip, Span, WindowId};

const HEADER: &str = "vscroll-layout 1";

/// %LOCALAPPDATA%\vscroll-wm\layout.state (atau di samping exe kalau tidak ada).
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        })?;
    Some(base.join("vscroll-wm").join("layout.state"))
}

fn span_str(s: Span) -> &'static str {
    match s {
        Span::Full => "full",
        Span::Left => "left",
        Span::Right => "right",
        Span::Hidden => "hidden",
    }
}

fn parse_span(s: &str) -> Option<Span> {
    Some(match s {
        "full" => Span::Full,
        "left" => Span::Left,
        "right" => Span::Right,
        "hidden" => Span::Hidden,
        _ => return None,
    })
}

fn height_str(h: Height) -> String {
    match h {
        Height::Third => "third".to_string(),
        Height::Half => "half".to_string(),
        Height::Full => "full".to_string(),
        Height::Custom(p) => format!("c{p}"),
    }
}

fn parse_height(s: &str) -> Option<Height> {
    Some(match s {
        "third" => Height::Third,
        "half" => Height::Half,
        "full" => Height::Full,
        _ => Height::Custom(s.strip_prefix('c')?.parse::<i32>().ok()?.clamp(150, 1000)),
    })
}

/// Susunan -> teks. `ident` memberi identitas tiap window; yang tidak punya (None)
/// tidak disimpan.
pub fn encode(saved: &SavedLayout, ident: impl Fn(WindowId) -> Option<String>) -> String {
    let mut body = String::new();
    let mut kept = 0usize;
    let mut active = 0usize;
    for (k, s) in saved.strips.iter().enumerate() {
        let items: Vec<(&Item, String)> = s
            .items
            .iter()
            .filter_map(|it| ident(it.id).map(|i| (it, i.replace(['\r', '\n'], " "))))
            .collect();
        if items.is_empty() {
            continue;
        }
        if k == saved.active {
            active = kept;
        }
        kept += 1;
        body.push_str("strip\n");
        if let Some(f) = s
            .focused
            .filter(|f| items.iter().any(|(it, _)| it.id == *f))
        {
            body.push_str(&format!("focus {}\n", f.0));
        }
        for (it, id) in items {
            body.push_str(&format!(
                "item {} {} {} {} {} {}\n",
                it.id.0,
                span_str(it.span),
                height_str(it.height),
                span_str(it.restore),
                it.split,
                id
            ));
        }
    }
    format!("{HEADER}\nactive {active}\n{body}")
}

/// Teks -> susunan + identitas tiap window. None kalau formatnya tidak dikenal / rusak.
pub fn decode(text: &str) -> Option<(SavedLayout, HashMap<u64, String>)> {
    let mut lines = text.lines();
    if lines.next()?.trim() != HEADER {
        return None;
    }
    let mut active = 0usize;
    let mut strips: Vec<SavedStrip> = Vec::new();
    let mut idents: HashMap<u64, String> = HashMap::new();
    for line in lines {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("active ") {
            active = v.trim().parse().ok()?;
        } else if line == "strip" {
            strips.push(SavedStrip {
                items: Vec::new(),
                focused: None,
            });
        } else if let Some(v) = line.strip_prefix("focus ") {
            strips.last_mut()?.focused = Some(WindowId(v.trim().parse().ok()?));
        } else if let Some(v) = line.strip_prefix("item ") {
            let mut f = v.splitn(6, ' ');
            let id: u64 = f.next()?.parse().ok()?;
            let span = parse_span(f.next()?)?;
            let height = parse_height(f.next()?)?;
            let restore = parse_span(f.next()?)?;
            let split: i32 = f.next()?.parse().ok()?;
            let ident = f.next()?.to_string();
            strips.last_mut()?.items.push(Item {
                id: WindowId(id),
                span,
                height,
                restore,
                split: split.clamp(100, 900),
            });
            idents.insert(id, ident);
        } else {
            return None;
        }
    }
    if strips.is_empty() {
        return None;
    }
    Some((SavedLayout { active, strips }, idents))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedLayout {
        let item = |id: u64, span, height, split| Item {
            id: WindowId(id),
            span,
            height,
            restore: span,
            split,
        };
        SavedLayout {
            active: 1,
            strips: vec![
                SavedStrip {
                    items: vec![
                        item(10, Span::Left, Height::Half, 500),
                        item(11, Span::Right, Height::Custom(420), 620),
                    ],
                    focused: Some(WindowId(11)),
                },
                SavedStrip {
                    items: vec![item(12, Span::Full, Height::Full, 500)],
                    focused: Some(WindowId(12)),
                },
                SavedStrip {
                    items: Vec::new(),
                    focused: None,
                },
            ],
        }
    }

    #[test]
    fn round_trip_keeps_everything_but_empty_workspaces() {
        let ident = |id: WindowId| Some(format!("app{}.exe|Some Class {}", id.0, id.0));
        let text = encode(&sample(), ident);
        let (back, idents) = decode(&text).expect("decodes");
        let mut want = sample();
        want.strips.pop(); // workspace kosong tidak disimpan
        assert_eq!(back, want);
        assert_eq!(idents[&10], "app10.exe|Some Class 10"); // spasi di identitas selamat
        assert_eq!(idents.len(), 3);
    }

    #[test]
    fn windows_without_identity_are_not_saved() {
        let text = encode(&sample(), |id| (id.0 != 11).then(|| "a|b".to_string()));
        let (back, _) = decode(&text).unwrap();
        assert_eq!(back.strips[0].items.len(), 1);
        assert_eq!(back.strips[0].focused, None); // window fokusnya tidak ikut
        assert_eq!(back.active, 1);
    }

    #[test]
    fn active_index_follows_dropped_workspaces() {
        // workspace 0 tidak punya window yang dikenal: aktif (tadinya 1) jadi 0
        let text = encode(&sample(), |id| (id.0 == 12).then(|| "a|b".to_string()));
        let (back, _) = decode(&text).unwrap();
        assert_eq!(back.strips.len(), 1);
        assert_eq!(back.active, 0);
    }

    #[test]
    fn rejects_unknown_or_broken_files() {
        assert!(decode("").is_none());
        assert!(decode("something else\n").is_none());
        assert!(decode("vscroll-layout 1\nactive 0\n").is_none()); // tanpa workspace
        assert!(decode("vscroll-layout 1\nitem 1 full full full 500 a|b\n").is_none()); // item tanpa strip
        assert!(decode("vscroll-layout 1\nstrip\nitem x full full full 500 a|b\n").is_none());
        assert!(decode("vscroll-layout 1\nstrip\nitem 1 wide full full 500 a|b\n").is_none());
        assert!(decode("vscroll-layout 1\nstrip\nbogus line\n").is_none());
    }

    #[test]
    fn out_of_range_numbers_are_clamped() {
        let (back, _) = decode("vscroll-layout 1\nstrip\nitem 1 left c5 left 9999 a|b\n").unwrap();
        let it = back.strips[0].items[0];
        assert_eq!(it.height, Height::Custom(150));
        assert_eq!(it.split, 900);
    }
}
