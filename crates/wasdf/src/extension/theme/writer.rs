//! Persistence of theme selection and custom theme definitions into the
//! managed block in config.scm. Uses the same BLOCK_BEGIN/BLOCK_END markers
//! and atomic-write contract as `settings/writer.rs`.

use std::io;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

use super::registry::{NamedTheme, RoleStyle};
use crate::core::color_token;

fn bool_tok(b: bool) -> &'static str {
    if b { "#t" } else { "#f" }
}

/// `Color::Reset` ("nil"/transparent) is the bare symbol `nil`; any other
/// color is a quoted token, same as everywhere else colors are written.
fn color_tok(c: Color) -> String {
    if c == Color::Reset { "nil".into() } else { format!("{:?}", color_token(c)) }
}

pub const BLOCK_BEGIN: &str =
    ";; >>> wasdf settings (managed — edited via the SETTINGS panel; do not hand-edit)";
pub const BLOCK_END: &str = ";; <<< wasdf settings";

pub struct ThemeWriter {
    path: PathBuf,
}

impl ThemeWriter {
    pub fn new(path: PathBuf) -> Self {
        ThemeWriter { path }
    }

    /// Insert or replace the single `(set-theme "name")` form in the managed block.
    pub fn set_active(&self, name: &str) -> io::Result<()> {
        let new_line = format!("(set-theme {:?})", name);
        self.upsert_line(|line| line.trim().starts_with("(set-theme "), &new_line)
    }

    /// Insert or replace the `(define-theme "name" …)` multi-line form. Every
    /// override is one line, always the full `(role fg bg bold italic
    /// underline)` shape — `fg`/`bg` a quoted color token or the bare symbol
    /// `nil` for "no color" (`Color::Reset`), `bold`/`italic`/`underline`
    /// `#t`/`#f`. There is no partial/merged/standalone form: a role's whole
    /// look is one setting, written as one line.
    pub fn upsert_theme(&self, t: &NamedTheme) -> io::Result<()> {
        let mut body = format!("(define-theme {:?}", t.name);
        for (role, style) in &t.overrides {
            let RoleStyle { fg, bg, bold, italic, underline } = *style;
            body.push_str(&format!(
                "\n  ({} {} {} {} {} {})",
                role.token(), color_tok(fg), color_tok(bg),
                bool_tok(bold), bool_tok(italic), bool_tok(underline),
            ));
        }
        body.push(')');
        self.upsert_form(&t.name, &body)
    }

    /// Remove the `(define-theme "name" …)` form from the managed block.
    pub fn remove_theme(&self, name: &str) -> io::Result<()> {
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)?
        } else {
            return Ok(());
        };
        let updated = remove_define_theme(&text, name);
        atomic_write(&self.path, &updated)
    }

    fn upsert_line(&self, is_match: impl Fn(&str) -> bool, new_line: &str) -> io::Result<()> {
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)?
        } else {
            String::new()
        };
        let updated = upsert_line_in_block(&text, is_match, new_line);
        atomic_write(&self.path, &updated)
    }

    fn upsert_form(&self, name: &str, new_form: &str) -> io::Result<()> {
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)?
        } else {
            String::new()
        };
        let updated = upsert_define_theme(&text, name, new_form);
        atomic_write(&self.path, &updated)
    }
}

// ── Block manipulation helpers ─────────────────────────────────────────────────

/// Insert or replace a single-line form inside the managed block.
fn upsert_line_in_block(text: &str, is_match: impl Fn(&str) -> bool, new_line: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            let mut replaced = false;
            for i in (b + 1)..e {
                if is_match(&out[i]) {
                    out[i] = new_line.to_string();
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                out.insert(e, new_line.to_string());
            }
            let mut result = out.join("\n");
            if text.ends_with('\n') { result.push('\n'); }
            result
        }
        _ => {
            let base = text.trim_end_matches('\n');
            if base.is_empty() {
                format!("{BLOCK_BEGIN}\n{new_line}\n{BLOCK_END}\n")
            } else {
                format!("{base}\n{BLOCK_BEGIN}\n{new_line}\n{BLOCK_END}\n")
            }
        }
    }
}

/// Find the line range `[start, end]` (inclusive) of the `(define-theme "name" …)`
/// form inside a managed block bounded by `[b+1, e)`. Returns `None` if absent.
fn find_define_theme_range(lines: &[&str], b: usize, e: usize, name_token: &str) -> Option<(usize, usize)> {
    let start = lines[b + 1..e]
        .iter()
        .position(|l| {
            let t = l.trim();
            t.starts_with("(define-theme ") && t.contains(name_token)
        })
        .map(|i| i + b + 1)?;
    Some((start, find_form_end(lines, start, e)))
}

