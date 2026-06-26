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

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;

use crate::core::{AppState, ExtensionIntent, Intent};
use crate::extension::Extension;
use crate::script::keymap::{Binding, Layer};
use crate::script::{codec, config, sexpr};
use crate::services::command::CommandDef;
use crate::services::resolver::ResolverEntry;

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
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type HandleFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *const c_char;
type CursorFn = unsafe extern "C" fn(*const c_char) -> *const c_char;

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
    handle: Option<HandleFn>,
    cursor: Option<CursorFn>,
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

/// The cursor path a dynamic extension is told about: the File-mode entry, or the
/// file-search candidate. Mirrors the kernel's cursor signature.
fn cursor_path(state: &AppState) -> Option<std::path::PathBuf> {
    use crate::core::Mode;
    match state.mode() {
        Mode::File => state.current_entry().map(|e| e.path.clone()),
        Mode::Select(spec) if spec.id == "file-search" => state
            .select
            .as_ref()
            .and_then(|s| s.view.as_ref())
            .and_then(|v| v.current())
            .map(|e| e.path.clone()),
        _ => None,
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

    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        let Some(f) = self.handle else { return Vec::new() };
        let Ok(name) = CString::new(intent.intent.as_str()) else { return Vec::new() };
        let Ok(data) = CString::new(codec::ext_value_to_scheme(&intent.data)) else {
            return Vec::new();
        };
        call_abi_reply(|| unsafe { f(name.as_ptr(), data.as_ptr()) })
    }

    /// Bridge the cursor-changed event across the ABI: pass the cursor path to
    /// `wasdf_on_cursor_changed`, decode its reply as follow-up intents. The
    /// dynamic extension reacts via the push path (e.g. `show-function-content`).
    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        let Some(f) = self.cursor else { return Vec::new() };
        let Some(path) = cursor_path(state) else { return Vec::new() };
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
            other => return Err(format!("unknown glue section: {other:?}")),
        }
    }
    Ok(OptionalExtension { id, lib, commands, resolvers, keymaps })
}

