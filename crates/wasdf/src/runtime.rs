//! The runtime: TaskManager owns the kernel's worker threads and the async
//! round trip back to the event loop.

pub mod task;

pub use task::TaskManager;
