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
/// This is the initial prototype ABI, version 0. Its surface: the glue
/// declarations live in a sibling `.scm` manifest that names the native library
/// via `(lib …)` and may carry a `(keymaps …)` section (Extension-layer key
/// bindings); the library exports `wasdf_abi_version` plus the optional
/// `wasdf_handle_intent` and `wasdf_on_cursor_changed` entry points, each
/// returning follow-up intents as a Scheme datum (e.g. `show-function-content`).
pub const API_VERSION: u32 = 0;

pub use chunk::{read_chunk, read_chunk_at};
pub use format::{format_permissions, format_size};
pub use magic::{Signature, detect_signature};
pub use mime::{MimeClass, detect_mime};
