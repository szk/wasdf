//! The resolver chain: turns a ResolverRequest (operation key + args) into a
//! concrete argv with placeholders expanded, plus the destructive flag from the
//! matched entry. Candidates fall back by OS match and executable runnability;
//! a non-zero exit after spawn is final and never re-enters the chain.

use std::path::{Path, PathBuf};

use crate::core::ResolverRequest;

/// A placeholder slot in a candidate's argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Src,
    Dst,
    Path,
    Paths,
    /// Expands the request's chosen option tokens (`-r`, `-v`, …) in order.
    Opts,
}

/// A user-selectable command-line option declared on a resolver entry. The TUI
/// flag-picker offers these; the chosen `token`s fill the request's `opts`.
#[derive(Debug, Clone, PartialEq)]
pub struct OptDef {
    pub token: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArgEl {
    Lit(String),
    Ph(Slot),
}

/// The OS/tooling a candidate targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Uutils,
    Native,
    NativeMacos,
    NativeLinux,
}

impl Kind {
    fn os_matches(self) -> bool {
        match self {
            Kind::Uutils | Kind::Native => true,
            Kind::NativeMacos => cfg!(target_os = "macos"),
            Kind::NativeLinux => cfg!(target_os = "linux"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    pub argv: Vec<ArgEl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolverEntry {
    pub key: String,
    pub destructive: bool,
    /// Selectable options offered by the TUI flag-picker for this operation.
    pub options: Vec<OptDef>,
    pub candidates: Vec<Candidate>,
}

/// The resolved, runnable command.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCommand {
    pub argv: Vec<String>,
    pub destructive: bool,
    pub label: String,
}

/// The ordered chain of entries; later entries win on merge.
#[derive(Debug, Default, Clone)]
pub struct ResolverChain {
    entries: Vec<ResolverEntry>,
}

impl ResolverChain {
    pub fn new() -> Self {
        ResolverChain { entries: Vec::new() }
    }

    /// Append entries (later wins on lookup).
    pub fn extend(&mut self, entries: impl IntoIterator<Item = ResolverEntry>) {
        self.entries.extend(entries);
    }

    /// Whether the operation's entry is marked destructive.
    pub fn is_destructive(&self, op: &str) -> bool {
        self.lookup(op).map(|e| e.destructive).unwrap_or(false)
    }

    /// The selectable options declared for an operation (for the command Select).
    pub fn options(&self, op: &str) -> Vec<OptDef> {
        self.lookup(op).map(|e| e.options.clone()).unwrap_or_default()
    }

    /// The executable an op resolves to (the first OS-matching candidate's argv[0]
    /// literal), for extracting its options from `--help`/`man`.
    pub fn command_of(&self, op: &str) -> Option<String> {
        let entry = self.lookup(op)?;
        entry
            .candidates
            .iter()
            .filter(|c| c.kind.os_matches())
            .find_map(|c| match c.argv.first() {
                Some(ArgEl::Lit(s)) => Some(s.clone()),
                _ => None,
            })
    }

    /// The argv skeleton (literals interleaved with placeholders) of the op's
    /// first OS-matching candidate, for previewing the actual command line.
    pub fn command_template(&self, op: &str) -> Vec<ArgEl> {
        self.lookup(op)
            .and_then(|e| e.candidates.iter().find(|c| c.kind.os_matches()))
            .map(|c| c.argv.clone())
            .unwrap_or_default()
    }

    fn lookup(&self, op: &str) -> Option<&ResolverEntry> {
        // Later wins: search from the end.
        self.entries.iter().rev().find(|e| e.key == op)
    }

    /// Resolve a request into a runnable command. `cwd` makes relative dst/path
    /// absolute.
    pub fn resolve(&self, req: &ResolverRequest, cwd: &Path) -> Result<ResolvedCommand, String> {
        let entry = self
            .lookup(&req.op)
            .ok_or_else(|| format!("unresolved operation: {}", req.op))?;
        for cand in &entry.candidates {
            if !cand.kind.os_matches() {
                continue;
            }
            let argv = match expand(&cand.argv, req, cwd) {
                Ok(a) => a,
                Err(e) => return Err(e),
            };
            if argv.is_empty() {
                continue;
            }
            if !is_runnable(&argv[0]) {
                continue;
            }
            return Ok(ResolvedCommand {
                argv,
                destructive: entry.destructive,
                label: req.label.clone(),
            });
        }
        Err(format!("no runnable candidate for {}", req.op))
    }
}

fn expand(template: &[ArgEl], req: &ResolverRequest, cwd: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for el in template {
        match el {
            ArgEl::Lit(s) => out.push(s.clone()),
            ArgEl::Ph(Slot::Src) => {
                let src = req.src.as_ref().ok_or("missing src")?;
                out.push(src.to_string_lossy().into_owned());
            }
            ArgEl::Ph(Slot::Dst) => {
                let dst = req.dst.as_ref().ok_or("missing dst")?;
                out.push(make_absolute(dst, cwd).to_string_lossy().into_owned());
            }
            ArgEl::Ph(Slot::Path) => {
                let path = req.path.as_ref().ok_or("missing path")?;
                out.push(make_absolute(path, cwd).to_string_lossy().into_owned());
            }
            ArgEl::Ph(Slot::Paths) => {
                if req.paths.is_empty() {
                    return Err("no targets".into());
                }
                for p in &req.paths {
                    out.push(p.to_string_lossy().into_owned());
                }
            }
            ArgEl::Ph(Slot::Opts) => out.extend(req.opts.iter().cloned()),
        }
    }
    Ok(out)
}

fn make_absolute(s: &str, cwd: &Path) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() { p } else { cwd.join(p) }
}

/// True when `cmd` is an absolute runnable file or is found on PATH.
fn is_runnable(cmd: &str) -> bool {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return p.is_file();
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if Path::new(dir).join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

/// The embedded default resolver entries (the Scheme `defresolvers` analogue).
pub fn defaults() -> Vec<ResolverEntry> {
    use ArgEl::{Lit, Ph};
    use Kind::Native;
    use Slot::{Dst, Opts, Path, Paths, Src};
    let lit = |s: &str| Lit(s.to_string());
    let opt = |token: &str, label: &str| OptDef { token: token.into(), label: label.into() };
    vec![
        ResolverEntry {
            key: "copy".into(),
            destructive: false,
            options: vec![opt("-v", "verbose"), opt("-n", "no-clobber")],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("cp"), lit("-R"), Ph(Opts), Ph(Paths), Ph(Dst)],
            }],
        },
        ResolverEntry {
            key: "move".into(),
            destructive: false,
            options: vec![opt("-v", "verbose"), opt("-n", "no-clobber")],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("mv"), Ph(Opts), Ph(Paths), Ph(Dst)],
            }],
        },
        ResolverEntry {
            key: "delete".into(),
            destructive: true,
            options: vec![],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("rm"), lit("-rf"), Ph(Paths)],
            }],
        },
        ResolverEntry {
            key: "rename".into(),
            destructive: false,
            options: vec![],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("mv"), Ph(Src), Ph(Dst)],
            }],
        },
        ResolverEntry {
            key: "mkdir".into(),
            destructive: false,
            options: vec![],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("mkdir"), lit("-p"), Ph(Path)],
            }],
        },
        ResolverEntry {
            key: "touch".into(),
            destructive: false,
            options: vec![],
            candidates: vec![Candidate {
                kind: Native,
                argv: vec![lit("touch"), Ph(Path)],
            }],
        },
        ResolverEntry {
            key: "open".into(),
            destructive: false,
            options: vec![],
            candidates: vec![
                Candidate { kind: Kind::NativeMacos, argv: vec![lit("open"), Ph(Path)] },
                Candidate { kind: Kind::NativeLinux, argv: vec![lit("xdg-open"), Ph(Path)] },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ResolverChain {
        let mut c = ResolverChain::new();
        c.extend(defaults());
        c
    }

    #[test]
    fn opts_splice_into_argv_in_order() {
        let req = ResolverRequest {
            op: "copy".into(),
            src: None,
            dst: Some("/dst".into()),
            path: None,
            paths: vec![PathBuf::from("/a")],
            opts: vec!["-v".into(), "-n".into()],
            label: "copy".into(),
        };
        let cmd = chain().resolve(&req, Path::new("/")).unwrap();
        // cp -R <opts> <paths> <dst>
        assert_eq!(cmd.argv, vec!["cp", "-R", "-v", "-n", "/a", "/dst"]);
    }

    #[test]
    fn declared_options_per_op() {
        let c = chain();
        assert_eq!(c.options("copy").len(), 2, "copy declares verbose + no-clobber");
        assert!(c.options("delete").is_empty(), "delete declares none");
        assert!(c.is_destructive("delete"));
    }
}
