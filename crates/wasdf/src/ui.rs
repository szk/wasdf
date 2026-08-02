//! The UI layer: UiManager (render-only caches) and the panel render functions.
//! Rust renders; layouts and panel placement would be Scheme-declared in the
//! full design. Renderer selection is native Rust dispatch.

pub mod bottom;
pub mod chrome;
pub mod content;
pub mod manager;
pub mod middle;
pub mod render;
pub mod top;

pub use manager::UiManager;
