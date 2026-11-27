//! The command builder, hosted in the function panel as an **interactive
//! form** — the reference form, next to preview's reference content. It owns
//! its state in interior mutability, handles its keys via `handle_intent`,
//! draws itself via `render_form`, and reads its browse directory through the
//! generic `LoadContent` → `accept_content` path, so core / ui see only the
//! generic seams (`form_owner`, `FunctionInput`, `render_form`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use crate::core::event::ResolverRequest;
use crate::core::scroll::Scroll;
use crate::core::{
    AppState, CmdToken, Entry, ExtensionIntent, ExtensionValue, FunctionInput, Intent, Content,
    Role, PANEL_FUNCTION,
};
use crate::extension::{Extension, FunctionRenderCtx};
use crate::services::resolver::{ArgEl, ResolverChain, Slot};

const ID: &str = "cmdop";
/// Rows the CLI-builder preview occupies at the top of the panel.
const PREVIEW_H: u16 = 5;
/// `"[x] "` — the pinned option-checkbox prefix width.
const OPT_PREFIX_W: usize = 4;

/// Which command the builder is assembling. `Create` (mkdir/touch, combined —
/// see `run_create`) has no resolver op of its own: it dispatches one
/// `mkdir`/`touch` request per typed line, so `key()` is never read for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Copy,
    Move,
    Rename,
    Create,
}

impl Op {
    fn key(self) -> &'static str {
        match self {
            Op::Copy => "copy",
            Op::Move => "move",
            Op::Rename => "rename",
            Op::Create => "create",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Op::Copy => "Copy",
            Op::Move => "Move",
            Op::Rename => "Rename",
            Op::Create => "Create",
        }
    }
    /// Copy/move/rename only — `start-create` takes the separate `start_create`
    /// path (no sources requirement, no destination browser).
    fn from_intent(id: &str) -> Option<Op> {
        match id {
            "start-copy" => Some(Op::Copy),
            "start-move" => Some(Op::Move),
            "start-rename" => Some(Op::Rename),
            _ => None,
        }
    }
}

/// A toggleable option row (e.g. `-r` / "Recursive").
#[derive(Debug, Clone)]
struct OptRow {
    token: String,
    label: String,
    checked: bool,
}

/// The live builder form (interior-mutable inside the extension).
struct Form {
    op: Op,
    command_line: Vec<CmdToken>,
    options: Vec<OptRow>,
    entries: Vec<Entry>,
    list_cwd: PathBuf,
    /// Unified cursor: `0..options.len()` = option rows; the rest = listing rows.
    cursor: usize,
    on_builder: bool,
    /// Self-clamped scrolls (no kernel round-trip — bound each render).
    opt_hscroll: Scroll,
    builder_scroll: Scroll,
    /// Persistent follow-cursor offsets for the two body sections.
    opt_off: usize,
    list_off: usize,
    /// The function-panel listing's own marks — independent of `AppState.selection`.
    list_marks: HashSet<PathBuf>,
    /// The persisted filter query (post-submit), or `None` for no filter.
    filter_query: Option<String>,
    /// `/` two-stage deep-then-shallow search (doc/CMDOP.md): `false`
    /// until a deep search over `list_cwd` has landed, at which point `/`
    /// switches from issuing another deep search to `filter_query`'s live
    /// shallow narrowing of the results already in hand. Reset whenever
    /// `list_cwd` changes (parent/descend) — deep results describe one
    /// directory, not the browser session.
    deep_results: bool,
    /// Set when a deep search (`Intent::LoadSearch`) has been issued but its
    /// completion hasn't arrived yet, so `accept_content` can tell a deep
    /// result apart from an ordinary directory (re)listing for the same
    /// path. Cleared on the next accepted `Dir` for `list_cwd`, or
    /// immediately on a dir change (the stale completion, if it ever
    /// arrives, is ignored by the existing `list_cwd == path` self-gate).
    awaiting_deep: bool,
    /// The last submitted deep-search query, kept only to show in the panel
    /// title while `awaiting_deep` or `deep_results` holds. Cleared with them.
    deep_query: Option<String>,
}

impl Form {
    /// A fresh form browsing `list_cwd`, the cursor on the first listing row.
    fn new(op: Op, command_line: Vec<CmdToken>, options: Vec<OptRow>, list_cwd: PathBuf) -> Self {
        Form {
            op,
            command_line,
            cursor: options.len(),
            options,
            entries: Vec::new(),
            list_cwd,
            on_builder: false,
            opt_hscroll: Scroll::default(),
            builder_scroll: Scroll::default(),
            opt_off: 0,
            list_off: 0,
            list_marks: HashSet::new(),
            filter_query: None,
            deep_results: false,
            awaiting_deep: false,
            deep_query: None,
        }
    }

    /// Browse `dir`: the listing empties until its read lands, and deep-search
    /// state is dropped — deep results describe one directory, not the whole
    /// builder session.
    fn browse(&mut self, dir: PathBuf) -> Intent {
        self.list_cwd = dir.clone();
        self.entries.clear();
        self.cursor = self.options.len();
        self.list_off = 0;
        self.deep_results = false;
        self.awaiting_deep = false;
        self.deep_query = None;
        Intent::LoadContent { owner: ID.into(), path: dir, offset: 0 }
    }

    /// The destination for copy/move (rename's comes from the prompt): a pinned
    /// mark, else the entry under the cursor, else the browsed directory.
    fn destination(&self) -> PathBuf {
        if !self.list_marks.is_empty() {
            if let Some(e) = self.entries.iter().find(|e| self.list_marks.contains(&e.path)) {
                return e.path.clone();
            }
            if let Some(p) = self.list_marks.iter().next() {
                return p.clone();
            }
        }
        if self.cursor >= self.options.len()
            && let Some(e) = self.entries.get(self.cursor - self.options.len()) {
                return e.path.clone();
            }
        self.list_cwd.clone()
    }

