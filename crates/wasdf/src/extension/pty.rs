//! The `pty` extension: pseudo-terminal sessions rendered inside
//! `PANEL_FUNCTION`. See doc/PTY.md for the canon.

pub mod encode;
pub mod extension;
pub mod graphics;
pub mod kbmode;
pub mod provider;
pub mod session;
pub mod sessions;

pub use extension::PtyExtension;
