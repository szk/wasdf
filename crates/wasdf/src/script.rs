//! The script layer. In the full design, an embedded R7RS Scheme session (stak)
//! parses configuration, glues extensions, and declares layouts. This MVP
//! provides the same registries and the native `:when` condition evaluator and
//! codec, with the embedded defaults declared natively in place of the resident
//! REPL. The codec remains the single owner of key/intent/mode string forms.

pub mod codec;
pub mod condition;
pub mod config;
pub mod keymap;
pub mod session;
pub mod sexpr;

pub use session::SchemeSession;
