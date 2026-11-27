//! The mode stack and the Select specification. Spec data here is immutable per
//! mode instance; runtime Select state lives in AppState and is mutated only by
//! the reducer.

use std::path::PathBuf;

use crate::core::event::ResolverRequest;
use crate::core::extension_value::ExtensionValue;
use crate::core::intent::{ExtensionIntent, Intent};
use crate::core::state::Entry;

/// The active display frame of the function panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubLayout {
    Content,
    Exec,
}

/// A mode on the stack. Each carries its defining spec; behavior-affecting
/// runtime state lives in AppState.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    File,
    Select(SelectSpec),
    Policy(Box<Intent>),
    /// A small popup text-entry dialog for one value (free text or a choice
    /// among candidates). Shares its chrome with `Policy`'s Confirm box (see
    /// `ui/render.rs`); Enter confirms (emits `spec.template` with the typed
    /// text under `KEY_ITEM`), Esc cancels. Runtime text/cursor state lives in
    /// `AppState::input_form`, mirroring how `SelectState` sits beside `Select`.
    InputForm(InputFormSpec),
    Extension {
        extension: String,
        mode: String,
        state: ExtensionValue,
        /// An **overlay** mode (`true`) sits on top of the mode below but lets
        /// unhandled keys fall through to it (e.g. an info panel that adds `o`
        /// while `wasd` keeps navigating the file list). A non-overlay extension
        /// mode is **modal** — it captures all keys like Select / Policy.
        overlay: bool,
    },
}

impl Mode {
    /// The colon-separated mode id used for keymap and handler prefix matching.
    pub fn id(&self) -> String {
        match self {
            Mode::File => "file".into(),
            Mode::Select(spec) => format!("select:{}", spec.id),
            Mode::Policy(_) => "policy".into(),
            // Fixed — every instance behaves identically (Enter/Esc/typing), so
            // there's no per-instance keymap scope to distinguish.
            Mode::InputForm(_) => "input-form".into(),
            Mode::Extension { extension, mode, .. } => format!("{extension}:{mode}"),
        }
    }

    /// Whether this mode **captures** key input (a modal takeover) and so stops
    /// the keymap's mode-stack walk, versus an **overlay** that falls through to
    /// the mode beneath it on any key it does not itself bind. File / Select /
    /// Policy / InputForm capture; an extension mode captures unless it
    /// declared itself an overlay.
    pub fn captures_keys(&self) -> bool {
        match self {
            Mode::File | Mode::Select(_) | Mode::Policy(_) | Mode::InputForm(_) => true,
            Mode::Extension { overlay, .. } => !overlay,
        }
    }
}

/// Which widget a `Mode::InputForm` popup renders and how it edits its value.
/// The wire encoding of the value (what `spec.initial` holds and what gets
/// inserted under `KEY_ITEM` on confirm) is chosen per kind to match what the
/// existing consumers already read — see doc/UI.md's Input Form Mode section.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// Free text, edited via `tui-input`. `spec.initial`/the confirmed value
    /// are the raw string.
    Text,
    /// A List with two rows, "On"/"Off". Wire value: `"#t"` / `"#f"`.
    Bool,
    /// A List, one row per option. Wire value: the chosen option string.
    Choice(Vec<String>),
    /// A Gauge showing the value's position in `min..max`. Wire value: the
    /// decimal string.
    Int { min: i64, max: i64 },
    /// A Table, one row per item, free multi-value. Wire value: comma-joined.
    List,
}

