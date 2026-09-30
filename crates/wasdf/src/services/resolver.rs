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

/// Whether resolving `req` would silently overwrite an existing path — the
/// same condition `cp`/`mv -i` prompt on. Checked dynamically (unlike
/// `ResolverEntry::destructive`, a static per-op flag): copy/move/rename only
/// destroy data when the *destination happens to already exist*, never
/// otherwise, so this cannot be a fixed property of the operation. Not
/// evaluated for any other op — `mkdir -p` is idempotent and `touch` never
/// erases a file's content, so neither one destroys anything by "overwriting".
pub fn would_overwrite(req: &ResolverRequest, cwd: &Path) -> bool {
    match req.op.as_str() {
        "copy" | "move" => {
            let Some(dst) = req.dst.as_deref() else { return false };
            let dst_path = make_absolute(dst, cwd);
            if dst_path.is_dir() {
                // Real cp/mv semantics: each source lands as dst/basename(src).
                req.paths.iter().any(|src| {
                    src.file_name().map(|name| dst_path.join(name).exists()).unwrap_or(false)
                })
            } else {
                dst_path.exists()
            }
        }
        "rename" => {
            let Some(dst) = req.dst.as_deref() else { return false };
            let dst_path = make_absolute(dst, cwd);
            // Renaming onto the exact same path is a no-op, not an overwrite.
            if req.src.as_deref() == Some(dst_path.as_path()) {
                return false;
            }
            dst_path.exists()
        }
        _ => false,
    }
}

/// True when `cmd` is an absolute runnable file or is found on PATH.
pub(crate) fn is_runnable(cmd: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ResolverChain {
        let mut c = ResolverChain::new();
        c.extend(crate::app::defaults::resolvers());
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wasdf-resolver-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_onto_an_existing_file_needs_confirmation() {
        let dir = scratch_dir("copy-existing-file");
        std::fs::write(dir.join("dst.txt"), "old").unwrap();
        let req = ResolverRequest {
            op: "copy".into(),
            src: None,
            dst: Some("dst.txt".into()),
            path: None,
            paths: vec![PathBuf::from("/somewhere/src.txt")],
            opts: vec![],
            label: "copy".into(),
        };
        assert!(would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_to_a_new_name_needs_no_confirmation() {
        let dir = scratch_dir("copy-new-name");
        let req = ResolverRequest {
            op: "copy".into(),
            src: None,
            dst: Some("brand-new.txt".into()),
            path: None,
            paths: vec![PathBuf::from("/somewhere/src.txt")],
            opts: vec![],
            label: "copy".into(),
        };
        assert!(!would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_into_an_existing_directory_checks_each_sources_basename() {
        let dir = scratch_dir("copy-into-dir");
        let target_dir = dir.join("dest");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("a.txt"), "old").unwrap();
        let req = ResolverRequest {
            op: "copy".into(),
            src: None,
            dst: Some("dest".into()),
            path: None,
            paths: vec![PathBuf::from("/somewhere/a.txt"), PathBuf::from("/somewhere/b.txt")],
            opts: vec![],
            label: "copy".into(),
        };
        // a.txt already exists under dest/, even though b.txt does not.
        assert!(would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_onto_an_existing_file_needs_confirmation() {
        let dir = scratch_dir("move-existing");
        std::fs::write(dir.join("dst.txt"), "old").unwrap();
        let req = ResolverRequest {
            op: "move".into(),
            src: None,
            dst: Some("dst.txt".into()),
            path: None,
            paths: vec![PathBuf::from("/somewhere/src.txt")],
            opts: vec![],
            label: "move".into(),
        };
        assert!(would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_onto_an_existing_name_needs_confirmation() {
        let dir = scratch_dir("rename-existing");
        std::fs::write(dir.join("taken.txt"), "old").unwrap();
        let req = ResolverRequest {
            op: "rename".into(),
            src: Some(dir.join("original.txt")),
            dst: Some("taken.txt".into()),
            path: None,
            paths: vec![],
            opts: vec![],
            label: "rename".into(),
        };
        assert!(would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_onto_itself_needs_no_confirmation() {
        let dir = scratch_dir("rename-noop");
        std::fs::write(dir.join("same.txt"), "content").unwrap();
        let req = ResolverRequest {
            op: "rename".into(),
            src: Some(dir.join("same.txt")),
            dst: Some(dir.join("same.txt").to_string_lossy().into_owned()),
            path: None,
            paths: vec![],
            opts: vec![],
            label: "rename".into(),
        };
        assert!(!would_overwrite(&req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mkdir_and_touch_never_need_overwrite_confirmation() {
        let dir = scratch_dir("mkdir-touch");
        std::fs::write(dir.join("existing"), "x").unwrap();
        let mkdir_req = ResolverRequest {
            op: "mkdir".into(),
            src: None,
            dst: None,
            path: Some("existing".into()),
            paths: vec![],
            opts: vec![],
            label: "mkdir".into(),
        };
        let touch_req = ResolverRequest { op: "touch".into(), ..mkdir_req.clone() };
        assert!(!would_overwrite(&mkdir_req, &dir));
        assert!(!would_overwrite(&touch_req, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
