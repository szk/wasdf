//! Synchronous directory reads, run inside kernel async tasks. Produces the
//! Entry list and the recursive walk used by file-search.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::core::Entry;

/// Read one directory into a sorted Entry list (directories first, then names).
pub fn read_directory(path: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(path)? {
        let dirent = match dirent {
            Ok(d) => d,
            Err(_) => continue,
        };
        let p = dirent.path();
        let name = dirent.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        entries.push(make_entry(p, name));
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// Build an Entry from a path, following symlinks for type/size but recording
/// the link nature and target.
pub fn make_entry(path: PathBuf, name: String) -> Entry {
    let link_meta = std::fs::symlink_metadata(&path).ok();
    let is_symlink = link_meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false);
    let symlink_target = if is_symlink {
        std::fs::read_link(&path).ok()
    } else {
        None
    };
    let meta = std::fs::metadata(&path).ok().or(link_meta);
    let (is_dir, size, mode, uid, gid, modified, created, accessed) = match &meta {
        Some(m) => (
            m.is_dir(),
            m.len(),
            m.permissions().mode(),
            m.uid(),
            m.gid(),
            m.modified().ok(),
            m.created().ok(),
            m.accessed().ok(),
        ),
        None => (false, 0, 0, 0, 0, None, None, None),
    };
    Entry { path, name, is_dir, is_symlink, size, mode, uid, gid, modified, created, accessed, symlink_target }
}

/// A bounded recursive walk yielding file paths relative-friendly for ranking.
/// Skips hidden entries unless `show_hidden`, and caps total results.
pub fn walk(root: &Path, show_hidden: bool, cap: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for dirent in rd.flatten() {
            let name = dirent.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let p = dirent.path();
            let is_dir = dirent.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(p.clone());
            }
            out.push(p);
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}
