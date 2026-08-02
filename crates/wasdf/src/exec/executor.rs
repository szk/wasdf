//! The command executor: spawns a resolved argv in the Background form (no output
//! capture, completion notification only) and the Captured form (stdout/stderr
//! collected into the Exec frame).

use std::path::Path;
use std::process::{Command, Stdio};

use crate::core::{ExecOutput, OpDone};

/// Background form: run argv to completion in `cwd`, report only success/failure.
pub fn run_background(label: &str, argv: &[String], cwd: &Path) -> OpDone {
    if argv.is_empty() {
        return OpDone {
            label: label.into(),
            success: false,
            message: Some(format!("{label}: empty command")),
        };
    }
    let result = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match result {
        Ok(out) if out.status.success() => OpDone {
            label: label.into(),
            success: true,
            message: None,
        },
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err.lines().next().unwrap_or("non-zero exit").trim().to_string();
            OpDone {
                label: label.into(),
                success: false,
                message: Some(format!("{label}: {msg}")),
            }
        }
        Err(e) => OpDone {
            label: label.into(),
            success: false,
            message: Some(format!("{label}: {e}")),
        },
    }
}

/// Captured form: run argv in `cwd` and collect stdout+stderr for the Exec frame.
pub fn run_captured(argv: &[String], cwd: &Path) -> ExecOutput {
    if argv.is_empty() {
        return ExecOutput { lines: vec!["empty command".into()], finished: true, exit: Some(1) };
    }
    let result = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match result {
        Ok(out) => {
            let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect();
            for l in String::from_utf8_lossy(&out.stderr).lines() {
                lines.push(l.to_string());
            }
            ExecOutput { lines, finished: true, exit: out.status.code() }
        }
        Err(e) => ExecOutput { lines: vec![format!("spawn failed: {e}")], finished: true, exit: Some(1) },
    }
}
