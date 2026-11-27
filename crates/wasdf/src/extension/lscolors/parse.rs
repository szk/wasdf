//! Pure parsing for `LS_COLORS`: SGR escape decoding and the resulting
//! extension → style table. Kept separate from `LsColorsExtension` so it's
//! testable without an env var or the `Extension` trait.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};

use crate::core::Entry;

/// The parsed `LS_COLORS` table: fixed type slots plus a `*.ext` pattern map.
/// Other two-letter keys (`pi`/`so`/`bd`/`cd`/`or`/`mi`/…) are not modeled —
/// skipped at parse time, not an error.
#[derive(Default, Clone)]
pub struct LsColorsTable {
    dir: Option<Style>,
    symlink: Option<Style>,
    exec: Option<Style>,
    file: Option<Style>,
    by_ext: HashMap<String, Style>,
}

/// Decode one SGR code sequence (e.g. `"01;38;5;208"`) into a `Style`.
/// Unrecognized codes are silently skipped; returns `None` if nothing
/// recognizable was found at all.
pub fn parse_sgr(codes: &str) -> Option<Style> {
    let parts: Vec<i64> = codes.split(';').filter_map(|s| s.parse().ok()).collect();
    let mut style = Style::default();
    let mut found = false;
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            1 => {
                style = style.add_modifier(Modifier::BOLD);
                found = true;
            }
            30..=37 => {
                style = style.fg(basic_color((parts[i] - 30) as u8));
                found = true;
            }
            90..=97 => {
                style = style.fg(bright_color((parts[i] - 90) as u8));
                found = true;
            }
            40..=47 => {
                style = style.bg(basic_color((parts[i] - 40) as u8));
                found = true;
            }
            100..=107 => {
                style = style.bg(bright_color((parts[i] - 100) as u8));
                found = true;
            }
            38 => {
                if let Some(c) = parse_extended_color(&parts, &mut i) {
                    style = style.fg(c);
                    found = true;
                }
            }
            48 => {
                if let Some(c) = parse_extended_color(&parts, &mut i) {
                    style = style.bg(c);
                    found = true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    found.then_some(style)
}

/// Parse the `5;N` (256-color) or `2;r;g;b` (truecolor) tail following a `38`
/// or `48` code, advancing `i` past the consumed elements.
fn parse_extended_color(parts: &[i64], i: &mut usize) -> Option<Color> {
    match parts.get(*i + 1) {
        Some(5) => {
            let n = *parts.get(*i + 2)?;
            *i += 2;
            Some(Color::Indexed(n as u8))
        }
        Some(2) => {
            let r = *parts.get(*i + 2)?;
            let g = *parts.get(*i + 3)?;
            let b = *parts.get(*i + 4)?;
            *i += 4;
            Some(Color::Rgb(r as u8, g as u8, b as u8))
        }
        _ => None,
    }
}

fn basic_color(n: u8) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn bright_color(n: u8) -> Color {
    match n {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

/// Parse a full `LS_COLORS` string (`key=SGR:key=SGR:…`) into a table.
pub fn parse_ls_colors(raw: &str) -> LsColorsTable {
    let mut table = LsColorsTable::default();
    for entry in raw.split(':') {
        let Some((key, sgr)) = entry.split_once('=') else { continue };
        let Some(style) = parse_sgr(sgr) else { continue };
        match key {
            "di" => table.dir = Some(style),
            "ln" => table.symlink = Some(style),
            "ex" => table.exec = Some(style),
            "fi" => table.file = Some(style),
            _ => {
                if let Some(ext) = key.strip_prefix("*.") {
                    table.by_ext.insert(ext.to_string(), style);
                }
            }
        }
    }
    table
}

/// The extension candidates to try, longest-first, for a file name — e.g.
/// `"x.tar.gz"` tries `"tar.gz"` then `"gz"`, so a `*.tar.gz` pattern is
/// preferred over a `*.gz` one when both exist.
fn ext_candidates(name: &str) -> Vec<&str> {
    let mut out = Vec::new();
    if let Some(first) = name.find('.')
        && first > 0 {
            out.push(&name[first + 1..]);
        }
    if let Some(last) = name.rfind('.')
        && last > 0 {
            let e = &name[last + 1..];
            if !out.contains(&e) {
                out.push(e);
            }
        }
    out
}

/// Resolve `entry`'s style from `table`, in priority order: symlink, dir,
/// executable, extension pattern (longest suffix first), then the generic
/// `fi` fallback. `None` means the table has no opinion — the caller falls
/// back to the theme's own dir/symlink/default coloring.
pub fn style_for(table: &LsColorsTable, entry: &Entry) -> Option<Style> {
    if entry.is_symlink
        && let Some(s) = table.symlink {
            return Some(s);
        }
    if entry.is_dir
        && let Some(s) = table.dir {
            return Some(s);
        }
    if entry.mode & 0o111 != 0
        && let Some(s) = table.exec {
            return Some(s);
        }
    for ext in ext_candidates(&entry.name) {
        if let Some(s) = table.by_ext.get(ext) {
            return Some(*s);
        }
    }
    table.file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bold_basic_color() {
        let s = parse_sgr("01;34").unwrap();
        assert_eq!(s.fg, Some(Color::Blue));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn parses_256_color() {
        let s = parse_sgr("38;5;208").unwrap();
        assert_eq!(s.fg, Some(Color::Indexed(208)));
    }

    #[test]
    fn parses_truecolor() {
        let s = parse_sgr("38;2;255;0;0").unwrap();
        assert_eq!(s.fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn unrecognized_codes_yield_none() {
        assert!(parse_sgr("").is_none());
        assert!(parse_sgr("999").is_none());
    }

    fn entry(name: &str, is_dir: bool, is_symlink: bool, mode: u32) -> Entry {
        Entry {
            path: name.into(),
            name: name.to_string(),
            is_dir,
            is_symlink,
            size: 0,
            mode,
            uid: 0,
            gid: 0, owner: "".into(), group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }

    #[test]
    fn longest_extension_pattern_wins() {
        let table = parse_ls_colors("*.gz=01;31:*.tar.gz=01;33");
        let s = style_for(&table, &entry("x.tar.gz", false, false, 0o644)).unwrap();
        assert_eq!(s.fg, Some(Color::Yellow), "tar.gz pattern should win over gz");

        let s2 = style_for(&table, &entry("x.gz", false, false, 0o644)).unwrap();
        assert_eq!(s2.fg, Some(Color::Red));
    }

    #[test]
    fn fallback_order_symlink_dir_exec_ext_fi() {
        let table = parse_ls_colors("di=01;34:ln=01;36:ex=01;32:fi=0;37:*.rs=38;5;208");

        assert_eq!(
            style_for(&table, &entry("x", true, true, 0o644)).unwrap().fg,
            Some(Color::Cyan),
            "symlink beats dir"
        );
        assert_eq!(
            style_for(&table, &entry("x", true, false, 0o644)).unwrap().fg,
            Some(Color::Blue),
            "dir beats exec/ext"
        );
        assert_eq!(
            style_for(&table, &entry("x", false, false, 0o755)).unwrap().fg,
            Some(Color::Green),
            "exec beats ext"
        );
        assert_eq!(
            style_for(&table, &entry("x.rs", false, false, 0o644)).unwrap().fg,
            Some(Color::Indexed(208)),
            "extension beats fi"
        );
        assert_eq!(
            style_for(&table, &entry("plain.txt", false, false, 0o644)).unwrap().fg,
            Some(Color::Gray),
            "fi is the final fallback"
        );
    }

    #[test]
    fn no_match_and_empty_table_yield_none() {
        let table = LsColorsTable::default();
        assert!(style_for(&table, &entry("x.rs", false, false, 0o644)).is_none());

        let table = parse_ls_colors("di=01;34");
        assert!(style_for(&table, &entry("plain.txt", false, false, 0o644)).is_none());
    }
}