    /// The live command line: `<op> <checked-opts> <sources> <dest>`, from the
    /// resolved skeleton (or a bare fallback). `dest_text` overrides the
    /// destination while a prompt/builder input is open.
    fn preview(&self, sources: &[PathBuf], dest_text: Option<&str>) -> String {
        let dest = dest_text.map(String::from).unwrap_or_else(|| self.destination().to_string_lossy().into_owned());
        let opts: Vec<&str> = self.options.iter().filter(|o| o.checked).map(|o| o.token.as_str()).collect();
        let disp = |p: &PathBuf| p.display().to_string();
        let parts: Vec<String> = if self.command_line.is_empty() {
            let mut p: Vec<String> = opts.iter().map(|s| s.to_string()).collect();
            p.extend(sources.iter().map(disp));
            p.push(dest);
            p
        } else {
            self.command_line
                .iter()
                .flat_map(|tok| -> Vec<String> {
                    match tok {
                        CmdToken::Lit(s) => vec![s.clone()],
                        CmdToken::Opts => opts.iter().map(|s| s.to_string()).collect(),
                        CmdToken::Src => sources.iter().take(1).map(disp).collect(),
                        CmdToken::Paths => sources.iter().map(disp).collect(),
                        CmdToken::Dst | CmdToken::Path => vec![dest.clone()],
                    }
                })
                .collect()
        };
        parts.join(" ")
    }

    fn reset_hscroll_off_options(&mut self) {
        if self.cursor >= self.options.len() {
            self.opt_hscroll.go_top();
        }
    }
}

/// The bundled CmdOp form extension.
pub struct CmdOp {
    form: RefCell<Option<Form>>,
    resolver: OnceLock<Arc<ResolverChain>>,
    /// Memo of `--help`-parsed options, one entry per external command.
    opt_cache: RefCell<HashMap<String, Vec<(String, String)>>>,
    store: OnceLock<Arc<crate::core::OptionStore>>,
    /// Every registered extension id, filled by the kernel once all are
    /// registered: `cmdop.runner` is user data and may name one that is not.
    extension_ids: Arc<OnceLock<Vec<String>>>,
}

impl CmdOp {
    pub fn new(extension_ids: Arc<OnceLock<Vec<String>>>) -> Self {
        CmdOp {
            form: RefCell::new(None),
            resolver: OnceLock::new(),
            opt_cache: RefCell::new(HashMap::new()),
            store: OnceLock::new(),
            extension_ids,
        }
    }
}

impl CmdOp {
    /// The option rows for an op: declared options, or those parsed once from the
    /// resolved command's `--help` (memoized).
    fn options_for(&self, op: Op) -> Vec<OptRow> {
        let Some(resolver) = self.resolver.get() else { return Vec::new() };
        let key = op.key();
        let declared: Vec<(String, String)> =
            resolver.options(key).into_iter().map(|o| (o.token, o.label)).collect();
        let extracted = match resolver.command_of(key) {
            Some(cmd) => {
                if let Some(c) = self.opt_cache.borrow().get(&cmd).cloned() {
                    c
                } else {
                    let parsed = crate::exec::extract_options(&cmd);
                    self.opt_cache.borrow_mut().insert(cmd, parsed.clone());
                    parsed
                }
            }
            None => Vec::new(),
        };
        let src = if extracted.is_empty() { declared } else { extracted };
        src.into_iter().map(|(token, label)| OptRow { token, label, checked: false }).collect()
    }

    /// The resolver command skeleton for an op, as `CmdToken`s.
    fn command_for(&self, op: Op) -> Vec<CmdToken> {
        let Some(resolver) = self.resolver.get() else { return Vec::new() };
        resolver
            .command_template(op.key())
            .into_iter()
            .map(|el| match el {
                ArgEl::Lit(s) => CmdToken::Lit(s),
                ArgEl::Ph(Slot::Opts) => CmdToken::Opts,
                ArgEl::Ph(Slot::Src) => CmdToken::Src,
                ArgEl::Ph(Slot::Paths) => CmdToken::Paths,
                ArgEl::Ph(Slot::Dst) => CmdToken::Dst,
                ArgEl::Ph(Slot::Path) => CmdToken::Path,
            })
            .collect()
    }

    /// Open the builder for `op`.
    fn start(&self, op: Op, state: &AppState) -> Vec<Intent> {
        let sources = state.targets();
        if sources.is_empty() {
            return Vec::new(); // nothing to operate on
        }
        *self.form.borrow_mut() = Some(Form::new(op, self.command_for(op), self.options_for(op), state.cwd.clone()));
        let mut out = vec![
            Intent::SetFunctionForm(Some(ID.into())),
            Intent::LoadContent { owner: ID.into(), path: state.cwd.clone(), offset: 0 },
        ];
        // Rename edits a name via the prompt (function panel focused); copy/move
        // keep file focus and browse for the destination.
        if op == Op::Rename {
            let name = sources
                .first()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(Intent::FocusPanel(PANEL_FUNCTION.into()));
            out.push(Intent::BeginInput { purpose: "prompt".into(), seed: name, multiline: false });
        }
        out
    }

    /// Open the mkdir/touch builder (M): needs no target selection and no
    /// destination browser — every line is created directly in `state.cwd`.
    /// The multi-line name buffer lives in the generic `FunctionInput`
    /// (`multiline: true`), not in `Form`; `Form` here only carries `op` so
    /// `render_form` can tell Create apart from Copy/Move/Rename.
    fn start_create(&self, state: &AppState) -> Vec<Intent> {
        *self.form.borrow_mut() = Some(Form::new(Op::Create, Vec::new(), Vec::new(), state.cwd.clone()));
        vec![
            Intent::SetFunctionForm(Some(ID.into())),
            Intent::BeginInput { purpose: "create-lines".into(), seed: String::new(), multiline: true },
        ]
    }

