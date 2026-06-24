//! The external-command executor: the Background and Captured execution forms
//! that run a resolved argv as a child process. The resolver (`services`) builds
//! the command line; this module runs it. The third form, Suspended, lives in the
//! terminal layer because it must own the foreground terminal.

pub mod executor;
pub mod options;

pub use executor::{run_background, run_captured};
pub use options::extract_options;
