//! The mode stack and the Select specification. Spec data here is immutable per
//! mode instance; runtime Select state lives in AppState and is mutated only by
//! the reducer.

use std::path::PathBuf;

use crate::core::event::ResolverRequest;
use crate::core::extension_value::ExtensionValue;
use crate::core::intent::{ExtensionIntent, Intent};

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
    Extension {
        extension: String,
        mode: String,
        state: ExtensionValue,
    },
}

impl Mode {
    /// The colon-separated mode id used for keymap and handler prefix matching.
    pub fn id(&self) -> String {
        match self {
            Mode::File => "file".into(),
            Mode::Select(spec) => format!("select:{}", spec.id),
            Mode::Policy(_) => "policy".into(),
            Mode::Extension { extension, mode, .. } => format!("{extension}:{mode}"),
        }
    }
}

/// Where a Select instance draws its candidates from.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectSource {
    FileWalk,
    Commands,
    PathCompletion,
    Static(Vec<Candidate>),
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
    /// Re-inject this extension intent template with the confirm shape inserted
    /// under the reserved `item` key.
    Emit(ExtensionIntent),
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
        }
    }

    /// An extension path/text picker confirmed through Emit (covers extension
    /// path and text entry; extensions use Emit, not the core Resolve flow).
    pub fn emit_path_input(
        id: impl Into<String>,
        template: ExtensionIntent,
        initial: Option<String>,
        hint: Option<FunctionHint>,
    ) -> Self {
        SelectSpec {
            id: id.into(),
            source: SelectSource::PathCompletion,
            input: SelectInput::Path,
            on_confirm: OnConfirm::Emit(template),
            initial_query: initial,
            function_hint: hint,
            command_line: Vec::new(),
        }
    }

    /// An extension Static picker confirmed through Emit.
    pub fn emit_static(
        id: impl Into<String>,
        candidates: Vec<Candidate>,
        template: ExtensionIntent,
        hint: Option<FunctionHint>,
    ) -> Self {
        SelectSpec {
            id: id.into(),
            source: SelectSource::Static(candidates),
            input: SelectInput::Fuzzy,
            on_confirm: OnConfirm::Emit(template),
            initial_query: None,
            function_hint: hint,
            command_line: Vec::new(),
        }
    }
}

/// The result of confirming a Select. Exactly three shapes; whether a value
/// came from a candidate or free input is never distinguished by type.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmShape {
    Single(Candidate),
    Many(Vec<Candidate>),
    InputOnly(String),
}

impl ConfirmShape {
    /// Lower the shape into the ExtensionValue stored under the reserved item
    /// key for Emit.
    pub fn to_value(&self) -> ExtensionValue {
        match self {
            ConfirmShape::Single(c) => c.value.clone(),
            ConfirmShape::Many(items) => {
                ExtensionValue::List(items.iter().map(|c| c.value.clone()).collect())
            }
            ConfirmShape::InputOnly(s) => ExtensionValue::String(s.clone()),
        }
    }
}