    /// Drop the form and give the panel back.
    fn close(&self) -> Vec<Intent> {
        self.form.borrow_mut().take();
        vec![Intent::SetFunctionForm(None)]
    }

    /// Split the submitted multi-line buffer into `mkdir`/`touch` requests —
    /// one per non-blank line, decided by a trailing `/` (`DIR/` → `mkdir -p
    /// DIR`, else `FILE` → `touch FILE`) — then close the form. Blank lines
    /// (including the terminating one that triggered submit) are dropped.
    fn run_create(&self, text: &str) -> Vec<Intent> {
        self.form.borrow_mut().take();
        let mut out: Vec<Intent> = create_targets(text)
            .map(|(op, name)| {
                Intent::RunResolver(ResolverRequest {
                    op: op.into(),
                    src: None,
                    dst: None,
                    path: Some(name.to_string()),
                    paths: Vec::new(),
                    opts: Vec::new(),
                    label: op.into(),
                })
            })
            .collect();
        out.push(Intent::SetFunctionForm(None));
        out
    }

    /// Descend into the dir under the cursor (or, on an option row, h-scroll right;
    /// on a file, confirm with it).
    fn descend(&self, state: &AppState) -> Vec<Intent> {
        enum Act {
            None,
            Browse(Intent),
            Confirm,
        }
        let act = {
            let mut f = self.form.borrow_mut();
            let Some(form) = f.as_mut() else { return Vec::new() };
            if form.on_builder {
                Act::None
            } else if form.cursor < form.options.len() {
                form.opt_hscroll.advance(1);
                Act::None
            } else {
                let idx = form.cursor - form.options.len();
                match form.entries.get(idx) {
                    Some(e) if e.is_dir => {
                        let dir = e.path.clone();
                        Act::Browse(form.browse(dir))
                    }
                    Some(_) => Act::Confirm,
                    None => Act::None,
                }
            }
        };
        match act {
            Act::None => Vec::new(),
            Act::Browse(load) => vec![load],
            Act::Confirm => self.confirm(state),
        }
    }

    /// Ascend to the parent listing dir (or, on an option row, h-scroll left).
    fn parent(&self) -> Vec<Intent> {
        let mut f = self.form.borrow_mut();
        let Some(form) = f.as_mut() else { return Vec::new() };
        if form.on_builder {
            return Vec::new();
        }
        if form.cursor < form.options.len() {
            form.opt_hscroll.retreat(1);
            return Vec::new();
        }
        let Some(parent) = form.list_cwd.parent().map(|p| p.to_path_buf()) else {
            return Vec::new();
        };
        vec![form.browse(parent)]
    }

    /// Space: toggle the option checkbox / listing mark under the cursor.
    fn space(&self) -> Vec<Intent> {
        let mut f = self.form.borrow_mut();
        let Some(form) = f.as_mut() else { return Vec::new() };
        if form.on_builder {
            return Vec::new();
        }
        if form.cursor < form.options.len() {
            form.options[form.cursor].checked = !form.options[form.cursor].checked;
            return Vec::new();
        }
        if let Some(e) = form.entries.get(form.cursor - form.options.len()) {
            let p = e.path.clone();
            if !form.list_marks.remove(&p) {
                form.list_marks.insert(p);
            }
        }
        Vec::new()
    }

    /// Enter: on the builder → edit mode (a seeded generic input); on a body row →
    /// build the copy/move request and close.
    fn confirm(&self, state: &AppState) -> Vec<Intent> {
        // On the builder: open edit mode seeded with the live command line.
        {
            let f = self.form.borrow();
            if let Some(form) = f.as_ref() {
                if form.on_builder {
                    let seed = form.preview(&state.targets(), None);
                    return vec![Intent::BeginInput { purpose: "builder-edit".into(), seed, multiline: false }];
                }
            } else {
                return Vec::new();
            }
        }
        self.run(state, None)
    }

    /// Build and emit the resolver request from the form (+ optional prompt text),
    /// then close the form.
    fn run(&self, state: &AppState, prompt: Option<&str>) -> Vec<Intent> {
        let sources = state.targets();
        let Some(form) = self.form.borrow_mut().take() else { return Vec::new() };
        if form.op == Op::Create {
            // Unreachable via the normal flow: Create's whole interaction is
            // the "create-lines" FunctionInput, which never reaches `confirm`
            // (its only caller) — the browsing/option-list navigation that
            // leads here doesn't exist for Create. Kept only so this match
            // stays exhaustive over `Op`.
            return vec![Intent::SetFunctionForm(None)];
        }
        let opts: Vec<String> = form.options.iter().filter(|o| o.checked).map(|o| o.token.clone()).collect();
        let dest = prompt
            .map(String::from)
            .unwrap_or_else(|| form.destination().to_string_lossy().into_owned());
        let req = match form.op {
            Op::Copy | Op::Move => ResolverRequest {
                op: form.op.key().into(),
                src: None,
                dst: Some(dest),
                path: None,
                paths: sources,
                opts,
                label: form.op.key().into(),
            },
            Op::Rename => ResolverRequest {
                op: "rename".into(),
                src: sources.into_iter().next(),
                dst: Some(dest),
                path: None,
                paths: Vec::new(),
                opts,
                label: "rename".into(),
            },
            Op::Create => unreachable!("handled by the early return above"),
        };
        vec![Intent::RunResolver(req), Intent::SetFunctionForm(None)]
    }

