//! Bookmarks extension facade. The implementation lives under `bookmarks/`; this
//! file is the thin module facade (`<dirname>.rs`, per CLAUDE.md) re-exporting
//! the public surface.

mod extension;
mod provider;

pub use extension::BookmarksExtension;