/// The spec for a `Mode::InputForm` popup: immutable per instance (the typed
/// value/cursor/selection are runtime state in `AppState::input_form`).
#[derive(Debug, Clone, PartialEq)]
pub struct InputFormSpec {
    /// Shown as the popup box's border title (may include a value hint, e.g.
    /// `"coloring (none | LS_COLORS | Light | Dark | Custom)"`).
    pub title: String,
    /// Prefilled value, encoded per `kind` (see [`InputKind`]).
    pub initial: String,
    pub kind: InputKind,
    /// Emitted on confirm with the encoded value inserted under `KEY_ITEM` —
    /// the same contract as `Select`'s `OnConfirm::Emit`, so one `*-apply`
    /// handler serves both. Nothing is emitted before
    /// confirm; there is no live-apply.
    pub template: ExtensionIntent,
    /// What this form's input is for; see [`SelectSpec::input_hint`].
    pub input_hint: Option<String>,
}

impl InputFormSpec {
    /// The free-text popup (theme new/duplicate/rename, archive pack).
    pub fn emit_input_form(
        title: impl Into<String>,
        template: ExtensionIntent,
        initial: Option<String>,
    ) -> Self {
        InputFormSpec {
            title: title.into(),
            initial: initial.unwrap_or_default(),
            kind: InputKind::Text,
            template,
            input_hint: Some("type value".into()),
        }
    }

    /// A typed-widget popup (Bool/Choice/Int/List/Color).
    pub fn emit_input_widget(
        title: impl Into<String>,
        template: ExtensionIntent,
        kind: InputKind,
        initial: impl Into<String>,
    ) -> Self {
        InputFormSpec {
            title: title.into(),
            initial: initial.into(),
            kind,
            template,
            input_hint: Some("type value".into()),
        }
    }
}

/// Where a Select instance draws its candidates from.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectSource {
    /// Deep search: a recursive walk from `AppState.cwd`, ranked async via
    /// `Plan::Search`. See doc/UI.md (Deep search vs. shallow filter).
    FileWalk,
    Commands,
    Static(Vec<Candidate>),
    /// Shallow filter: a fixed, already-in-hand `Entry` pool (the file
    /// panel's current listing, or a frozen deep-search result view) ranked
    /// **synchronously** in the reducer, no I/O, no plan. Works on virtual
    /// paths, unlike `FileWalk`.
    Entries(Vec<Entry>),
}

/// The input field behavior of a Select instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectInput {
    Fuzzy,
    Path,
    None,
}

/// Which field of a resolver request the confirmed Select **input** fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveFill {
    /// The confirmed input text → `dst` (copy/move/rename destination).
    Dst,
    /// The confirmed input text → `path` (mkdir/touch name).
    Path,
}

/// What confirming a Select instance does. A fixed set, never extended.
#[derive(Debug, Clone, PartialEq)]
pub enum OnConfirm {
    Navigate,
    RunCommand,
    /// The generic core "command op": fill `fill` of `template` from the typed
    /// input and `opts` from the marked option candidates, then run
    /// `RunResolver(template)`. One Select carries both the destination (the input)
    /// and the option checkboxes (Static candidates + Space marks).
    Resolve {
        template: ResolverRequest,
        fill: ResolveFill,
    },
}

/// A renderer hint for the function panel during Select.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionHint {
    DirListing,
    CommandSummary,
    Extension(String),
}

/// One token of the resolved command line shown in the Command panel: the
/// literal executable and flags (`cp`, `-R`) interleaved with placeholders that
/// the live Select values fill. The kernel fills this from the resolver chain at
/// push time, so the panel shows the **actual external command**, not the op key.
#[derive(Debug, Clone, PartialEq)]
pub enum CmdToken {
    Lit(String),
    Opts,
    Src,
    Paths,
    Dst,
    Path,
}

/// A single pickable candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub label: String,
    pub value: ExtensionValue,
}

impl Candidate {
    pub fn new(label: impl Into<String>, value: ExtensionValue) -> Self {
        Candidate { label: label.into(), value }
    }

    /// A candidate whose value is a filesystem path; label is the file name.
    pub fn path(p: PathBuf) -> Self {
        let label = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned());
        Candidate { label, value: ExtensionValue::Path(p) }
    }
}

