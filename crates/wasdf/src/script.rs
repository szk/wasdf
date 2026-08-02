//! The script layer. An embedded Scheme session (steel-core, dedicated thread)
//! parses configuration, glues extensions, and evaluates dynamic conditions.
//! The codec is the single owner of key/intent/mode/datum string forms.

pub mod codec;
pub mod condition;
pub mod config;
pub mod keymap;
pub mod session;
pub mod sexpr;

pub use session::SchemeSession;