/// The platform dynamic-library filename for a bare crate lib name, e.g.
/// `wasdf_example_ext` → `libwasdf_example_ext.dylib` (macOS) / `.so` (Linux).
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
    let mut manifests: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("scm"))
            .collect(),
        Err(_) => return Vec::new(),
    };
    manifests.sort();

    let mut loaded: Vec<Box<dyn Extension>> = Vec::new();
    for path in manifests {
        match load_one(&path) {
            Ok(ext) => {
                if loaded.iter().any(|e| e.id() == ext.id()) {
                    eprintln!("extension id collision ({}); disabling {}", ext.id(), path.display());
                    continue;
                }
                loaded.push(ext);
            }
            Err(e) => eprintln!("skipping extension {}: {e}", path.display()),
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

    // Purely declarative manifest: no native library, no handlers.
    let Some(lib_name) = parsed.lib.clone() else {
        return Ok(Box::new(DynamicExtension {
            _lib: None,
            id: parsed.id,
            commands: parsed.commands,
            resolvers: parsed.resolvers,
            keymaps: parsed.keymaps,
            handle: None,
            cursor: None,
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
        // The intent handler and the cursor-changed hook are both optional.
        let handle: Option<HandleFn> =
            lib.get::<HandleFn>(b"wasdf_handle_intent").ok().map(|s| *s);
        let cursor: Option<CursorFn> =
            lib.get::<CursorFn>(b"wasdf_on_cursor_changed").ok().map(|s| *s);
        Ok::<_, String>(DynamicExtension {
            _lib: Some(lib),
            id: parsed.id.clone(),
            commands: parsed.commands.clone(),
            resolvers: parsed.resolvers.clone(),
            keymaps: parsed.keymaps.clone(),
            handle,
            cursor,
        })
    })
    .map_err(|_| "extension panicked during load".to_string())??;
    Ok(Box::new(built))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_declarative_glue() {
        let glue = "(extension \"demo\" \
            (commands ((demo-refresh \"demo: refresh\" refresh))) \
            (resolvers ((demo:echo #f ((native \"echo\" \"hi\"))))))";
        let ext = parse_glue(glue).unwrap();
        assert_eq!(ext.id, "demo");
        assert_eq!(ext.commands.len(), 1);
        assert_eq!(ext.commands[0].name, "demo-refresh");
        assert_eq!(ext.resolvers[0].key, "demo:echo");
    }

    #[test]
    fn rejects_non_extension_glue() {
        assert!(parse_glue("(nonsense)").is_err());
    }

    #[test]
    fn parses_lib_section() {
        let ext = parse_glue("(extension \"demo\" (lib \"demo_ext\") (keymaps ((file file ((\"g\" (ext \"demo\" \"x\")))))))").unwrap();
        assert_eq!(ext.id, "demo");
        assert_eq!(ext.lib.as_deref(), Some("demo_ext"), "the named native library");
    }

    #[test]
    fn declarative_only_manifest_loads_without_a_library() {
        // A `.scm` manifest with no `(lib …)` is a pure-declarative extension:
        // it wires a core intent (no Rust, no dylib needed).
        let dir = std::env::temp_dir().join(format!("wasdf-decl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("decl.scm"),
            "(extension \"decl\" (keymaps ((file file ((\"z\" refresh))))))",
        )
        .unwrap();
        let exts = load_optional(&dir);
        assert_eq!(exts.len(), 1, "the declarative manifest loaded with no library");
        assert_eq!(exts[0].id(), "decl");
        assert_eq!(exts[0].keymaps().len(), 1);
        assert!(exts[0].handle_intent(
            &ExtensionIntent { extension: "decl".into(), intent: "x".into(), data: crate::core::ExtensionValue::Nil },
            &AppState::new(dir.clone())
        ).is_empty(), "no native handler");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end smoke test: load the *real* compiled `wasdf-example-ext`
    /// dynamic library through the actual loader (dlopen → ABI version check →
    /// glue parse → symbol bind), then exercise the full bridge: the `g` key resolves
    /// to the extension's intent, and a real cross-library `handle_intent` call
    /// returns the decoded follow-up. Self-skips when the artifact is not built
    /// (so a plain `cargo test` stays green); build it first with
    /// `cargo build -p wasdf-example-ext`.
    #[test]
    fn loads_real_example_ext_dylib_end_to_end() {
        use crate::core::{AppState, ExtensionIntent, ExtensionValue, Key, KeyCode, Mods};
        use crate::script::condition::Conditions;
        use crate::script::keymap::KeymapRegistry;

        let ext_suffix = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let artifact = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(format!("libwasdf_example_ext.{ext_suffix}"));
        if !artifact.exists() {
            eprintln!("smoke skipped: {} not built", artifact.display());
            return;
        }

        // Isolate the extension in a temp directory the loader scans: the shipped
        // `.scm` manifest plus the library under the `lib<name>` filename the
        // manifest's `(lib …)` resolves to.
        let dir = std::env::temp_dir().join(format!("wasdf-smoke-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&artifact, dir.join(format!("libwasdf_example_ext.{ext_suffix}"))).unwrap();
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../wasdf-example-ext/example.scm");
        std::fs::copy(&manifest, dir.join("example.scm")).unwrap();

        let exts = load_optional(&dir);
        assert_eq!(exts.len(), 1, "the example extension loaded");
        let ext = &exts[0];
        assert_eq!(ext.id(), "example");

        // Phase A: the `g` binding crossed the ABI and resolves to the ext intent.
        let mut km = KeymapRegistry::new();
        km.extend(ext.keymaps());
        let conds = Conditions::default();
        let state = AppState::new(dir.clone());
        let got = km.resolve(&conds, &state, "file", Key { code: KeyCode::Char('g'), mods: Mods::NONE });
        match got {
            Some(Intent::Extension(ei)) => {
                assert_eq!((ei.extension.as_str(), ei.intent.as_str()), ("example", "greet"));
            }
            other => panic!("g did not route to the extension: {other:?}"),
        }

        // The real cross-library handle_intent round-trip: greet returns a
        // ShowFunctionContent intent carrying styled lines decoded from Scheme.
        let ei = ExtensionIntent {
            extension: "example".into(),
            intent: "greet".into(),
            data: ExtensionValue::Nil,
        };
        match ext.handle_intent(&ei, &state).as_slice() {
            [Intent::ShowFunctionContent { owner, content: crate::core::PanelContent::Lines { lines, styles } }] => {
                assert_eq!(owner, "example");
                assert_eq!(lines.first().map(String::as_str), Some("GREET"));
                assert_eq!(styles[0][0].len, 5, "the GREET run covers 5 bytes");
            }
            other => panic!("greet did not produce function content: {other:?}"),
        }

        // The `step` intent drives the extension's own state and writes the
        // kernel view state across the ABI: it returns UpdateFunctionView + the
        // refreshed content, both decoded from the Scheme reply.
        let step = ExtensionIntent { extension: "example".into(), intent: "step".into(), data: ExtensionValue::Nil };
        match ext.handle_intent(&step, &state).as_slice() {
            [Intent::UpdateFunctionView(ExtensionValue::Int(_)), Intent::ShowFunctionContent { owner, .. }] => {
                assert_eq!(owner, "example");
            }
            other => panic!("step did not update the view + content: {other:?}"),
        }

        // The cursor-changed event crosses the boundary. With a cursor
        // entry in File mode, on_cursor_changed calls wasdf_on_cursor_changed and
        // decodes the pushed content reply.
        let mut cursor_state = AppState::new(dir.clone());
        cursor_state.entries = vec![crate::core::Entry {
            path: dir.join("hello.txt"),
            name: "hello.txt".into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
        }];
        match ext.on_cursor_changed(&cursor_state).as_slice() {
            [Intent::ShowFunctionContent { owner, content: crate::core::PanelContent::Lines { lines, .. } }] => {
                assert_eq!(owner, "example");
                assert!(lines.iter().any(|l| l.contains("hello.txt")), "echoes the cursor path: {lines:?}");
            }
            other => panic!("cursor-changed did not push content: {other:?}"),
        }

        drop(exts); // dlclose before we remove the file
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_keymaps_section_and_resolves_to_extension_intent() {
        use crate::core::{AppState, Key, KeyCode, Mods};
        use crate::script::condition::Conditions;
        use crate::script::keymap::KeymapRegistry;

        let glue = "(extension \"demo\" \
            (keymaps ((file file ((\"g\" (ext \"demo\" \"greet\")))))))";
        let ext = parse_glue(glue).unwrap();
        assert_eq!(ext.keymaps.len(), 1);
        let b = &ext.keymaps[0];
        assert_eq!(b.mode, "file");
        assert_eq!(b.panel.as_deref(), Some("file"));
        assert_eq!(b.layer, Layer::Extension);
        assert_eq!(b.key, Key { code: KeyCode::Char('g'), mods: Mods::NONE });
        match &b.intent {
            Intent::Extension(ei) => {
                assert_eq!((ei.extension.as_str(), ei.intent.as_str()), ("demo", "greet"));
            }
            other => panic!("expected an extension intent, got {other:?}"),
        }

        // Parse → register at the Extension layer → resolve the bound key.
        let mut km = KeymapRegistry::new();
        km.extend(ext.keymaps.clone());
        let conds = Conditions::default();
        let state = AppState::new(std::env::temp_dir());
        let got = km.resolve(&conds, &state, "file", Key { code: KeyCode::Char('g'), mods: Mods::NONE });
        assert!(matches!(got, Some(Intent::Extension(_))), "key routes to the extension");
    }
}
