//! wasdf-sdk: ABI-stable types and helpers shared with dynamically loaded
//! extensions. Chunked reading, formatting helpers, file signature detection,
//! and MIME detection live here so both the kernel and optional extensions
//! agree on representation.

pub mod chunk;
pub mod format;
pub mod magic;
pub mod mime;

/// The ABI version the kernel and optional extensions must agree on exactly.
/// A mismatch causes the extension to be skipped with a warning.
///
/// v2: the glue grammar gained an optional `(keymaps …)` section, letting an
/// extension bind keys (to extension intents) at the Extension layer.
/// v3: the optional `wasdf_on_cursor_changed(path) -> *const c_char` entry point —
/// extensions subscribe to the cursor-changed event and reply with follow-up
/// intents (e.g. `show-function-content`), same datum form as `wasdf_handle_intent`.
/// v4: the glue moved out of the library into a sibling `.scm` manifest that names
/// the library via `(lib …)`; `wasdf_glue` is gone. The library now exports only
/// `wasdf_abi_version` plus the optional `wasdf_handle_intent` /
/// `wasdf_on_cursor_changed` behavior the manifest's intents route to.
pub const API_VERSION: u32 = 0;

pub use chunk::{read_chunk, read_chunk_at};
pub use format::{format_permissions, format_size};
pub use magic::{Signature, detect_signature};
pub use mime::{MimeClass, detect_mime};
