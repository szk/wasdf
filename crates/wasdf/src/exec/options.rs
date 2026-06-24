//! Best-effort extraction of a command's options from its `--help` text, falling
//! back to its `man` page, for the command Select's option checkboxes. The parse
//! is heuristic and cross-platform (GNU `--help`, BSD `man`) — never
//! authoritative; an empty result just means no checkboxes are offered.

use std::collections::HashSet;
use std::process::{Command, Stdio};

/// The options `(token, description)` a command advertises. Tries `cmd --help`
/// first, then `man cmd`; deduped by token and capped.
pub fn extract_options(cmd: &str) -> Vec<(String, String)> {
    let mut opts = parse(&run(cmd, &["--help"]));
    if opts.is_empty() {
        opts = parse(&strip_overstrike(&run("man", &[cmd])));
    }
    dedupe(opts)
}

/// Run a command capturing stdout+stderr (BSD tools print usage to stderr).
fn run(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push('\n');
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(_) => String::new(),
    }
}

/// Remove man's overstrike bold/underline (`X\x08Y` → `Y`).
fn strip_overstrike(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\u{8}' {
            out.pop();
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse `-x` / `--long  description` lines from help/man text into (token, desc).
fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if !t.starts_with('-') {
            continue;
        }
        // Split the flag cluster from the description at the first 2-space gap.
        let (flags, desc) = match t.find("  ") {
            Some(i) => (t[..i].trim(), t[i..].trim()),
            None => (t.trim(), ""),
        };
        // The first flag in the cluster (e.g. `-R, -r, --recursive` → `-R`).
        let first = flags.split([',', ' ', '=', '[']).next().unwrap_or("").trim();
        if !is_flag(first) || matches!(first, "-h" | "--help" | "--version" | "--usage") {
            continue;
        }
        out.push((first.to_string(), truncate(desc, 48)));
    }
    out
}

/// A token that looks like an option: `-` then an alphanumeric.
fn is_flag(s: &str) -> bool {
    s.starts_with('-')
        && s.trim_start_matches('-').chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Keep the first occurrence of each token, capped so the list stays usable.
fn dedupe(opts: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (token, label) in opts {
        if seen.insert(token.clone()) {
            out.push((token, label));
            if out.len() >= 24 {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnu_style_help() {
        let help = "Usage: cp [OPTION]... SOURCE DEST\n\
            \x20 -f, --force                  remove existing destinations\n\
            \x20 -i, --interactive            prompt before overwrite\n\
            \x20 -R, -r, --recursive          copy directories recursively\n\
            \x20     --help     display this help and exit\n";
        let opts = parse(help);
        let tokens: Vec<&str> = opts.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(tokens, vec!["-f", "-i", "-R"], "first flag per line; --help filtered");
        assert_eq!(opts[0].1, "remove existing destinations");
    }

    #[test]
    fn strips_man_overstrike() {
        // man bold: "-_R_" style "R\x08R"; underline "_\x08x".
        assert_eq!(strip_overstrike("R\u{8}Recursive"), "Recursive");
        assert_eq!(strip_overstrike("_\u{8}x_\u{8}y"), "xy");
    }
}
