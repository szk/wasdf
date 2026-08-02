//! LS Colors extension facade. The implementation lives under `lscolors/`;
//! this file is the thin module facade (`<dirname>.rs`, per CLAUDE.md)
//! re-exporting the public surface.

mod extension;
mod parse;
mod registry;
mod writer;

pub use extension::LsColorsExtension;
pub use registry::NamedPalette;

/// The raw preset scheme source (include_str! at compile time).
pub const PRESETS_SCHEME: &str = include_str!("lscolors/presets.scm");