    /// A generic function-panel input was submitted / cancelled — dispatch by its
    /// `purpose` tag.
    fn on_input(&self, submitted: bool, data: &ExtensionValue, state: &AppState) -> Vec<Intent> {
        let purpose = data.get("purpose").and_then(|v| v.as_str()).unwrap_or("");
        let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        match purpose {
            "filter" => {
                if let Some(form) = self.form.borrow_mut().as_mut() {
                    form.filter_query = if submitted && !text.is_empty() { Some(text) } else { None };
                }
                Vec::new()
            }
            // Submit-only (unlike "filter"'s live per-keystroke narrowing): a
            // deep walk per keystroke would spam searches for no benefit.
            // Esc / an empty submit leaves the browser exactly as it was —
            // no deep search issued, `deep_results` stays false.
            "search-deep" => {
                if !submitted || text.is_empty() {
                    return Vec::new();
                }
                let Some(root) = self.form.borrow_mut().as_mut().map(|form| {
                    form.deep_query = Some(text.clone());
                    form.awaiting_deep = true;
                    form.entries.clear(); // loading state until results land
                    form.list_cwd.clone()
                }) else {
                    return Vec::new();
                };
                vec![Intent::LoadSearch { owner: ID.into(), root, query: text }]
            }
            "prompt" if submitted => self.run(state, Some(&text)),
            "prompt" | "create-lines" if !submitted => self.close(),
            "builder-edit" => {
                if submitted {
                    let argv: Vec<String> = text.split_whitespace().map(String::from).collect();
                    if argv.is_empty() {
                        self.close()
                    } else {
                        self.form.borrow_mut().take();
                        self.run_command(argv, state)
                    }
                } else {
                    Vec::new() // back to builder navigation
                }
            }
            "create-lines" => self.run_create(&text),
            _ => Vec::new(),
        }
    }
}

impl CmdOp {
    /// How to run a command the builder assembled: `Execute` into the Exec
    /// frame, or — when `cmdop.runner` names a registered extension — that
    /// extension's `"run"` (the runner contract, doc/CMDOP.md). The runner
    /// claims the panel itself, so the form is released first.
    fn run_command(&self, argv: Vec<String>, state: &AppState) -> Vec<Intent> {
        let runner = self.store.get().map(|s| s.get_str("cmdop.runner")).unwrap_or_default();
        if runner.is_empty() {
            return vec![Intent::SetFunctionForm(None), Intent::Execute { argv }];
        }
        if !self.extension_ids.get().is_some_and(|ids| ids.contains(&runner)) {
            return vec![
                Intent::SetFunctionForm(None),
                Intent::Notify(format!("cmdop.runner: no extension \"{runner}\"; running captured")),
                Intent::Execute { argv },
            ];
        }
        let label = argv.join(" ");
        let data = std::collections::BTreeMap::from([
            ("argv".to_string(), ExtensionValue::List(argv.into_iter().map(ExtensionValue::String).collect())),
            ("cwd".to_string(), ExtensionValue::Path(state.cwd.clone())),
            ("label".to_string(), ExtensionValue::String(label)),
            ("escape".to_string(), ExtensionValue::String("prefix-and-cursor".into())),
        ]);
        vec![
            Intent::SetFunctionForm(None),
            Intent::Extension(ExtensionIntent { extension: runner, intent: "run".into(), data: ExtensionValue::Map(data) }),
        ]
    }
}

/// The active filter query to apply this frame: the live input while filtering,
/// else the persisted query.
fn active_filter<'a>(form: &'a Form, input: Option<&'a FunctionInput>) -> Option<&'a str> {
    match input {
        Some(i) if i.purpose == "filter" => Some(i.query.as_str()),
        _ => form.filter_query.as_deref(),
    }
}

fn substr(name: &str, q: &str) -> bool {
    name.to_lowercase().contains(&q.to_lowercase())
}

/// Parse a create-lines buffer into `(resolver_op, name)` pairs: one per
/// non-blank line, trimmed, a trailing `/` deciding `mkdir` (stripped from
/// the name) vs `touch`. Shared by `run_create` (what actually gets built)
/// and `create_preview` (what the CLI-builder shows while typing), so the two
/// can never disagree about what a line means.
fn create_targets(text: &str) -> impl Iterator<Item = (&'static str, &str)> {
    text.split('\n').map(str::trim).filter(|l| !l.is_empty()).map(|line| match line.strip_suffix('/') {
        Some(dir) => ("mkdir", dir),
        None => ("touch", line),
    })
}

