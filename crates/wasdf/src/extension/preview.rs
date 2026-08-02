//! Preview extension facade. The implementation lives under `preview/`; this file
//! is the thin module facade (`<dirname>.rs`, per CLAUDE.md): it declares the
//! detail submodules and re-exports the public surface.

pub mod decode;
pub mod highlight;
pub mod markdown;
pub mod search;

mod extension;

pub use extension::PreviewExtension;
