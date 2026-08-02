//! Archive extension facade. The implementation lives under `archive/`; this file
//! is the thin module facade (`<dirname>.rs`, per CLAUDE.md) re-exporting the
//! public surface.

mod extension;

pub use extension::ArchiveExtension;