/// The live CLI-builder preview for the create-lines buffer: one resolved
/// `mkdir -p NAME` / `touch NAME` line per typed name.
fn create_preview(text: &str) -> String {
    create_targets(text)
        .map(|(op, name)| if op == "mkdir" { format!("mkdir -p {name}") } else { format!("touch {name}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The (row, column) of byte offset `caret` within a multi-line `text` — both
/// counted from the start of their line, `column` in bytes (matching `caret`
/// itself, a byte offset — see `FunctionInput::caret`).
fn row_col_of(text: &str, caret: usize) -> (usize, usize) {
    let mut row = 0;
    let mut line_start = 0;
    for (i, ch) in text.char_indices() {
        if i >= caret {
            break;
        }
        if ch == '\n' {
            row += 1;
            line_start = i + 1;
        }
    }
    (row, caret.saturating_sub(line_start))
}

/// Render the Create builder: the live `mkdir`/`touch` CLI preview (one line
/// per typed name), a rule, then the multi-line name buffer itself with a
/// caret — no destination browser, no options, unlike Copy/Move/Rename.
fn render_create_form(frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx, theme: &crate::core::Theme) -> String {
    let title = "Create".to_string();
    let input = ctx.func.input.as_ref().filter(|i| i.purpose == "create-lines");
    let query = input.map(|i| i.query.as_str()).unwrap_or("");
    let caret = input.map(|i| i.caret).unwrap_or(0);

    let text_w = inner.width;
    let bottom = inner.y + inner.height;
    let mut row = inner.y;

    // ── CLI-builder preview ──
    let preview = create_preview(query);
    let bright = Style::default().fg(theme.role(Role::Base)).add_modifier(Modifier::BOLD);
    let preview_lines: Vec<Line> = if preview.is_empty() {
        vec![Line::from(Span::styled("(type a name below)", theme.style(Role::Muted)))]
    } else {
        preview.lines().map(|l| Line::from(Span::styled(l.to_string(), bright))).collect()
    };
    let preview_h = (preview_lines.len() as u16).min(inner.height.saturating_sub(2)).max(1);
    frame.render_widget(
        Paragraph::new(preview_lines),
        Rect { x: inner.x, y: row, width: text_w, height: preview_h },
    );
    row += preview_h;
    if row >= bottom {
        return title;
    }

    h_rule(frame, inner.x, row, inner.width, theme);
    row += 1;
    if row >= bottom {
        return title;
    }

    // ── The multi-line name buffer, with a caret on its own line ──
    let (caret_row, caret_col) = row_col_of(query, caret);
    for (i, line) in query.split('\n').enumerate() {
        if row >= bottom {
            break;
        }
        if i == caret_row {
            let before = line.get(..caret_col).unwrap_or(line).to_string();
            let under_ch = line.get(caret_col..).and_then(|s| s.chars().next());
            let under = under_ch.map(|c| c.to_string()).unwrap_or_else(|| " ".into());
            let after_start = caret_col + under_ch.map(|c| c.len_utf8()).unwrap_or(0);
            let after = line.get(after_start..).unwrap_or("").to_string();
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(before),
                    Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
                    Span::raw(after),
                ])),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
        } else {
            frame.render_widget(
                Paragraph::new(Span::raw(line.to_string())),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
        }
        row += 1;
    }
    if row < bottom {
        let hint = "one name per line — trailing / makes a dir  Enter: new line  blank Enter: create  Esc: cancel";
        frame.render_widget(
            Paragraph::new(Span::styled(hint, theme.style(Role::Muted))),
            Rect { x: inner.x, y: row, width: text_w, height: 1 },
        );
    }
    title
}

/// Greedy wrapped-row count (matches ratatui `Wrap { trim: false }` closely enough
/// to bound the builder's vertical scroll).
fn wrapped_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let mut rows = 0usize;
    for logical in text.split('\n') {
        rows += 1;
        let mut col = 0usize;
        let place = |rows: &mut usize, col: &mut usize, wlen: usize| {
            if wlen <= width {
                *col = wlen;
            } else {
                *rows += (wlen - 1) / width;
                *col = wlen - (wlen - 1) / width * width;
            }
        };
        for (i, word) in logical.split(' ').enumerate() {
            let wlen = word.chars().count();
            if i == 0 {
                place(&mut rows, &mut col, wlen);
            } else if col + 1 + wlen <= width {
                col += 1 + wlen;
            } else {
                rows += 1;
                place(&mut rows, &mut col, wlen);
            }
        }
    }
    rows.max(1)
}

/// Window offset that keeps `cursor` visible in `viewport` rows over `total` (the
/// file-list follow-cursor rule).
fn follow_cursor(prev: usize, cursor: usize, viewport: usize, total: usize) -> usize {
    if viewport == 0 || total == 0 {
        return 0;
    }
    let max_off = total.saturating_sub(viewport);
    let mut off = prev.min(max_off);
    if cursor < off {
        off = cursor;
    } else if cursor >= off + viewport {
        off = cursor + 1 - viewport;
    }
    off.min(max_off)
}

/// The cursor cell style: bright while the panel is focused, subdued otherwise.
fn cursor_style(theme: &crate::core::Theme, focused: bool) -> Style {
    theme.style(if focused { Role::CursorActive } else { Role::Cursor })
}

fn h_rule(frame: &mut Frame, x: u16, y: u16, width: u16, theme: &crate::core::Theme) {
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(width as usize), theme.style(Role::Border))),
        Rect { x, y, width, height: 1 },
    );
}

#[allow(clippy::too_many_arguments)]
fn section_scrollbar(
    frame: &mut Frame,
    x: u16,
    y: u16,
    height: u16,
    total: usize,
    viewport: usize,
    position: usize,
    theme: &crate::core::Theme,
) {
    if height == 0 {
        return;
    }
    let style = theme.style(Role::Border);
    if total <= viewport {
        let buf = frame.buffer_mut();
        for yy in y..y + height {
            if let Some(cell) = buf.cell_mut((x, yy)) {
                cell.set_char('│');
                cell.set_style(style);
            }
        }
        return;
    }
    let positions = total - viewport + 1;
    let mut st = ScrollbarState::new(positions).viewport_content_length(viewport).position(position.min(positions - 1));
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .style(style);
    frame.render_stateful_widget(bar, Rect { x, y, width: 1, height }, &mut st);
}

impl Extension for CmdOp {
    fn id(&self) -> &str {
        ID
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new("CmdOp", "Copy / move / rename command builder in the function panel")
    }

    fn bind_store(&self, store: &Arc<crate::core::OptionStore>) {
        let _ = self.store.set(store.clone());
    }

    fn bind_services(&self, resolver: &Arc<ResolverChain>) {
        let _ = self.resolver.set(resolver.clone());
    }

