//! The command executor: spawns a resolved argv in the Background form (no output
//! capture, completion notification only) and the Captured form (stdout/stderr
//! collected into the Exec frame).

use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::core::{ExecOutput, OpDone};

/// Run `argv` to completion in `cwd` with no stdin and stderr captured;
/// `Err` for an empty argv or a failed spawn.
fn run(argv: &[String], cwd: &Path, stdout: Stdio) -> Result<Output, String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())
}

/// Background form: run argv to completion in `cwd`, report only success/failure.
pub fn run_background(label: &str, argv: &[String], cwd: &Path) -> OpDone {
    let failure = match run(argv, cwd, Stdio::null()) {
        Ok(out) if out.status.success() => None,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            Some(err.lines().next().unwrap_or("non-zero exit").trim().to_string())
        }
        Err(e) => Some(e),
    };
    OpDone { label: label.into(), success: failure.is_none(), message: failure.map(|m| format!("{label}: {m}")) }
}

/// Captured form: run argv in `cwd` and collect stdout+stderr for the Exec frame.
pub fn run_captured(argv: &[String], cwd: &Path) -> ExecOutput {
    match run(argv, cwd, Stdio::piped()) {
        Ok(out) => {
            let lines = [&out.stdout, &out.stderr]
                .into_iter()
                .flat_map(|bytes| String::from_utf8_lossy(bytes).lines().map(str::to_string).collect::<Vec<_>>())
                .collect();
            ExecOutput { lines, finished: true, exit: out.status.code() }
        }
        Err(e) if argv.is_empty() => ExecOutput { lines: vec![e], finished: true, exit: Some(1) },
        Err(e) => ExecOutput { lines: vec![format!("spawn failed: {e}")], finished: true, exit: Some(1) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn background_reports_success_the_first_stderr_line_or_the_spawn_error() {
        let cwd = std::env::temp_dir();
        let ok = run_background("op", &argv(&["true"]), &cwd);
        assert!(ok.success && ok.message.is_none());
        let fail = run_background("op", &argv(&["sh", "-c", "echo boom >&2; echo more >&2; exit 3"]), &cwd);
        assert!(!fail.success);
        assert_eq!(fail.message.as_deref(), Some("op: boom"));
        assert_eq!(run_background("op", &[], &cwd).message.as_deref(), Some("op: empty command"));
        assert!(run_background("op", &argv(&["/no/such/wasdf-bin"]), &cwd).message.unwrap().starts_with("op: "));
    }

    #[test]
    fn captured_collects_stdout_then_stderr_with_the_exit_code() {
        let cwd = std::env::temp_dir();
        let out = run_captured(&argv(&["sh", "-c", "echo a; echo b >&2; exit 2"]), &cwd);
        assert_eq!(out.lines, ["a", "b"]);
        assert_eq!(out.exit, Some(2));
        assert_eq!(run_captured(&[], &cwd).lines, ["empty command"]);
        assert!(run_captured(&argv(&["/no/such/wasdf-bin"]), &cwd).lines[0].starts_with("spawn failed: "));
    }
}
