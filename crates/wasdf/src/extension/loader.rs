//! The optional (dynamically loaded) extension loader. An optional extension is
//! a `.scm` **manifest** in the user's extensions directory — the glue
//! declarations (id, palette commands, resolver entries, Extension-layer keymaps)
//! read **as data** by the codec's s-expression reader — that names its native
//! library via `(lib …)`. The loader loads that library for the intent handlers.
//! A manifest with no `(lib …)` is a purely declarative extension (no Rust; its
//! keymaps/commands wire existing core intents).
//!
//! Loading rules (EXTENSION.md): the named library is loaded only when its API
//! version matches exactly; bundled first then optional in manifest file-name
//! order; an id collision disables the later one; a fault disables the extension
//! and the application continues.
//!
//! Intent handling is bridged across the ABI as strings: the kernel passes the
//! intent id and its ExtensionValue payload (as a Scheme datum), and the library
//! returns follow-up intents as a Scheme list, decoded back into core intents.
//! The manifest is data, not code: it never "runs" — the link from a declared
//! `(ext id intent)` to Rust happens at intent dispatch over the C ABI, by id.
//!
//! A library that also exports `wasdf_vfs_list` gets its own VFS mount at
//! `@/<id>` for free (no manifest field): `DynamicVfsProvider` bridges
//! `Vfs::list` calls to it the same way `handle_intent`/`on_cursor_changed`
//! bridge theirs, decoding the reply via `codec::entries_from_datum_list`.

use std::ffi::{CStr, CString};
use std::io;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::core::{AppState, Entry, ExtensionIntent, ExtensionValue, Intent, OptionStore};
use crate::extension::Extension;
use crate::script::keymap::{Binding, Layer};
use crate::script::{codec, config, sexpr};
use crate::services::command::CommandDef;
use crate::services::resolver::ResolverEntry;
use crate::vfs::VfsProvider;

/// Declarations parsed from an extension's glue string (a `.scm` manifest).
#[derive(Debug, Clone, PartialEq)]
pub struct OptionalExtension {
    pub id: String,
    /// The native library that provides the intent handlers, named in `(lib …)`
    /// (bare crate lib name; the loader adds the platform `lib`/`.so`/`.dylib`
    /// affixes). `None` for a purely declarative extension (no Rust behavior —
    /// its keymaps/commands wire existing core intents).
    pub lib: Option<String>,
    pub commands: Vec<CommandDef>,
    pub resolvers: Vec<ResolverEntry>,
    /// Extension-layer key bindings (intents are typically `(ext …)` forms).
    pub keymaps: Vec<Binding>,
    /// The `(options …)` section re-serialized as an `option_scheme` source
    /// (`(quote ((option …) …))`), or `None` when the manifest declares no
    /// options. Lets a user extension declare configurable settings — appearing
    /// editable under `@/settings/Extensions/<id>` exactly like a bundled one.
    pub options: Option<String>,
    /// The prelude's file name, relative to the manifest, from `(prelude …)`.
    pub prelude: Option<String>,
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type HandleFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *const c_char;
type CursorFn = unsafe extern "C" fn(*const c_char) -> *const c_char;
type ListFn = unsafe extern "C" fn(*const c_char) -> *const c_char;

/// A loaded optional extension. Keeps the library handle alive so its intent
/// handler stays callable, and bridges the kernel Extension trait to the ABI.
struct DynamicExtension {
    /// Kept alive so the bound handler pointers stay callable. `None` for a
    /// purely declarative (.scm-only) extension with no native library.
    _lib: Option<libloading::Library>,
    id: String,
    commands: Vec<CommandDef>,
    resolvers: Vec<ResolverEntry>,
    keymaps: Vec<Binding>,
    options: Option<String>,
    prelude: Option<String>,
    handle: Option<HandleFn>,
    cursor: Option<CursorFn>,
    /// Optional VFS-listing hook: when present, this extension owns the mount
    /// `@/<id>` (see `vfs_providers`).
    list: Option<ListFn>,
    /// The shared option store, stashed by `bind_store` after all extensions are
    /// registered. Lets `handle_intent` hand the extension its own live option
    /// values over the ABI (the `wasdf_sdk::Settings` snapshot the SDK describes).
    store: OnceLock<Arc<OptionStore>>,
}

/// The VFS side of a dynamic extension's `wasdf_vfs_list` hook. Copies the fn
/// pointer out of `DynamicExtension` (safe: both the owning `Library`, kept
/// alive in `ExtensionRegistry`, and the `Vfs` that holds this provider are
/// constructed once at boot and live for the process's lifetime, exactly like
/// `HandleFn`/`CursorFn` already rely on). Mount name is the extension's id —
/// no manifest field needed, and collision-free since ids are already deduped
/// at registration.
struct DynamicVfsProvider {
    id: String,
    list: ListFn,
}

impl VfsProvider for DynamicVfsProvider {
    fn mount(&self) -> &str {
        &self.id
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let Ok(p) = CString::new(path.to_string_lossy().into_owned()) else {
            return Ok(Vec::new());
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ptr = unsafe { (self.list)(p.as_ptr()) };
            if ptr.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
            }
        }))
        .unwrap_or_default();
        match sexpr::parse(&result) {
            Ok(d) => Ok(codec::entries_from_datum_list(&d, path)),
            Err(_) => Ok(Vec::new()),
        }
    }
}