    fn handle_intent(&self, ei: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        if ei.extension != ID {
            return Vec::new();
        }
        if let Some(op) = Op::from_intent(&ei.intent) {
            return self.start(op, state);
        }
        if ei.intent == "start-create" {
            return self.start_create(state);
        }
        match ei.intent.as_str() {
            "cursor-down" => {
                if let Some(f) = self.form.borrow_mut().as_mut() {
                    if f.on_builder {
                        if f.builder_scroll.at_end() {
                            f.on_builder = false;
                            f.cursor = 0;
                        } else {
                            f.builder_scroll.advance(1);
                        }
                    } else {
                        let total = f.options.len() + f.entries.len();
                        if f.cursor + 1 < total {
                            f.cursor += 1;
                        }
                        f.reset_hscroll_off_options();
                    }
                }
                Vec::new()
            }
            "cursor-up" => {
                if let Some(f) = self.form.borrow_mut().as_mut() {
                    if f.on_builder {
                        f.builder_scroll.retreat(1);
                    } else if f.cursor == 0 {
                        f.on_builder = true;
                        f.builder_scroll.go_top();
                    } else {
                        f.cursor -= 1;
                        f.reset_hscroll_off_options();
                    }
                }
                Vec::new()
            }
            "descend" => self.descend(state),
            "parent" => self.parent(),
            "section-toggle" => {
                if let Some(f) = self.form.borrow_mut().as_mut() {
                    f.on_builder = false;
                    if f.cursor < f.options.len() {
                        f.cursor = f.options.len();
                    } else if !f.options.is_empty() {
                        f.cursor = 0;
                    }
                }
                Vec::new()
            }
            "space" => self.space(),
            "confirm" => self.confirm(state),
            "cancel" => self.close(),
            // `/` is two-stage (doc/CMDOP.md): deep search over
            // the browsed dir until one has landed for it, then shallow live
            // narrowing of those results — same key, same purpose the user
            // already reaches for, escalating from "find it" to "narrow it".
            "filter-start" => {
                let deep_results = self.form.borrow().as_ref().is_some_and(|f| f.deep_results);
                if deep_results {
                    let seed = self.form.borrow().as_ref().and_then(|f| f.filter_query.clone()).unwrap_or_default();
                    vec![Intent::BeginInput { purpose: "filter".into(), seed, multiline: false }]
                } else {
                    vec![Intent::BeginInput { purpose: "search-deep".into(), seed: String::new(), multiline: false }]
                }
            }
            "input-submit" => self.on_input(true, &ei.data, state),
            "input-cancel" => self.on_input(false, &ei.data, state),
            _ => Vec::new(),
        }
    }

    fn accept_content(&self, path: &std::path::Path, result: &Content) {
        // Self-gate: only accept the listing for the directory we are browsing
        // (also rejects a deep-search completion that arrives after the user
        // has since browsed elsewhere — `awaiting_deep` for the old dir is
        // already false by then, reset by `Form::reset_deep`).
        if let Content::Dir { entries } = result
            && let Some(form) = self.form.borrow_mut().as_mut()
                && form.list_cwd == path {
                    form.entries = entries.clone();
                    // A deep search (Intent::LoadSearch) completes through
                    // this same generic content Read. Distinguish it from an ordinary (re)listing
                    // by the pending flag `search-deep`'s submit set.
                    if form.awaiting_deep {
                        form.awaiting_deep = false;
                        form.deep_results = true;
                    }
                    let min = form.options.len();
                    if form.cursor < min {
                        form.cursor = min;
                    }
                    let max = min + form.entries.len().saturating_sub(1);
                    if !form.entries.is_empty() && form.cursor > max {
                        form.cursor = max;
                    }
                }
    }

