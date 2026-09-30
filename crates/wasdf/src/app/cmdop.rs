//! CmdOp, the built-in command builder (doc/CMDOP.md). Part of the kernel —
//! always registered, never disabled — though it is written on the same form
//! seam extensions use.

mod builder;

pub use builder::CmdOp;