/// Call a string→string ABI entry point under `catch_unwind`, returning the
/// reply (empty on null/fault), then decode it as a follow-up intent list. A
/// fault disables the call, not the application.
fn call_abi_reply(f: impl FnOnce() -> *const c_char) -> Vec<Intent> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ptr = f();
        if ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
        }
    }))
    .unwrap_or_default();
    match sexpr::parse(&result) {
        Ok(d) => codec::intents_from_datum_list(&d),
        Err(_) => Vec::new(),
    }
}

impl DynamicExtension {
    /// This extension's own option values as an `ExtensionValue::Map` — the
    /// snapshot handed to `handle_intent` over the ABI. Empty until `bind_store`
    /// runs (or if the extension declares no options).
    fn option_snapshot(&self) -> ExtensionValue {
        let Some(store) = self.store.get() else { return ExtensionValue::Nil };
        let pairs = store
            .options_for_owner(&self.id)
            .into_iter()
            .map(|d| (d.key.clone(), ExtensionValue::String(store.get_str(&d.key))));
        ExtensionValue::map(pairs)
    }
}

impl Extension for DynamicExtension {
    fn id(&self) -> &str {
        &self.id
    }

    fn commands(&self) -> Vec<CommandDef> {
        self.commands.clone()
    }

    fn keymaps(&self) -> Vec<Binding> {
        self.keymaps.clone()
    }

    fn resolver_entries(&self) -> Vec<ResolverEntry> {
        self.resolvers.clone()
    }

    fn option_scheme(&self) -> Option<String> {
        self.options.clone()
    }

    fn prelude(&self) -> Option<String> {
        self.prelude.clone()
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.store.set(store.clone());
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        match self.list {
            Some(list) => vec![Box::new(DynamicVfsProvider { id: self.id.clone(), list })],
            None => Vec::new(),
        }
    }

    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        let Some(f) = self.handle else { return Vec::new() };
        let Ok(name) = CString::new(intent.intent.as_str()) else { return Vec::new() };
        // Hand the extension its own live option values as the data payload (the
        // ABI's `wasdf_sdk::Settings` snapshot), unless the intent form carried
        // explicit data of its own — in which case that wins.
        let payload = match &intent.data {
            ExtensionValue::Nil => self.option_snapshot(),
            other => other.clone(),
        };
        let Ok(data) = CString::new(codec::ext_value_to_scheme(&payload)) else {
            return Vec::new();
        };
        call_abi_reply(|| unsafe { f(name.as_ptr(), data.as_ptr()) })
    }

    /// Bridge the cursor-changed event across the ABI: pass the cursor path to
    /// `wasdf_on_cursor_changed`, decode its reply as follow-up intents. The
    /// dynamic extension reacts via the push path (e.g. `show-function-content`).
    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        let Some(f) = self.cursor else { return Vec::new() };
        let Some(path) = state.cursor_path() else { return Vec::new() };
        let Ok(p) = CString::new(path.to_string_lossy().into_owned()) else { return Vec::new() };
        call_abi_reply(|| unsafe { f(p.as_ptr()) })
    }
}

