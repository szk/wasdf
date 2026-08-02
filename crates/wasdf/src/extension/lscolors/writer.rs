//! Persistence of ls-colors palette selection and custom palette definitions
//! into the managed block in config.scm. Same `BLOCK_BEGIN`/`BLOCK_END`
//! markers and atomic-write contract as `theme/writer.rs` / `settings/writer.rs`.

use std::io;
use std::path::{Path, PathBuf};

use super::registry::NamedPalette;

pub const BLOCK_BEGIN: &str =
    ";; >>> wasdf settings (managed — edited via the SETTINGS panel; do not hand-edit)";
pub const BLOCK_END: &str = ";; <<< wasdf settings";

pub struct LsColorsWriter {
    path: PathBuf,
}

impl LsColorsWriter {
    pub fn new(path: PathBuf) -> Self {
        LsColorsWriter { path }
    }

    /// Insert or replace the single `(set-lscolors-palette "name")` form in the managed block.
    pub fn set_active(&self, name: &str) -> io::Result<()> {
        let new_line = format!("(set-lscolors-palette {:?})", name);
        self.upsert_line(|line| line.trim().starts_with("(set-lscolors-palette "), &new_line)
    }

    /// Insert or replace the `(define-lscolors-palette "name" (raw "…"))` form.
    pub fn upsert_palette(&self, p: &NamedPalette) -> io::Result<()> {
        let body = format!("(define-lscolors-palette {:?} (raw {:?}))", p.name, p.raw);
        self.upsert_form(&p.name, &body)
    }

    /// Remove the `(define-lscolors-palette "name" …)` form from the managed block.
    pub fn remove_palette(&self, name: &str) -> io::Result<()> {
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)?
        } else {
            return Ok(());
        };
        let updated = remove_define_palette(&text, name);
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
        let updated = upsert_define_palette(&text, name, new_form);
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

/// Find the line range `[start, end]` (inclusive) of the
/// `(define-lscolors-palette "name" …)` form inside a managed block bounded by
/// `[b+1, e)`. Returns `None` if absent.
fn find_define_palette_range(lines: &[&str], b: usize, e: usize, name_token: &str) -> Option<(usize, usize)> {
    let start = lines[b + 1..e]
        .iter()
        .position(|l| {
            let t = l.trim();
            t.starts_with("(define-lscolors-palette ") && t.contains(name_token)
        })
        .map(|i| i + b + 1)?;
    Some((start, find_form_end(lines, start, e)))
}

/// Insert or replace a `(define-lscolors-palette "name" …)` form in the block.
fn upsert_define_palette(text: &str, name: &str, new_form: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    let name_token = format!("{:?}", name); // "name"

    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            if let Some((start, form_end)) = find_define_palette_range(&lines, b, e, &name_token) {
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

/// Remove the `(define-lscolors-palette "name" …)` form from the block.
fn remove_define_palette(text: &str, name: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    let name_token = format!("{:?}", name);

    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            if let Some((start, form_end)) = find_define_palette_range(&lines, b, e, &name_token) {
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
    let tmp = dir.join(".config_scm_lscolors_tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}
