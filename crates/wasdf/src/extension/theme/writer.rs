//! Persistence of theme selection and custom theme definitions into the
//! managed block in config.scm. Uses the same BLOCK_BEGIN/BLOCK_END markers
//! and atomic-write contract as `settings/writer.rs`.

use std::io;
use std::path::{Path, PathBuf};

use super::registry::NamedTheme;
use crate::core::color_token;

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

    /// Insert or replace the `(define-theme "name" …)` multi-line form.
    pub fn upsert_theme(&self, t: &NamedTheme) -> io::Result<()> {
        let mut body = format!("(define-theme {:?}", t.name);
        for (role, color) in &t.overrides {
            body.push_str(&format!("\n  ({} {})", role.token(), color_token(*color)));
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
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_path() -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("wasdf-theme-writer-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.scm")
    }

    #[test]
    fn set_active_upserts_single_line() {
        let path = temp_path();
        let w = ThemeWriter::new(path.clone());
        w.set_active("nord").unwrap();
        w.set_active("gruvbox").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"(set-theme "gruvbox")"#));
        assert!(!content.contains(r#"(set-theme "nord")"#), "old value replaced");
        assert_eq!(content.matches("set-theme").count(), 1, "no duplicates");
    }

    #[test]
    fn upsert_theme_creates_block_and_form() {
        let path = temp_path();
        let w = ThemeWriter::new(path.clone());
        let t = NamedTheme {
            name: "ocean".into(),
            preset: false,
            overrides: vec![(crate::core::Role::Border, ratatui::style::Color::Blue)],
        };
        w.upsert_theme(&t).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"(define-theme "ocean""#));
        assert!(content.contains("(border blue)"));
    }

    #[test]
    fn upsert_theme_replaces_no_duplicate() {
        let path = temp_path();
        let w = ThemeWriter::new(path.clone());
        let t1 = NamedTheme {
            name: "ocean".into(),
            preset: false,
            overrides: vec![(crate::core::Role::Border, ratatui::style::Color::Blue)],
        };
        w.upsert_theme(&t1).unwrap();
        let t2 = NamedTheme {
            name: "ocean".into(),
            preset: false,
            overrides: vec![(crate::core::Role::Accent, ratatui::style::Color::Red)],
        };
        w.upsert_theme(&t2).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.matches("ocean").count(), 1, "only one define-theme for ocean");
        assert!(content.contains("(accent red)"));
        assert!(!content.contains("(border blue)"), "old form replaced");
    }

    #[test]
    fn remove_theme_deletes_form() {
        let path = temp_path();
        let w = ThemeWriter::new(path.clone());
        let t = NamedTheme {
            name: "ocean".into(),
            preset: false,
            overrides: vec![(crate::core::Role::Accent, ratatui::style::Color::Red)],
        };
        w.upsert_theme(&t).unwrap();
        w.remove_theme("ocean").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("ocean"), "form removed");
    }

    #[test]
    fn hand_written_forms_outside_block_preserved() {
        let path = temp_path();
        std::fs::write(&path, "; user comment\n(set-theme \"hand\")\n").unwrap();
        let w = ThemeWriter::new(path.clone());
        w.set_active("managed").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("; user comment"), "comment preserved");
        assert!(content.contains(r#"(set-theme "hand")"#), "hand-written form preserved");
        assert!(content.contains(r#"(set-theme "managed")"#), "managed form written");
    }
}
