//! Atomic persistence of settings into the delimited managed block in config.scm.
//! The writer only touches the block it owns; hand-written forms outside it are
//! preserved. Writes are atomic: a temp sibling is written first, then renamed
//! over the original so a crash never truncates the user's config.

use std::io;
use std::path::{Path, PathBuf};

const BLOCK_BEGIN: &str =
    ";; >>> wasdf settings (managed — edited via the SETTINGS panel; do not hand-edit)";
const BLOCK_END: &str = ";; <<< wasdf settings";

pub struct ConfigWriter {
    path: PathBuf,
}

impl ConfigWriter {
    pub fn new(path: PathBuf) -> Self {
        ConfigWriter { path }
    }

    /// Insert or replace `(set-option "key" "val")` in the managed block.
    pub fn upsert_option(&self, key: &str, val: &str) -> io::Result<()> {
        let new_line = format!("(set-option {:?} {:?})", key, val);
        self.upsert(|line| is_set_option_for(line, key), &new_line)
    }

    /// Insert or replace the bound key for `intent` in the managed block.
    /// Identity is `(mode, panel, intent)` — editing a key replaces the existing
    /// binding rather than stacking a duplicate.
    pub fn upsert_bind(
        &self,
        mode: &str,
        panel: Option<&str>,
        key: &str,
        intent: &str,
    ) -> io::Result<()> {
        let panel_tok = panel.unwrap_or("#f");
        let new_line = format!("(bind {} {} {:?} {})", mode, panel_tok, key, intent);
        self.upsert(|line| is_bind_for(line, mode, panel, intent), &new_line)
    }

    fn upsert(&self, is_match: impl Fn(&str) -> bool, new_line: &str) -> io::Result<()> {
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)?
        } else {
            String::new()
        };
        let updated = upsert_in_block(&text, is_match, new_line);
        atomic_write(&self.path, &updated)
    }
}

fn upsert_in_block(text: &str, is_match: impl Fn(&str) -> bool, new_line: &str) -> String {
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
            if text.ends_with('\n') {
                result.push('\n');
            }
            result
        }
        _ => {
            // No block yet: append it at EOF.
            let base = text.trim_end_matches('\n');
            if base.is_empty() {
                format!("{BLOCK_BEGIN}\n{new_line}\n{BLOCK_END}\n")
            } else {
                format!("{base}\n{BLOCK_BEGIN}\n{new_line}\n{BLOCK_END}\n")
            }
        }
    }
}

fn is_set_option_for(line: &str, key: &str) -> bool {
    let t = line.trim();
    t.starts_with("(set-option") && t.contains(&format!("{:?}", key))
}

fn is_bind_for(line: &str, mode: &str, panel: Option<&str>, intent: &str) -> bool {
    let t = line.trim();
    if !t.starts_with("(bind ") {
        return false;
    }
    let panel_tok = panel.unwrap_or("#f");
    let expected_prefix = format!("(bind {} {}", mode, panel_tok);
    let expected_suffix = format!(" {})", intent);
    t.starts_with(&expected_prefix) && t.ends_with(&expected_suffix)
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "config path has no parent"))?;
    // Use the same directory so the rename is atomic (same filesystem).
    let tmp = dir.join(".config_scm_tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_config_path() -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("wasdf-writer-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.scm")
    }

    #[test]
    fn upsert_into_empty_creates_block() {
        let path = temp_config_path();
        let w = ConfigWriter::new(path.clone());
        w.upsert_option("border-style", "rounded").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(BLOCK_BEGIN), "block header present");
        assert!(content.contains(BLOCK_END), "block footer present");
        assert!(content.contains(r#"(set-option "border-style" "rounded")"#));
    }

    #[test]
    fn upsert_same_key_replaces_no_duplicate() {
        let path = temp_config_path();
        let w = ConfigWriter::new(path.clone());
        w.upsert_option("border-style", "rounded").unwrap();
        w.upsert_option("border-style", "double").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"(set-option "border-style" "double")"#));
        assert!(!content.contains(r#"(set-option "border-style" "rounded")"#));
        assert_eq!(content.matches("border-style").count(), 1, "no duplicates");
    }

    #[test]
    fn upsert_different_key_appends_inside_block() {
        let path = temp_config_path();
        let w = ConfigWriter::new(path.clone());
        w.upsert_option("border-style", "rounded").unwrap();
        w.upsert_option("content-auto-update", "#t").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"(set-option "border-style" "rounded")"#));
        assert!(content.contains("(set-option \"content-auto-update\" \"#t\")"));
        // Both must be inside the managed block (between the markers).
        let b = content.find(BLOCK_BEGIN).unwrap();
        let e = content.find(BLOCK_END).unwrap();
        assert!(content.find("border-style").unwrap() > b);
        assert!(content.find("border-style").unwrap() < e);
        assert!(content.find("content-auto-update").unwrap() > b);
        assert!(content.find("content-auto-update").unwrap() < e);
    }

    #[test]
    fn hand_written_lines_outside_block_preserved() {
        let path = temp_config_path();
        std::fs::write(
            &path,
            "; hand-written comment\n(set-option \"border-style\" \"single\")\n",
        )
        .unwrap();
        let w = ConfigWriter::new(path.clone());
        w.upsert_option("border-style", "rounded").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("; hand-written comment"), "comment preserved");
        assert!(
            content.contains(r#"(set-option "border-style" "single")"#),
            "hand-written form preserved"
        );
        assert!(
            content.contains(r#"(set-option "border-style" "rounded")"#),
            "managed form written"
        );
    }
}
