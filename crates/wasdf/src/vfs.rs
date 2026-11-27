//! Virtual File System: a router that exposes any tree-structured data through
//! the same `Entry`/`ReadDir` path that the real filesystem uses. Virtual
//! locations are relative paths under the reserved root token `@`; real
//! locations are always absolute — the distinction is `path.is_absolute()`.
//!
//! Providers (filesystem, bookmarks, …) plug in via the `VfsProvider` trait and
//! are assembled at boot in `Vfs::extend`, shaped like `ResolverChain`.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::core::Entry;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// The first path component of every virtual location.
pub const VFS_ROOT_TOKEN: &str = "@";

/// True iff `p` is a virtual path (relative, first component is `@`).
pub fn is_virtual(p: &Path) -> bool {
    p.is_relative()
        && p.components().next() == Some(Component::Normal(OsStr::new(VFS_ROOT_TOKEN)))
}

/// The VFS root path: `@`.
pub fn vfs_root() -> PathBuf {
    PathBuf::from(VFS_ROOT_TOKEN)
}

/// The mount name (second path component) for virtual path `p`, e.g.
/// `"settings"` for `@/settings/display`. `None` for the root `@` itself or
/// any non-virtual path.
pub fn mount_of(p: &Path) -> Option<&str> {
    if !is_virtual(p) {
        return None;
    }
    let mut comps = p.components();
    comps.next(); // skip `@`
    comps.next().and_then(|c| {
        if let Component::Normal(s) = c { s.to_str() } else { None }
    })
}

/// The third path component for a virtual path: `Some("Display")` for
/// `@/settings/Display`, `None` for `@/settings` itself or shallower paths.
/// Used by `SettingsProvider` to distinguish the category listing from the
/// root listing.
pub fn tail_after_mount(p: &Path) -> Option<&str> {
    if !is_virtual(p) {
        return None;
    }
    let mut comps = p.components();
    comps.next(); // skip `@`
    comps.next(); // skip mount name
    comps.next().and_then(|c| {
        if let Component::Normal(s) = c { s.to_str() } else { None }
    })
}

// ── Provider trait ────────────────────────────────────────────────────────────

/// Adapt any tree-structured data as a navigable list of `Entry`s. Called from
/// the async task thread, so `Send + Sync` is required. Keep `list` cheap
/// (in-memory); it runs on every nav-panel render.
pub trait VfsProvider: Send + Sync {
    /// The mount name (`"filesystem"` / `"bookmarks"` / …), matching the second
    /// component of the paths routed here.
    fn mount(&self) -> &str;

    /// List the children of virtual directory `path`. Each child is either a
    /// subtree (`is_dir = true`) or a leaf (`is_dir = false, activation = Some`).
    fn list(&self, path: &Path) -> io::Result<Vec<Entry>>;
}

// ── Router ────────────────────────────────────────────────────────────────────

/// The VFS router. Shaped like `ResolverChain`: built by `extend` (embedded
/// defaults → extension providers → user), then held as an `Arc` by both the
/// `TaskManager` (for listing) and the renderer (for the nav panel).
#[derive(Default)]
pub struct Vfs {
    providers: Vec<Box<dyn VfsProvider>>,
}


impl Vfs {
    pub fn extend(&mut self, ps: impl IntoIterator<Item = Box<dyn VfsProvider>>) {
        self.providers.extend(ps);
    }

    fn find(&self, mount: &str) -> Option<&dyn VfsProvider> {
        self.providers.iter().find(|p| p.mount() == mount).map(|p| p.as_ref())
    }

    /// The nav panel's ancestor columns for `path`, root first: at each level
    /// the directories beside it (hidden ones only when on the path) and which
    /// one the path takes. Runs in the ReadDir task.
    pub fn trail(&self, path: &Path) -> Vec<crate::core::TrailColumn> {
        use crate::core::TrailColumn;
        if !is_virtual(path) {
            return crate::fs::ancestor_columns(path);
        }
        let mut cols = vec![TrailColumn { names: vec!["@".into()], index: 0 }];
        let mut level = vfs_root();
        for c in path.components() {
            let std::path::Component::Normal(s) = c else { continue };
            if s == std::ffi::OsStr::new("@") {
                continue;
            }
            let siblings = self.list(&level, false).unwrap_or_default();
            level = level.join(s);
            let index = siblings.iter().position(|e| e.path.file_name() == Some(s)).unwrap_or(0);
            cols.push(TrailColumn { names: siblings.into_iter().map(|e| e.name).collect(), index });
        }
        cols
    }

