//! Core: AppState, Intent, the reducer, Mode/SelectSpec, ExtensionValue, and
//! the async round-trip types (AppEvent, Plan, AsyncResult). The reducer is
//! pure and synchronous; all I/O lives in kernel async tasks.

pub mod event;
pub mod extension_value;
pub mod intent;
pub mod mode;
pub mod reducer;
pub mod state;

pub use event::{
    AppEvent, AsyncResult, AsyncStatus, ExecOutput, OpDone, PanelContent, Payload, Plan, Purpose,
    ReadResult, ResolverRequest, StyleRun,
};
pub use extension_value::{ExtensionValue, KEY_ITEM, KEY_RESOLVER};
pub use intent::{ExtensionIntent, Intent, Key, KeyCode, Mods};
pub use mode::{
    Candidate, CmdToken, ConfirmShape, FunctionHint, Mode, OnConfirm, ResolveFill, SelectInput,
    SelectSpec, SubLayout,
};
pub use reducer::{CommandLookup, Effects, Notice, apply_intent, apply_result};
pub use state::{AppState, Entry, EntryList, ListGeom, ListLayout, PanelSearch, SelectPhase};
