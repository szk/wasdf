//! The app layer: the kernel (dispatch_intent and state), the event loop, the
//! terminal boundary, and the Suspended execution form.

pub mod event_loop;
pub mod kernel;
pub mod terminal;

pub use event_loop::run;
