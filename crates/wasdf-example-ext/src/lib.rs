//! An example optional (dynamically loaded) wasdf extension — the **native
//! behavior** half. Its declarations (id, commands, keymaps, resolvers) live in
//! the sibling `example.scm` manifest, which the kernel discovers and reads; this
//! library only exports the C ABI behavior the manifest's intents route to: an
//! API-version query, an intent handler, and (ABI v3+) a cursor-changed
//! subscriber that pushes content into the function panel as the cursor moves.
//!
//! Build it as a dynamic library and drop it — together with `example.scm` —
//! into the extensions directory (`$WASDF_EXTENSIONS_DIR` or
//! `~/.config/wasdf/extensions`). The manifest names this library via `(lib …)`.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Report the ABI version this extension was built against.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_abi_version() -> u32 {
    wasdf_sdk::API_VERSION
}

thread_local! {
    /// Holds the most recent result so the returned pointer stays valid until
    /// the next call (the kernel copies it immediately).
    static RESULT: RefCell<CString> = RefCell::new(CString::new("()").unwrap());
    /// The extension's own interactive state, advanced by the `step` intent.
    static STEP: RefCell<u32> = const { RefCell::new(0) };
}

/// Handle an extension intent. Receives the intent id and its payload (as a
/// Scheme datum), and returns follow-up intents as a Scheme list. `greet`
/// renders styled text into the function panel via `show-function-content`
/// (the first line carries a colored 5-byte run, "GREET"); the kernel stores and
/// draws it generically with no kernel edits.
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_handle_intent(intent: *const c_char, _data: *const c_char) -> *const c_char {
    let name = if intent.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(intent) }.to_str().unwrap_or("")
    };
    let response: String = match name {
        // The GREET run carries both fg and a bg (the 7-element run form), so the
        // extension owns its colors fully across the ABI.
        "greet" => "((show-function-content \"example\" (lines \
            (\"GREET\" (5 80 250 123 40 42 54)) \
            (\"Rendered by wasdf-example-ext in the function panel.\") \
            (\"This content crossed the C ABI as Scheme data.\") \
            (\"Scroll with j / k. Search with /. Press t to step.\"))))"
            .to_string(),
        // Interactive: advance the extension's own state and reflect it both in
        // the kernel view state (update-function-view) and the rendered content.
        "step" => {
            let n = STEP.with(|c| {
                *c.borrow_mut() += 1;
                *c.borrow()
            });
            format!(
                "((update-function-view {n}) \
                 (show-function-content \"example\" (lines \
                    (\"step: {n}\") \
                    (\"Press t again to advance; the view state is stored in the kernel.\"))))"
            )
        }
        _ => "()".to_string(),
    };
    RESULT.with(|r| {
        *r.borrow_mut() = CString::new(response).unwrap_or_default();
        r.borrow().as_ptr()
    })
}

thread_local! {
    /// Holds the cursor-changed reply pointer valid until the next call.
    static CURSOR_RESULT: RefCell<CString> = RefCell::new(CString::new("()").unwrap());
}

/// React to the kernel's **cursor-changed** event (ABI v3). The kernel passes the
/// path now under the cursor; we reply with `show-function-content` echoing it, to
/// demonstrate that a dynamically-loaded extension can subscribe to cursor
/// movement with no kernel edits. (While this extension is loaded it takes over
/// the function panel on every cursor move — that is the point of the demo.)
#[unsafe(no_mangle)]
pub extern "C" fn wasdf_on_cursor_changed(path: *const c_char) -> *const c_char {
    let path = if path.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned()
    };
    // Escape embedded double-quotes so the reply stays a valid Scheme string.
    let safe = path.replace('\\', "\\\\").replace('"', "\\\"");
    let response = format!(
        "((show-function-content \"example\" (lines \
            (\"cursor-changed →\" (13 80 250 123)) \
            (\"{safe}\") \
            (\"This line was pushed by wasdf-example-ext reacting to the\") \
            (\"cursor-changed event over the C ABI (no kernel edits).\"))))"
    );
    CURSOR_RESULT.with(|r| {
        *r.borrow_mut() = CString::new(response).unwrap_or_default();
        r.borrow().as_ptr()
    })
}
