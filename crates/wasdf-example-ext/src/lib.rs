//! `exampleext` — a minimal optional (dynamically loaded) wasdf extension that
//! **toggles** the cursor entry's info on the function panel with `o`.
//!
//! It demonstrates that a **fat extension owns its behavior**: the file-info
//! logic and the toggle state live here (this library), not in the kernel.
//! Nothing feature-specific was added to core / codec / the keymap for it — it
//! rides entirely on generic seams over the existing C ABI:
//!
//!   - `wasdf_on_cursor_changed(path)` records the path now under the cursor
//!     (returning no content, so the panel is untouched until asked);
//!   - `wasdf_handle_intent("show-info", data)` toggles an internal flag. On the
//!     first press it pushes an **overlay mode** (`push-overlay`) — so `wasd`
//!     still navigates the file list underneath — and shows the cursor entry's
//!     info as a logical `show-function-content` document (core owns the layout);
//!     on the next press it pops that mode (`pop-mode`) and the previewer reclaims
//!     the panel. Either way it also returns a `(notify "<greeting>: ON|OFF")`
//!     carrying the configured `exampleext.greeting` (which the kernel hands in as
//!     the data payload — its `wasdf_sdk::Settings` snapshot) with the ON/OFF
//!     suffix the extension appends from its own state.
//!   - `wasdf_vfs_list(path)` gives this extension its own VFS mount at
//!     `@/exampleext` (no manifest field — the mount name is always the
//!     extension's id) with a single static leaf, `About`, whose activation
//!     re-dispatches the very same `(ext "exampleext" "show-info")` intent `o`
//!     does — proving a dynamic extension's mount is an ordinary source of
//!     `(ext …)` activations, not a separate mechanism.
//!
//! Because turning ON focuses the function panel, the `o` binding is declared for
//! both the file and function panels (see `init.scm`), so the next `o` toggles it
//! back off regardless of which panel holds focus.
//!
//! The `o` key → `(ext "exampleext" "show-info")` binding and the extension's
//! options live in the sibling `init.scm` manifest. Build this crate as a
//! dynamic library and drop `libexampleext.{so,dylib}` + `init.scm` into
//! `~/.config/wasdf/extensions/exampleext/`.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Report the ABI version this extension was built against.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_abi_version() -> u32 {
    wasdf_sdk::API_VERSION
}

thread_local! {
    /// The path most recently under the cursor (recorded from the cursor-changed
    /// event), so `show-info` knows which entry to describe.
    static CURSOR: RefCell<String> = const { RefCell::new(String::new()) };
    /// Whether the info panel is currently shown — the toggle state `o` flips.
    static SHOWING: RefCell<bool> = const { RefCell::new(false) };
    /// Keeps the last reply string alive until the next call (the kernel copies
    /// it immediately).
    static RESULT: RefCell<CString> = RefCell::new(CString::new("()").unwrap());
}

fn c_str(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn reply(s: String) -> *const c_char {
    RESULT.with(|r| {
        *r.borrow_mut() = CString::new(s).unwrap_or_default();
        r.borrow().as_ptr()
    })
}

/// Escape a string for embedding inside a Scheme string literal.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Record the cursor path. While the feature is ON the info panel follows the
/// cursor (push refreshed content); while OFF the panel is left untouched — this
/// is what makes `o` a real feature toggle, not just a panel show/hide.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_on_cursor_changed(path: *const c_char) -> *const c_char {
    let path = c_str(path);
    CURSOR.with(|c| *c.borrow_mut() = path);
    if SHOWING.with(|s| *s.borrow()) {
        reply(format!("({})", show_info()))
    } else {
        reply("()".to_string())
    }
}

