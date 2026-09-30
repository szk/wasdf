//! Core: AppState, Intent, the reducer, Mode/SelectSpec, ExtensionValue, and
//! the async round-trip types (AppEvent, Plan, AsyncResult). The reducer is
//! pure and synchronous; all I/O lives in kernel async tasks.

pub mod event;
pub mod extension_value;
pub mod intent;
pub mod mode;
pub mod option;
pub mod reducer;
pub mod scroll;
pub mod state;
pub mod theme;

pub use event::{
    AppEvent, AsyncResult, AsyncStatus, Block, Emph, ExecOutput, OpDone, PanelView, Payload, Plan,
    Purpose, Content, ResolverRequest, SpanStyle, TextSpan, LINE_NUMBER_WIDTH,
};
pub use extension_value::{ExtensionValue, KEY_ITEM, KEY_RESOLVER};
pub use intent::{ExtensionIntent, Intent, Key, KeyCode, KeyKind, Mods};
pub use mode::{
    Candidate, CmdToken, ConfirmShape, FunctionHint, InputFormSpec, InputKind, Mode, OnConfirm,
    ResolveFill, SelectInput, SelectSource, SelectSpec, SubLayout,
};
pub use option::{ConfigWrite, OptionDef, OptionKind, OptionStore, OptionValue};
pub use scroll::Scroll;
pub use theme::{color_to_rgb, color_token, parse_color, Role, Theme};
pub use reducer::{CommandLookup, Effects, Notice, apply_intent, apply_result};
pub use state::{
    AppState, Entry, EntryList, FunctionInput, InputCapture, InputFormState, ListGeom, ListLayout,
    PanelSearch, SelectPhase, PANEL_FILE, PANEL_FUNCTION,
};