/// Parse an extension's glue string:
/// `(extension "id" (commands (…)) (resolvers (…)) (keymaps (…)))`.
/// All sections are optional. Keymap groups use the same shape the embedded
/// keymap config does — `((mode panel (binding…)) …)` — and are registered at
/// the Extension layer; their `:when` predicates are limited to core predicates
/// (a dynamic extension cannot register native predicates across the ABI).
pub fn parse_glue(glue: &str) -> Result<OptionalExtension, String> {
    let datum = sexpr::parse(glue)?;
    let list = datum.as_list().ok_or("glue is not a list")?;
    if list.first().and_then(|d| d.as_sym()) != Some("extension") {
        return Err("glue must start with `extension`".into());
    }
    let id = list.get(1).and_then(|d| d.as_str()).ok_or("glue missing id")?.to_string();
    let mut lib = None;
    let mut commands = Vec::new();
    let mut resolvers = Vec::new();
    let mut keymaps = Vec::new();
    let mut options = None;
    let mut prelude = None;
    for section in &list[2..] {
        let s = section.as_list().ok_or("glue section is not a list")?;
        match s.first().and_then(|d| d.as_sym()) {
            Some("lib") => {
                lib = Some(s.get(1).and_then(|d| d.as_str()).ok_or("lib missing name")?.to_string());
            }
            Some("commands") => {
                commands = config::parse_command_config(s.get(1).ok_or("empty commands")?)?;
            }
            Some("resolvers") => {
                resolvers = config::parse_resolver_config(s.get(1).ok_or("empty resolvers")?)?;
            }
            Some("keymaps") => {
                keymaps = config::parse_keymap_config(
                    s.get(1).ok_or("empty keymaps")?,
                    Layer::Extension,
                )?;
            }
            Some("options") => {
                // Validate now, but store as an `option_scheme` source so the same
                // path bundled extensions use tags them with this extension's id.
                let forms = s.get(1).ok_or("empty options")?;
                config::parse_option_config(forms)?;
                options = Some(format!("(quote {})", sexpr::render(forms)));
            }
            Some("prelude") => {
                prelude = Some(s.get(1).and_then(|d| d.as_str()).ok_or("prelude missing file name")?.to_string());
            }
            other => return Err(format!("unknown glue section: {other:?}")),
        }
    }
    Ok(OptionalExtension { id, lib, commands, resolvers, keymaps, options, prelude })
}

/// The platform dynamic-library filename for a bare crate lib name, e.g.
/// `exampleext` → `libexampleext.dylib` (macOS) / `libexampleext.so` (Linux).
fn lib_filename(name: &str) -> String {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    format!("lib{name}.{ext}")
}

/// Load all optional extensions in `dir`, in file-name order, applying the
/// loading rules. Each extension is a `.scm` **manifest** (the glue declarations,
/// read as data) that names its native library via `(lib …)`; the loader loads
/// that library for the intent handlers. A manifest with no `(lib …)` is a purely
/// declarative extension (no Rust). Returns extension objects whose libraries
/// stay loaded.
pub fn load_optional(dir: &Path) -> Vec<Box<dyn Extension>> {
    // Two manifest layouts are accepted, so a user extension gets its own folder
    // (mirroring the bundled `<name>/` structure) for its manifest + library +
    // assets: a directory `<dir>/<name>/init.scm`, or a flat `<dir>/<name>.scm`.
    let mut manifests: Vec<std::path::PathBuf> = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let init = p.join("init.scm");
                    if init.is_file() {
                        manifests.push(init);
                    }
                } else if p.extension().and_then(|s| s.to_str()) == Some("scm") {
                    manifests.push(p);
                }
            }
        }
        Err(_) => return Vec::new(),
    }
    manifests.sort();

    let mut loaded: Vec<Box<dyn Extension>> = Vec::new();
    for path in manifests {
        match load_one(&path) {
            Ok(ext) => {
                if loaded.iter().any(|e| e.id() == ext.id()) {
                    crate::services::messages::log(format!("extension id collision ({}); disabling {}", ext.id(), path.display()), true);
                    continue;
                }
                loaded.push(ext);
            }
            Err(e) => crate::services::messages::log(format!("skipping extension {}: {e}", path.display()), true),
        }
    }
    loaded
}