/// Insert or replace a multi-line `(define-theme "name" …)` form in the block.
fn upsert_define_theme(text: &str, name: &str, new_form: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    let name_token = format!("{:?}", name); // "name"

    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            if let Some((start, form_end)) = find_define_theme_range(&lines, b, e, &name_token) {
                let new_lines: Vec<String> = new_form.lines().map(String::from).collect();
                out.splice(start..=form_end, new_lines);
            } else {
                // Insert before BLOCK_END.
                let adjusted_e = out.iter().position(|l| l.trim() == BLOCK_END).unwrap_or(e);
                let new_lines: Vec<String> = new_form.lines().map(String::from).collect();
                for (i, nl) in new_lines.into_iter().enumerate() {
                    out.insert(adjusted_e + i, nl);
                }
            }
            let mut result = out.join("\n");
            if text.ends_with('\n') { result.push('\n'); }
            result
        }
        _ => {
            let base = text.trim_end_matches('\n');
            if base.is_empty() {
                format!("{BLOCK_BEGIN}\n{new_form}\n{BLOCK_END}\n")
            } else {
                format!("{base}\n{BLOCK_BEGIN}\n{new_form}\n{BLOCK_END}\n")
            }
        }
    }
}

/// Remove the `(define-theme "name" …)` form from the block.
fn remove_define_theme(text: &str, name: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    let name_token = format!("{:?}", name);

    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            if let Some((start, form_end)) = find_define_theme_range(&lines, b, e, &name_token) {
                let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
                out.drain(start..=form_end);
                let mut result = out.join("\n");
                if text.ends_with('\n') { result.push('\n'); }
                result
            } else {
                text.to_string()
            }
        }
        _ => text.to_string(),
    }
}

/// Find the last line index of the Scheme form starting at `start`, bounded by `end`.
/// Counts open parens to find closure; falls back to `start` for single-line forms.
fn find_form_end(lines: &[&str], start: usize, bound: usize) -> usize {
    let mut depth = 0i32;
    for (i, line) in lines[start..bound].iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            return start + i;
        }
    }
    start
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let dir = path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "config path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".config_scm_theme_tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Role;

    fn writer_in(dir: &Path) -> ThemeWriter {
        ThemeWriter::new(dir.join("config.scm"))
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wasdf-theme-writer-test-{tag}-{}", std::process::id()))
    }

    fn style(fg: Color, bg: Color, bold: bool, italic: bool, underline: bool) -> RoleStyle {
        RoleStyle { fg, bg, bold, italic, underline }
    }

    #[test]
    fn upsert_theme_writes_the_full_six_token_form_with_a_real_fg_and_nil_bg() {
        let dir = scratch_dir("full");
        let w = writer_in(&dir);
        let t = NamedTheme {
            name: "custom".into(),
            preset: false,
            overrides: vec![(Role::EntryDir, style(Color::Rgb(0x83, 0xa5, 0x98), Color::Reset, true, false, false))],
        };
        w.upsert_theme(&t).unwrap();
        let text = std::fs::read_to_string(dir.join("config.scm")).unwrap();
        assert!(text.contains(r##"(entry-dir "#83a598" nil #t #f #f)"##), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_theme_writes_nil_for_a_reset_fg_and_a_real_bg() {
        let dir = scratch_dir("bg");
        let w = writer_in(&dir);
        let t = NamedTheme {
            name: "custom".into(),
            preset: false,
            overrides: vec![(Role::Nameplate, style(Color::Reset, Color::Rgb(0, 0, 0), false, false, true))],
        };
        w.upsert_theme(&t).unwrap();
        let text = std::fs::read_to_string(dir.join("config.scm")).unwrap();
        assert!(text.contains(r##"(nameplate nil "#000000" #f #f #t)"##), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_theme_writes_one_line_per_overridden_role() {
        let dir = scratch_dir("multi");
        let w = writer_in(&dir);
        let t = NamedTheme {
            name: "custom".into(),
            preset: false,
            overrides: vec![
                (Role::Border, style(Color::Rgb(0x10, 0x20, 0x30), Color::Reset, false, false, false)),
                (Role::Title, style(Color::Reset, Color::Reset, false, true, false)),
            ],
        };
        w.upsert_theme(&t).unwrap();
        let text = std::fs::read_to_string(dir.join("config.scm")).unwrap();
        assert!(text.contains(r##"(border "#102030" nil #f #f #f)"##), "{text}");
        assert!(text.contains("(title nil nil #f #t #f)"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