/// The full specification of a Select instance. Immutable on the mode stack.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectSpec {
    pub id: String,
    pub source: SelectSource,
    pub input: SelectInput,
    pub on_confirm: OnConfirm,
    pub initial_query: Option<String>,
    pub function_hint: Option<FunctionHint>,
    /// The resolved command-line skeleton for the Command panel; empty for every
    /// non-command Select. Filled from the resolver chain at push time.
    pub command_line: Vec<CmdToken>,
    /// What this Select's input is *for*, shown keyless at the head of the help
    /// row ("type to filter", "type argument"). It describes the mode, not a
    /// key, so the mode that opens the input owns it (doc/HELP.md).
    pub input_hint: Option<String>,
}

impl SelectSpec {
    /// The file-search picker (f): recursive walk, fuzzy, navigate on confirm.
    pub fn file_search() -> Self {
        SelectSpec {
            id: "file-search".into(),
            source: SelectSource::FileWalk,
            input: SelectInput::Fuzzy,
            on_confirm: OnConfirm::Navigate,
            initial_query: None,
            function_hint: Some(FunctionHint::DirListing),
            command_line: Vec::new(),
            input_hint: Some("type to filter".into()),
        }
    }

    /// The shallow-filter picker (`f` on a virtual dir, or the file-search
    /// result view's own `f`/refine): rank a fixed `Entry` pool synchronously,
    /// navigate on confirm. Shares `commit_file_search`'s commit path with
    /// `file_search`.
    pub fn filter_list(entries: Vec<Entry>) -> Self {
        SelectSpec {
            id: "filter-list".into(),
            source: SelectSource::Entries(entries),
            input: SelectInput::Fuzzy,
            on_confirm: OnConfirm::Navigate,
            initial_query: None,
            function_hint: Some(FunctionHint::DirListing),
            command_line: Vec::new(),
            input_hint: Some("type to filter".into()),
        }
    }

    /// The key list (`?`): every binding in force where `?` was pressed, one
    /// row each — a cheat sheet to scroll and close (its rows run nothing).
    pub fn key_list(candidates: Vec<Candidate>) -> Self {
        SelectSpec {
            id: "key-list".into(),
            source: SelectSource::Static(candidates),
            input: SelectInput::None,
            on_confirm: OnConfirm::RunCommand,
            initial_query: None,
            function_hint: None,
            command_line: Vec::new(),
            input_hint: None,
        }
    }

    /// The command palette (x): static candidates, fuzzy, run the command.
    pub fn command_palette(candidates: Vec<Candidate>) -> Self {
        SelectSpec {
            id: "command-palette".into(),
            source: SelectSource::Static(candidates),
            input: SelectInput::Fuzzy,
            on_confirm: OnConfirm::RunCommand,
            initial_query: None,
            function_hint: None,
            command_line: Vec::new(),
            input_hint: Some("type to filter".into()),
        }
    }

    /// The single-screen command picker (c, m, R, mkdir, touch): the input fills
    /// `fill` (dst/name); the Static `options` are the option checkboxes (toggled
    /// with Space); the function panel shows the live command line. Confirm runs
    /// the completed command. `options` is empty for ops that declare none.
    pub fn command(
        template: ResolverRequest,
        fill: ResolveFill,
        options: Vec<Candidate>,
        initial: Option<String>,
    ) -> Self {
        SelectSpec {
            id: "command".into(),
            source: SelectSource::Static(options),
            input: SelectInput::Path,
            on_confirm: OnConfirm::Resolve { template, fill },
            initial_query: initial,
            function_hint: Some(FunctionHint::CommandSummary),
            // Filled at push time by the reducer from the resolver chain.
            command_line: Vec::new(),
            input_hint: Some("type argument".into()),
        }
    }
}

/// The result of confirming a Select. Whether a value came from a candidate
/// or free input is never distinguished by type.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmShape {
    Single(Candidate),
    InputOnly(String),
}
