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

/// Build an Entry reusing already-obtained symlink metadata (from stage 2 of
/// the affinity re-rank). Avoids re-issuing `symlink_metadata`; still calls
/// `metadata` (follow) to get the resolved kind/size/mode for symlinked targets.
pub fn make_entry_from(path: PathBuf, name: String, symlink_meta: Option<std::fs::Metadata>) -> Entry {
    let link_meta = symlink_meta;
    let is_symlink = link_meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false);
    let symlink_target = if is_symlink { std::fs::read_link(&path).ok() } else { None };
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
    let (owner, group) = (names::user(uid), names::group(gid));
    Entry { path, name, is_dir, is_symlink, size, mode, uid, gid, owner, group, modified, created, accessed, symlink_target, activation: None }
}

/// User and group names by id, memoised for the process: a directory holds
/// few distinct ids, and on macOS a lookup may go to the directory service.
mod names {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    type Memo = Mutex<HashMap<u32, Arc<str>>>;

    fn memo(table: &'static OnceLock<Memo>, id: u32, lookup: fn(u32) -> Option<String>) -> Arc<str> {
        let map = table.get_or_init(Default::default);
        if let Some(n) = map.lock().ok().and_then(|m| m.get(&id).cloned()) {
            return n;
        }
        let name: Arc<str> = lookup(id).unwrap_or_default().into();
        if let Ok(mut m) = map.lock() {
            m.insert(id, name.clone());
        }
        name
    }

    pub fn user(uid: u32) -> Arc<str> {
        static USERS: OnceLock<Memo> = OnceLock::new();
        memo(&USERS, uid, |uid| {
            // SAFETY: getpwuid returns null or a pointer to a static record
            // whose name is a NUL-terminated string; it is copied at once.
            unsafe {
                let pw = libc::getpwuid(uid as libc::uid_t);
                if pw.is_null() { None } else { std::ffi::CStr::from_ptr((*pw).pw_name).to_str().ok().map(str::to_string) }
            }
        })
    }

    pub fn group(gid: u32) -> Arc<str> {
        static GROUPS: OnceLock<Memo> = OnceLock::new();
        memo(&GROUPS, gid, |gid| {
            // SAFETY: as for getpwuid above.
            unsafe {
                let gr = libc::getgrgid(gid as libc::gid_t);
                if gr.is_null() { None } else { std::ffi::CStr::from_ptr((*gr).gr_name).to_str().ok().map(str::to_string) }
            }
        })
    }
}

/// Build an Entry from a path, following symlinks for type/size but recording
/// the link nature and target.
pub fn make_entry(path: PathBuf, name: String) -> Entry {
    let link_meta = std::fs::symlink_metadata(&path).ok();
    make_entry_from(path, name, link_meta)
}

/// The ancestor columns of a real directory, root first (see `Vfs::trail`).
pub fn ancestor_columns(path: &Path) -> Vec<crate::core::TrailColumn> {
    use crate::core::TrailColumn;
    let mut cols = vec![TrailColumn { names: vec!["/".into()], index: 0 }];
    let mut parent = PathBuf::from("/");
    for c in path.components() {
        let std::path::Component::Normal(s) = c else { continue };
        let sel = s.to_string_lossy().into_owned();
        let mut names: Vec<String> = std::fs::read_dir(&parent)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.') || *n == sel)
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        let index = names.iter().position(|n| *n == sel).unwrap_or_else(|| {
            names.push(sel.clone());
            names.len() - 1
        });
        cols.push(TrailColumn { names, index });
        parent.push(s);
    }
    cols
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_carry_their_owner_and_group_names() {
        let e = make_entry(std::env::temp_dir(), "tmp".into());
        // SAFETY: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        if e.uid == me {
            assert!(!e.owner.is_empty(), "the current user has a name");
        }
        assert!(std::sync::Arc::ptr_eq(&names::user(e.uid), &names::user(e.uid)), "memoised");
    }

    #[test]
    fn ancestor_columns_name_each_level_and_mark_the_path() {
        let dir = std::env::temp_dir().join(format!("wasdf-trail-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        let path = dir.join("b");
        let cols = ancestor_columns(&path);
        assert_eq!(cols[0].names, ["/"]);
        assert_eq!(cols.len(), path.components().count(), "one column per level");
        let last = cols.last().unwrap();
        assert_eq!(last.names[last.index], "b");
        assert!(!last.names.iter().any(|n| n == ".hidden"), "hidden siblings stay out");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