    fn render_form(&self, frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx) -> Option<String> {
        let mut form_ref = self.form.borrow_mut();
        let form = form_ref.as_mut()?;
        let theme = ctx.theme;
        // While a deep search (awaiting or landed) is active for the browsed
        // dir, the title shows its query — the two-stage `/` is otherwise
        // invisible once the filter row itself is showing the same text for
        // the *shallow* stage.
        let title = match &form.deep_query {
            Some(q) => format!("{} — search: {q}", form.op.title()),
            None => form.op.title().to_string(),
        };
        if inner.height == 0 {
            return Some(title);
        }
        if form.op == Op::Create {
            return Some(render_create_form(frame, inner, ctx, theme));
        }
        let input = ctx.func.input.as_ref();
        // The prompt / builder-edit text currently being typed (if any).
        let prompt_text = input.filter(|i| i.purpose == "prompt").map(|i| (i.query.as_str(), i.caret));
        let edit_text = input.filter(|i| i.purpose == "builder-edit").map(|i| (i.query.as_str(), i.caret));

        let bottom = inner.y + inner.height;
        let sb_x = inner.x + inner.width.saturating_sub(1);
        let text_w = inner.width.saturating_sub(1);
        let mut row = inner.y;

        // ── CLI builder (bright; reverse when focused; caret while editing) ──
        let preview_h = PREVIEW_H.min(inner.height);
        let bright = Style::default().fg(theme.role(Role::Base)).add_modifier(Modifier::BOLD);
        let dest_override = prompt_text.map(|(t, _)| t);
        let builder_str = match edit_text {
            Some((t, _)) => t.to_string(),
            None => form.preview(ctx.sources, dest_override),
        };
        let para = if let Some((t, caret)) = edit_text {
            let before = t[..caret].to_string();
            let under_ch = t[caret..].chars().next();
            let under = under_ch.map(|c| c.to_string()).unwrap_or_else(|| " ".into());
            let after_start = caret + under_ch.map(|c| c.len_utf8()).unwrap_or(0);
            Paragraph::new(Line::from(vec![
                Span::styled(before, bright),
                Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
                Span::styled(t[after_start..].to_string(), bright),
            ]))
        } else {
            let style = if form.on_builder {
                cursor_style(theme, ctx.focused).add_modifier(Modifier::BOLD)
            } else {
                bright
            };
            Paragraph::new(Span::styled(builder_str.clone(), style))
        };
        let total_lines = wrapped_rows(&builder_str, text_w as usize);
        let bmax = total_lines.saturating_sub(preview_h as usize);
        form.builder_scroll.bind(bmax);
        let bld_scroll = form.builder_scroll.offset();
        frame.render_widget(
            para.wrap(Wrap { trim: false }).scroll((bld_scroll as u16, 0)),
            Rect { x: inner.x, y: row, width: text_w, height: preview_h },
        );
        if total_lines > preview_h as usize {
            section_scrollbar(frame, sb_x, row, preview_h, total_lines, preview_h as usize, bld_scroll, theme);
        }
        row += preview_h;
        if row >= bottom {
            return Some(title);
        }

        h_rule(frame, inner.x, row, inner.width, theme);
        row += 1;
        if row >= bottom {
            return Some(title);
        }

        // Reserve the prompt rows at the bottom (rename dest).
        let prompt_h: u16 = if prompt_text.is_some() { 2 } else { 0 };
        let body_bottom = bottom.saturating_sub(prompt_h);

        // Filtered listing (live from the active filter, else the persisted query).
        let filt = active_filter(form, input).map(str::to_string);
        let filter_visible = filt.is_some();
        let visible: Vec<&Entry> = match &filt {
            Some(q) if !q.is_empty() => form.entries.iter().filter(|e| substr(&e.name, q)).collect(),
            _ => form.entries.iter().collect(),
        };

        // Split the body: options (capped at half), then the listing.
        let opt_sep: u16 = if form.options.is_empty() { 0 } else { 1 };
        let filter_h: u16 = if filter_visible { 1 } else { 0 };
        let body_h = body_bottom.saturating_sub(row);
        let opt_cap = body_h.saturating_sub(opt_sep + filter_h) / 2;
        let opt_vh: u16 = (form.options.len() as u16).min(opt_cap.max(if opt_sep > 0 { 1 } else { 0 }));

        // ── Option rows ──
        if form.cursor < form.options.len() {
            form.opt_off = follow_cursor(form.opt_off, form.cursor, opt_vh as usize, form.options.len());
        }
        let opt_off = form.opt_off;
        let label_w = (text_w as usize).saturating_sub(OPT_PREFIX_W);
        let longest = form.options.iter().map(|o| o.label.chars().count()).max().unwrap_or(0);
        form.opt_hscroll.bind(longest.saturating_sub(label_w));
        let opt_h = form.opt_hscroll.offset();
        let opt_top = row;
        for vi in 0..opt_vh {
            let Some(opt) = form.options.get(opt_off + vi as usize) else { break };
            let i = opt_off + vi as usize;
            let mark = if opt.checked { "[x]" } else { "[ ]" };
            let style = if form.cursor == i && !form.on_builder {
                cursor_style(theme, ctx.focused)
            } else {
                theme.style(Role::Entry)
            };
            let body: String = opt.label.chars().skip(opt_h).collect();
            frame.render_widget(
                Paragraph::new(Span::styled(format!("{mark} {body}"), style)),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
            row += 1;
        }
        if opt_vh > 0 {
            section_scrollbar(frame, sb_x, opt_top, opt_vh, form.options.len(), opt_vh as usize, opt_off, theme);
        }

        if !form.options.is_empty() && row < body_bottom {
            h_rule(frame, inner.x, row, inner.width, theme);
            row += 1;
        }

        // ── Filter row ──
        if filter_visible && row < body_bottom {
            let editing = input.map(|i| i.purpose == "filter").unwrap_or(false);
            let marker = if editing { "/ " } else { "  " };
            let q = filt.as_deref().unwrap_or("");
            frame.render_widget(
                Paragraph::new(Span::styled(format!("{marker}{q}"), theme.style(Role::SelectionMarker))),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
            row += 1;
        }

        // ── Listing ──
        let list_top = row;
        let list_vh = body_bottom.saturating_sub(row);
        let list_cursor = form.cursor.saturating_sub(form.options.len());
        let on_listing = form.cursor >= form.options.len() && !form.on_builder;
        if on_listing {
            form.list_off = follow_cursor(form.list_off, list_cursor, list_vh as usize, visible.len());
        }
        let list_off = form.list_off;
        for vi in 0..list_vh {
            let Some(entry) = visible.get(list_off + vi as usize) else { break };
            let i = list_off + vi as usize;
            let is_cursor = on_listing && i == list_cursor;
            let marked = form.list_marks.contains(&entry.path);
            let style = if is_cursor {
                cursor_style(theme, ctx.focused)
            } else if marked {
                theme.style(Role::SelectionMarker)
            } else if entry.is_dir {
                theme.style(Role::EntryDir)
            } else {
                theme.style(Role::Entry)
            };
            let marker = if marked { "*" } else { " " };
            let icon = if entry.is_dir { "/ " } else { "  " };
            frame.render_widget(
                Paragraph::new(Span::styled(format!("{marker}{icon}{}", entry.name), style)),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
            row += 1;
        }
        if list_vh > 0 {
            section_scrollbar(frame, sb_x, list_top, list_vh, visible.len(), list_vh as usize, list_off, theme);
        }

        // ── Prompt (rename dest) ──
        if let Some((t, caret)) = prompt_text {
            let mut row = body_bottom;
            if row < bottom {
                h_rule(frame, inner.x, row, inner.width, theme);
                row += 1;
            }
            if row < bottom {
                let before = Span::raw(t[..caret].to_string());
                let under_ch = t[caret..].chars().next();
                let under = under_ch.map(|c| c.to_string()).unwrap_or_else(|| " ".into());
                let after_start = caret + under_ch.map(|c| c.len_utf8()).unwrap_or(0);
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled("→ ", theme.style(Role::SelectionMarker)),
                        before,
                        Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
                        Span::raw(t[after_start..].to_string()),
                    ])),
                    Rect { x: inner.x, y: row, width: inner.width, height: 1 },
                );
            }
        }