    /// The VFS root listing: one `is_dir = true` entry per registered provider.
    /// Display names are capitalized ("FILESYSTEM", "BOOKMARKS", "SETTINGS");
    /// the filesystem provider's entry uses path `"/"` (real root) so entering
    /// it navigates directly into the real tree.
    fn root_listing(&self) -> Vec<Entry> {
        self.providers
            .iter()
            .map(|p| {
                let mount = p.mount();
                let display = mount.to_uppercase();
                if mount == "filesystem" {
                    Entry::virtual_dir(PathBuf::from("/"), display)
                } else {
                    Entry::virtual_dir(vfs_root().join(mount), display)
                }
            })
            .collect()
    }

    /// One entry point for all listing requests. Virtual paths go to the owning
    /// provider; real (absolute) paths go to the filesystem (unchanged).
    pub fn list(&self, path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
        if path == vfs_root() {
            return Ok(self.root_listing());
        }
        if is_virtual(path) {
            match mount_of(path).and_then(|m| self.find(m)) {
                // Directories first, as on disk; within each group the
                // provider's own order (declaration, priority) is kept.
                Some(p) => p.list(path).map(|mut entries| {
                    entries.sort_by_key(|e| !e.is_dir);
                    entries
                }),
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("unknown VFS mount: {}", path.display()),
                )),
            }
        } else {
            crate::fs::read_directory(path, show_hidden)
        }
    }
}

// ── Embedded providers ────────────────────────────────────────────────────────

/// Build the embedded default providers: just the filesystem. Every other mount
/// (bookmarks, settings, …) is contributed by its owning extension via
/// `Extension::vfs_providers()`.
pub fn defaults() -> Vec<Box<dyn VfsProvider>> {
    vec![Box::new(FilesystemProvider)]
}

// ── FilesystemProvider ────────────────────────────────────────────────────────

/// The reference provider. Its single mount entry in the VFS root listing has
/// `path = "/"` and `is_dir = true`, so entering it navigates directly to the
/// real root. From `/` downward, normal absolute-path I/O takes over.
struct FilesystemProvider;

impl VfsProvider for FilesystemProvider {
    fn mount(&self) -> &str {
        "filesystem"
    }

    fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
        // The filesystem provider does not expose a virtual subtree; its mount
        // entry in the root listing has path = real `/` so entering it goes
        // straight to the real tree.
        Ok(Vec::new())
    }
}


// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathbuf_parent_climbs_virtual_paths_lexically() {
        // PathBuf::parent() is pure lexical, no I/O — virtual paths climb correctly.
        let p = PathBuf::from("@/settings/display");
        assert_eq!(p.parent(), Some(Path::new("@/settings")));
        let p2 = PathBuf::from("@/settings");
        assert_eq!(p2.parent(), Some(Path::new("@")));
        // "@".parent() is Some("") — the empty relative path, not None.
        let root = PathBuf::from("@");
        let parent = root.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        assert!(parent.as_os_str().is_empty(), "parent of @ is the empty path");
    }

    #[test]
    fn a_virtual_listing_puts_directories_first_and_keeps_the_providers_order() {
        struct Mixed;
        impl VfsProvider for Mixed {
            fn mount(&self) -> &str {
                "mixed"
            }
            fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
                let leaf = |n: &str| Entry::virtual_leaf(PathBuf::from(format!("@/mixed/{n}")), n.into());
                let dir = |n: &str| Entry::virtual_dir(PathBuf::from(format!("@/mixed/{n}")), n.into());
                Ok(vec![leaf("about"), dir("Zeta"), leaf("Alpha"), dir("Beta")])
            }
        }
        let mut vfs = Vfs::default();
        vfs.extend([Box::new(Mixed) as Box<dyn VfsProvider>]);
        let names: Vec<String> = vfs.list(Path::new("@/mixed"), false).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Zeta", "Beta", "about", "Alpha"]);
    }

    #[test]
    fn vfs_routes_real_paths_to_filesystem() {
        let vfs = Vfs::default();
        // A real directory: routes to the real FS (must succeed).
        let entries = vfs.list(Path::new("/tmp"), false).unwrap();
        // /tmp always exists on Unix; entries may be empty but must not error.
        let _ = entries;
    }

    /// A minimal extra provider standing in for any extension-owned mount, so
    /// capitalization is proven generic rather than tied to a specific one.
    struct DummyProvider;
    impl VfsProvider for DummyProvider {
        fn mount(&self) -> &str {
            "widgets"
        }
        fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
            Ok(Vec::new())
        }
    }

}