/// Load one extension from its `.scm` manifest: read the declarations as data,
/// then (when `(lib …)` is present) dlopen the named library in the same
/// directory and bind its handlers. Faults are caught so a bad extension disables
/// itself rather than the application.
fn load_one(manifest: &Path) -> Result<Box<dyn Extension>, String> {
    let glue = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let parsed = parse_glue(&glue)?;
    let dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    let prelude = match &parsed.prelude {
        Some(file) => Some(
            std::fs::read_to_string(dir.join(file)).map_err(|e| format!("prelude {file}: {e}"))?,
        ),
        None => None,
    };

    // Purely declarative manifest: no native library, no handlers.
    let Some(lib_name) = parsed.lib.clone() else {
        return Ok(Box::new(DynamicExtension {
            _lib: None,
            id: parsed.id,
            commands: parsed.commands,
            resolvers: parsed.resolvers,
            keymaps: parsed.keymaps,
            options: parsed.options,
            prelude,
            handle: None,
            cursor: None,
            list: None,
            store: OnceLock::new(),
        }));
    };

    let lib_path = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(lib_filename(&lib_name));
    let built = std::panic::catch_unwind(|| unsafe {
        let lib = libloading::Library::new(&lib_path)
            .map_err(|e| format!("loading {}: {e}", lib_path.display()))?;
        let abi: libloading::Symbol<AbiVersionFn> =
            lib.get(b"wasdf_abi_version").map_err(|_| "no wasdf_abi_version".to_string())?;
        let version = abi();
        if version != wasdf_sdk::API_VERSION {
            return Err(format!("API version {version} != {}", wasdf_sdk::API_VERSION));
        }
        // The intent handler, the cursor-changed hook, and the VFS-listing hook
        // are all optional.
        let handle: Option<HandleFn> =
            lib.get::<HandleFn>(b"wasdf_handle_intent").ok().map(|s| *s);
        let cursor: Option<CursorFn> =
            lib.get::<CursorFn>(b"wasdf_on_cursor_changed").ok().map(|s| *s);
        let list: Option<ListFn> =
            lib.get::<ListFn>(b"wasdf_vfs_list").ok().map(|s| *s);
        Ok::<_, String>(DynamicExtension {
            _lib: Some(lib),
            id: parsed.id.clone(),
            commands: parsed.commands.clone(),
            resolvers: parsed.resolvers.clone(),
            keymaps: parsed.keymaps.clone(),
            options: parsed.options.clone(),
            prelude: prelude.clone(),
            handle,
            cursor,
            list,
            store: OnceLock::new(),
        })
    })
    .map_err(|_| "extension panicked during load".to_string())??;
    Ok(Box::new(built))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_options_become_an_option_scheme_source() {
        let glue = r#"(extension "e" (options ((option "e.n" "E" (int 1 8) "4" "n"))))"#;
        let parsed = parse_glue(glue).unwrap();
        let opts = parsed.options.expect("options section parsed");
        assert!(opts.starts_with("(quote "), "wrapped as an option_scheme source: {opts}");
        assert!(opts.contains("e.n") && opts.contains("(int 1 8)"), "{opts}");
    }

    #[test]
    fn a_user_extension_folder_with_init_scm_is_discovered() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("wasdf-load-{}", std::process::id()));
        let ext = dir.join("exampleext");
        std::fs::create_dir_all(&ext).unwrap();
        writeln!(
            std::fs::File::create(ext.join("init.scm")).unwrap(),
            r##"(extension "exampleext" (keymaps ((file file (("o" (ext "exampleext" "show-info"))))))
               (options ((option "exampleext.verbose" "exampleext" bool "#f" "verbose"))))"##
        )
        .unwrap();

        let loaded = load_optional(&dir);
        assert_eq!(loaded.len(), 1, "folder/init.scm was discovered");
        assert_eq!(loaded[0].id(), "exampleext");
        // Its declared options flow through the same option_scheme path bundled
        // extensions use, so they appear editable under @/settings/Extensions.
        let scheme = loaded[0].option_scheme().expect("option_scheme from the manifest");
        assert!(scheme.contains("exampleext.verbose"), "{scheme}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