        Some(title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, name: &str, is_dir: bool) -> Entry {
        Entry {
            path: path.into(),
            name: name.into(),
            is_dir,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0, owner: "".into(), group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }

    fn state_with_cursor() -> AppState {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = vec![entry("/d/a", "a", false)];
        s.cursor = 0;
        s
    }

    fn ei(intent: &str) -> ExtensionIntent {
        ExtensionIntent { extension: ID.into(), intent: intent.into(), data: ExtensionValue::Nil }
    }

    #[test]
    fn start_copy_opens_the_form_and_reads_the_listing() {
        let ext = CmdOp::new(Arc::default());
        let out = ext.handle_intent(&ei("start-copy"), &state_with_cursor());
        assert!(ext.form.borrow().is_some(), "the form is activated");
        // Shows/focuses the panel and reads the browse directory off-thread.
        assert!(matches!(out.first(), Some(Intent::SetFunctionForm(Some(id))) if id == ID));
        assert!(out.iter().any(|i| matches!(i, Intent::LoadContent { owner, .. } if owner == ID)));
    }

    fn store_with_runner(runner: &str) -> Arc<crate::core::OptionStore> {
        let store = Arc::new(crate::core::OptionStore::new(vec![crate::core::OptionDef {
            key: "cmdop.runner".into(),
            category: "CmdOp".into(),
            label: "runner".into(),
            kind: crate::core::OptionKind::Text,
            default: crate::core::OptionValue::Text(String::new()),
            owner: None,
        }]));
        store.set("cmdop.runner", crate::core::OptionValue::Text(runner.into()));
        store
    }

    fn cmdop_with(runner: &str, registered: &[&str]) -> CmdOp {
        let ids = Arc::new(OnceLock::new());
        let _ = ids.set(registered.iter().map(|s| s.to_string()).collect());
        let c = CmdOp::new(ids);
        c.bind_store(&store_with_runner(runner));
        c
    }

    #[test]
    fn confirm_emits_the_resolver_and_closes() {
        let ext = CmdOp::new(Arc::default());
        let s = state_with_cursor();
        ext.handle_intent(&ei("start-copy"), &s);
        let out = ext.handle_intent(&ei("confirm"), &s);
        assert!(ext.form.borrow().is_none(), "the form closes on confirm");
        assert!(out.iter().any(|i| matches!(i, Intent::RunResolver(r) if r.op == "copy")));
        assert!(out.iter().any(|i| matches!(i, Intent::SetFunctionForm(None))));
    }

    // --- `/` two-stage deep-then-shallow search (doc/CMDOP.md) ------------

    fn submit_search_deep(ext: &CmdOp, s: &AppState, text: &str) -> Vec<Intent> {
        let data = ExtensionValue::Map(
            [
                ("purpose".to_string(), ExtensionValue::String("search-deep".into())),
                ("text".to_string(), ExtensionValue::String(text.into())),
            ]
            .into_iter()
            .collect(),
        );
        ext.handle_intent(&ExtensionIntent { extension: ID.into(), intent: "input-submit".into(), data }, s)
    }

    #[test]
    fn accepting_deep_results_flips_slash_to_the_shallow_filter() {
        let ext = CmdOp::new(Arc::default());
        let s = state_with_cursor();
        ext.handle_intent(&ei("start-copy"), &s);
        submit_search_deep(&ext, &s, "needle");

        // The deep search completes (owner-addressed Search → the same
        // generic content Read every LoadContent completion uses).
        let found = vec![entry("/d/sub/needle.txt", "needle.txt", false)];
        ext.accept_content(std::path::Path::new("/d"), &Content::Dir { entries: found.clone() });
        assert!(ext.form.borrow().as_ref().unwrap().deep_results);
        assert!(!ext.form.borrow().as_ref().unwrap().awaiting_deep);
        assert_eq!(ext.form.borrow().as_ref().unwrap().entries, found);

        // `/` now means shallow live narrowing of these results, not another
        // deep walk.
        let out = ext.handle_intent(&ei("filter-start"), &s);
        assert!(matches!(out.as_slice(), [Intent::BeginInput { purpose, .. }] if purpose == "filter"));
    }

    #[test]
    fn a_stale_deep_completion_for_an_abandoned_dir_is_ignored() {
        let ext = CmdOp::new(Arc::default());
        let s = state_with_cursor();
        ext.handle_intent(&ei("start-copy"), &s);
        submit_search_deep(&ext, &s, "needle");
        // Leave the directory before the search completes. `parent` (unlike
        // `descend`) needs no entry under the cursor, so it works even while
        // the deep search has cleared the listing to a loading state.
        ext.handle_intent(&ei("parent"), &s);
        assert_eq!(ext.form.borrow().as_ref().unwrap().list_cwd, std::path::PathBuf::from("/"));
        // The stale completion, addressed to the old path, must not resurrect
        // deep_results/entries for the directory no longer being browsed.
        ext.accept_content(std::path::Path::new("/d"), &Content::Dir {
            entries: vec![entry("/d/needle.txt", "needle.txt", false)],
        });
        assert!(!ext.form.borrow().as_ref().unwrap().deep_results);
    }

}