/// Toggle the cursor-info feature. Every press notifies the configured greeting
/// with a `: ON` / `: OFF` suffix the extension appends from the new state. On
/// the ON press it pushes an overlay mode and shows the cursor entry's info
/// (which then follows the cursor via `wasdf_on_cursor_changed`); on the OFF
/// press it pops that mode (`pop-mode`) and the cleared flag stops the
/// cursor-follow, so the previewer reclaims the panel — the state before ON.
/// `data` is this extension's option snapshot (a `(("key" "value") …)` alist)
/// handed over the ABI.
fn toggle(data: &str) -> String {
    let settings = wasdf_sdk::Settings::from_scheme_alist(data);
    let greeting = match settings.get_str("exampleext.greeting") {
        s if s.is_empty() => "hello".to_string(),
        s => s,
    };
    let now_showing = SHOWING.with(|s| {
        let next = !*s.borrow();
        *s.borrow_mut() = next;
        next
    });
    // The extension decides the suffix from its own ON/OFF state.
    let message = format!("{greeting}: {}", if now_showing { "ON" } else { "OFF" });
    let notify = format!("(notify \"{}\")", esc(&message));
    if now_showing {
        // Take over the panel by pushing an overlay mode (keys fall through to the
        // file list underneath), then show the info document.
        format!("({notify} (push-overlay \"exampleext\" \"info\") {})", show_info())
    } else {
        // Pop the overlay mode; core clears our content and the previewer reclaims
        // the panel on the mode-change broadcast.
        format!("({notify} pop-mode)")
    }
}

/// Build the cursor entry's info as a single `(show-function-content …)` form
/// carrying a logical `document` — the extension emits key/value fields and lets
/// core lay them out (label alignment, theming, scroll); it no longer assembles
/// styled lines itself.
fn show_info() -> String {
    let path = CURSOR.with(|c| c.borrow().clone());
    if path.is_empty() {
        return document(&[muted_line("(no entry under the cursor)")]);
    }
    let p = std::path::Path::new(&path);
    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
    let mut blocks = vec![field("Name", &name, false), field("Path", &path, true)];
    match std::fs::symlink_metadata(p) {
        Ok(md) => {
            let ft = md.file_type();
            let kind = if ft.is_symlink() {
                match std::fs::read_link(p) {
                    Ok(t) => format!("symlink → {}", t.display()),
                    Err(_) => "symlink".to_string(),
                }
            } else if ft.is_dir() {
                "directory".to_string()
            } else {
                match p.extension().and_then(|s| s.to_str()) {
                    Some(ext) => format!(".{ext} file"),
                    None => "file".to_string(),
                }
            };
            blocks.push(field("Type", &kind, false));
            blocks.push(field("Size", &format!("{} ({} B)", wasdf_sdk::format_size(md.len()), md.len()), false));
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                blocks.push(field("Perm", &wasdf_sdk::format_permissions(md.mode()), true));
            }
        }
        Err(e) => blocks.push(muted_line(&format!("(cannot stat: {e})"))),
    }
    document(&blocks)
}

/// Wrap logical blocks in a single `(show-function-content "exampleext" (document …))`.
fn document(blocks: &[String]) -> String {
    format!("(show-function-content \"exampleext\" (document {}))", blocks.join(" "))
}

/// A `(field "label" ((span "value" [muted])))` block; `muted` themes the value.
fn field(label: &str, value: &str, muted: bool) -> String {
    let style = if muted { " (role \"muted\")" } else { "" };
    format!("(field \"{}\" ((span \"{}\"{style})))", esc(label), esc(value))
}

/// A `(line …)` block of a single muted span (for notices like the empty case).
fn muted_line(text: &str) -> String {
    format!("(line ((span \"{}\" (role \"muted\"))))", esc(text))
}

/// Handle an extension intent: `show-info` toggles the cursor entry's info.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_handle_intent(intent: *const c_char, data: *const c_char) -> *const c_char {
    let response = match c_str(intent).as_str() {
        "show-info" => toggle(&c_str(data)),
        _ => "()".to_string(),
    };
    reply(response)
}

/// This extension's VFS mount, `@/exampleext`: a single static leaf whose
/// activation re-dispatches `(ext "exampleext" "show-info")`, the same intent
/// the `o` keybinding sends — the mount is an ordinary source of activations,
/// not a separate mechanism. Ignores `path` since there is only the one level.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_vfs_list(_path: *const c_char) -> *const c_char {
    reply("((leaf \"About\" \"about\" (ext \"exampleext\" \"show-info\")))".to_string())
}
