# Architecture

wasdf is a TUI filer (Rust / ratatui / crossterm) built around a single intent
pipeline: key → Intent → plan → kernel async → AsyncResult → reducer → render.
This document defines the engine: the component model, the pipeline, the intent
catalog, the purity and render contracts, the async contract, the resolver
contract, state ownership, the module layout, and the startup sequence.
User-facing layout, modes, and keymaps are in [UI.md](UI.md); extensions in
[EXTENSION.md](EXTENSION.md); Scheme configuration in [SCHEME.md](SCHEME.md).

No module interprets a junction point on its own. MVP closure takes priority
over future extensibility. The default failure behavior everywhere is: disable,
notify, continue.

## Component Model

There are exactly two kinds of components. There is no plugin tier and there
are no per-service plugin traits.

| Kind | What it is |
|------|-----------|
| Kernel service | A concrete struct owned by the kernel. One implementation each, configured by data; no registration machinery. Built-in UI features (CmdOp — [CMDOP.md](CMDOP.md)) are part of the kernel too, even when implemented on the form seam extensions use |
| Extension | Implements the single Extension trait. Bundled (statically linked) or optional (dynamically loaded) — [EXTENSION.md](EXTENSION.md) |

Kernel services:

| Service | Role |
|---------|------|
| KeymapRegistry | (mode, panel, key) → Intent; the only key-to-Intent path ([UI.md](UI.md)) |
| Conditions | the `:when` predicate evaluator ([SCHEME.md](SCHEME.md)) |
| CommandRegistry | palette commands |
| ResolverChain | operation key → argv, plus the memo of options extracted from `--help`/`man` |
| OptionStore | the native **projection** of the settings store's options: typed effective values, with a **generation** bumped on every change. Read-only to everyone but the kernel's change-set applier |
| ThemeCell | the active `Theme` as a shared `Arc<Theme>`; written by whoever owns themes, read by render |
| Persister | writes `custom.scm` — the store's `custom` layer, dumped by the session — on its own thread, coalesced and atomic ([SCHEME.md](SCHEME.md)) |
| FileWriter | write-behind for files an extension owns (e.g. the bookmarks list): coalesced per path, atomic, off the main thread |
| Vfs | the virtual filesystem router (`@/…`), filesystem default + extension mounts |
| TaskManager | worker threads executing plans |
| MessageLog | `@/messages` — the single sink for everything wasdf tells the user |
| Monitor | the top panel's system metrics sampler |
| SchemeSession | the resident steel session; it holds the **settings store**, the single source of truth for options, bindings, layouts, commands, and display orders. The kernel keeps native projections of it (OptionStore, the keymap's User layer, CommandRegistry, layouts, orders) and updates them from each evaluation's change set ([SCHEME.md](SCHEME.md)) |

The only service trait is MatcherBackend (fuzzy ranking; one shipped
implementation, SkimMatcher — a test seam, not an extension point).

## The Intent Pipeline

Every keystroke flows through one ordered path. There are no mode-specific or
extension-specific side channels.

1. **Key routing** (`handle_key`), in this fixed order:
   - **Input capture.** While an extension holds an `InputCapture`, the
     capture's `release` key clears it unconditionally and then delivers
     `"prefix"` to the owner; a key the keymap resolves to one of the
     capture's `escape` intents clears it and dispatches that intent; every
     other key goes to the owner as an extension intent `"key"` carrying the
     lossless key encoding. Core alone honors the release, so a wedged
     extension cannot trap the user.
   - **Text input.** While the function-panel search input or a
     function-panel form input (`BeginInput`) is open, keys are delivered as
     `RawKeyEvent` — except Enter/Esc for the search input, which still
     resolve through the keymap. (A Select query and InputForm's typing kinds
     are not routed here: their keys go through the keymap, gated by phase /
     `input-form-typing`, and fall through as `RawKeyEvent` when unbound.)
   - **Active form.** While an extension holds the function panel as an
     *active* form (claimed with `SetFunctionForm`) and the function panel has
     focus, a key whose binding would dispatch a file-targeting intent
     (`Cursor*`, `Activate` — the intents that move focus to the file panel)
     is not resolved to that intent: it goes to the form owner as an
     extension intent `"key"` (lossless encoding), so the form keeps focus and
     can use `wasd` itself. The form's own bindings resolve normally. A
     *passive* form (`HostFunctionForm`) changes nothing.
   - **Keymap.** KeymapRegistry resolves (mode, focused panel, key) to an
     Intent. Scopes match by longest mode-id prefix, then panel scope, then
     layer priority (Core < Extension < User); `:when` conditions are
     evaluated natively. Only `Press` events resolve. In Select and InputForm
     an unbound key is delivered as `RawKeyEvent`.
2. **Dispatch** (`dispatch_intent`), in ordered stages:
   - Policy gate — while a Policy overlay is up, only Confirm/Cancel act.
   - Extension dispatch — an extension intent goes to its owner's
     `handle_intent`, whose returned intents are re-dispatched (depth-capped).
     If its data carries the reserved resolver key it is instead a plan the
     kernel executes (never re-dispatched).
   - Confirmation gate — plans that need confirmation (see Resolver Contract)
     reduce `PushMode(Policy(intent))` instead of running.
   - Reduction — the pure reducer (`apply_intent`) applies the transition and
     returns Effects: plans to spawn, notices to log, follow-up intents.
3. **Cursor-changed broadcast.** When the cursor signature (cursor path,
   function-panel visibility, function scroll offset, mode id) changes, the
   kernel calls every extension's `on_cursor_changed(&AppState)` and
   re-dispatches what they return. If a subscriber released the function
   panel during the pass (a form owner stepping aside), subscribers are
   notified once more — exactly once, since the panel cannot be released
   twice.
4. **Tick.** After each event the kernel calls every extension's `on_tick`,
   the only place out-of-band state (a child process exiting) becomes intents.
5. **Render** (see Render Contract).

Rules:

- The event loop and `dispatch_intent` contain no per-intent special-casing
  beyond the structural gates above. "Do Z before X" is expressed by the reducer
  returning follow-up intents.
- Extensions are called synchronously (`handle_intent`, `accept_content`,
  `view`, `on_cursor_changed`, `on_tick`) and return data/intents; the kernel
  owns all execution and all state mutation (via the reducer).

## Intent Catalog

Core intents are a closed set; an extension feature never adds a core variant.
Extension intents travel as data ([EXTENSION.md](EXTENSION.md)).

| Category | Intents | Notes |
|----------|---------|-------|
| Navigation | CursorUp, CursorDown, CursorTop, CursorBottom, CursorLeft, CursorRight, Activate, NavigateTo, ToggleVfsRoot | Geometric moves over `AppState.list_geom`; the left wall escapes to the parent; the Rows right wall acts on the entry. Activate enters a directory or opens the file via the resolver. NavigateTo issues a ReadDir for its target — whether the target is a directory is decided by the worker, not the reducer (see ReadDir). The same Cursor intents drive `Mode::InputForm`'s widgets ([UI.md](UI.md)) |
| Function cursor | FuncUp, FuncDown, FuncLeft, FuncRight | Operate the function panel from the file panel (scroll / h-scroll) |
| Selection | ToggleSelect, SelectAll, ClearSelection | Selection is a path set ([UI.md](UI.md)) |
| Filter | ClearFilter, FilterListing, SelectRefine | Shallow filtering and the filter stack ([UI.md](UI.md)). FilterListing / SelectRefine carry no data; the reducer ranks the pool it already holds, synchronously |
| External commands | RunResolver, Open, Edit, Execute | RunResolver carries a `ResolverRequest` (op + targets + dst/path + opts) |
| Initiators | DeleteSelected, StartEdit | Expanded by the reducer into the operations above |
| Mode | PushMode, PopMode | Mode stack transitions |
| Function panel | SetSubLayout, CycleFunctionPanel, HideFunctionPanel, ShowFunctionContent, UpdateFunctionView, LoadContent, LoadSearch, SetFunctionForm, HostFunctionForm, BeginInput | View state, not modes. `ShowFunctionContent{owner}` makes `owner` the content owner and shows the panel — it carries no content; the panel then pulls the owner's `view()`. LoadContent issues the owner-addressed `Plan::Read`; LoadSearch is its deep-search sibling (an owner-addressed `Plan::Search` completing through `accept_content`). SetFunctionForm claims/releases the panel as an *active* form (moves focus to it; see Active form under key routing); HostFunctionForm claims it as a *passive* form (focus untouched, no key diversion). BeginInput opens a function-panel text input tagged with a purpose; submit/cancel return to the form owner as `input-submit` / `input-cancel` |
| Scroll | ScrollUp, ScrollDown, ScrollTop, ScrollBottom, PageUp, PageDown, SetScroll | Function-panel vertical scroll |
| Content search | FunctionSearchStart, FunctionSearchSubmit, FunctionSearchNext, FunctionSearchPrev, FunctionSearchCancel, SetSearchMatches, ResetContentView | Less-style search; the owning extension computes matches in **logical coordinates** (block index, byte range within the block's text), core stores them (so `:when` can gate `n`/`p`) and maps them to panel rows at layout |
| Dialog | Confirm, Cancel | Phase-interpreted in Select; allow / deny in Policy; pop + re-inject in InputForm |
| InputForm rows | AddItem, RemoveItem | The `List` kind's row add/remove |
| Text | RawKeyEvent | Unbound keys into the active text input ([UI.md](UI.md), Line editing) |
| Panel | FocusNextPanel, FocusPrevPanel, FocusPanel | |
| View | ToggleDotFiles, ToggleLineNumbers, CycleListLayout | |
| Process | KillProcess | Exec frame only |
| Capture | SetInputCapture | Install / clear an extension's `InputCapture` |
| Scheme | Eval, Configure | `Eval{owner, expr}` evaluates an expression in the resident session (`Plan::EvalScheme`); the result goes to `owner`. `Configure(write)` is a structured store write in the `custom` layer — set / reset an option, bind / unbind, a layout, an order — that the codec renders as a store call; it is how the settings UI and extension editors change anything configurable |
| Misc | Refresh, Quit, Noop, Notify | Notify logs a line to `@/messages` |
| Extension | Extension, UpdateExtensionState | Extension intents as data; UpdateExtensionState is the single mechanism for extension-mode state updates |

## Purity Contract

The reducer is pure and synchronous: its output depends only on `AppState`,
the intent or result, and the read-only `CommandLookup`. (The one input that
can change between two identical calls is the resolver option memo behind
`CommandLookup`, which fills in once extraction lands.) Concretely it never:

- touches the filesystem (no `stat`, `is_dir`, `exists`, `read_dir`);
- reads the process environment — values it needs (`$HOME`, `$EDITOR`) are
  captured once at boot into `AppState.env`;
- starts a process or blocks — `CommandLookup` answers from in-memory data only
  (resolver option extraction is memoised off the input path, see Resolver
  Contract).

Extensions follow the same rule inside their synchronous hooks. Persisting
state is not an exception: a configurable value is changed by returning a
`Configure` intent (the store persists it), and a file the extension owns is
handed to FileWriter (via `Host`), which writes on its own thread. The sanctioned exceptions are exactly these, all belonging to the pty
extension ([PTY.md](PTY.md)):

| Exception | Where | Bound |
|-----------|-------|-------|
| Write to a PTY master | `handle_intent` | non-blocking; a write that would block is dropped, never retried |
| Open a PTY and spawn its child | `handle_intent` | once per explicit user action (open a shell, run a command, edit); failure is notified and the caller falls back |
| Resize a PTY | `render_form` | only when the panel's inner size changed since the last resize (one ioctl) |

## Render Contract

Render reads `AppState`, the render caches, and shared snapshots. It performs
no I/O (except the PTY resize listed under the Purity Contract), starts
nothing, and takes no lock that a worker can hold for longer than a copy.

- **What is drawn comes from state.** The nav panel's ancestor columns come
  from `AppState.trail` (computed by the ReadDir worker); owner / group names
  come from the `Entry` (resolved in the worker, memoised per id); system
  metrics and the load average come from the Monitor snapshot. Monitor always
  samples the load average (the prop readout); it samples the monitor panel's
  metrics only while that panel is visible.
- **Draw when dirty.** The loop draws after handling an event and otherwise
  sleeps. It waits with a timeout bounded by the nearest UI deadline: notice
  expiry, and the prop panel's refresh (its clock and load readout), due every
  `prop.refresh-ms` (default 1000; at most once per that interval). Owners of
  out-of-band sources (Monitor, a PTY reader thread) wake it with
  `AppEvent::Tick`. The input thread emits no idle ticks.
- **Derived values are cached by generation.** Render options derived from the
  OptionStore are rebuilt only when its generation changes; the theme is an
  `Arc<Theme>` clone from ThemeCell.
- **Function-panel layout is viewport-bounded.** The panel shows exactly one
  extension's content: the content owner's `view()`, a shared
  `Arc<PanelView>` that does not depend on the viewport; its pointer identity
  is the content revision. Core owns every width-dependent step — wrapping
  `Para` blocks, h-scroll, label alignment. UiManager caches, per (revision,
  inner width, line numbers, theme), the lowered rows, the block → row map, and
  the intrinsic content width, and per frame builds styled lines only for the
  visible slice. Image cells are cached per (revision, size). Search matches
  (logical coordinates, kept sorted) are mapped to rows through the block → row
  map, never by scanning the whole match list per row. Bringing a match into
  view is resolved there too: the reducer only marks `search.reveal`, the
  layout puts the current match's block on a row, and the kernel keeps that
  row as the scroll after the draw (the same render → state seam as the
  scroll bound).
- **The file list builds only its visible rows.** Column and field widths
  are measured over the listing so rows line up; the styled rows themselves
  (entry styles, size bars, times) are built for the window on screen only.
- **Per-entry hooks are O(1).** Hooks called per visible entry per frame
  (`entry_style`) resolve their tables when their inputs change, not per call.

## Async Contract

All I/O and heavy computation runs in kernel async tasks. The round trip is
fixed: Intent → Plan → kernel task → AsyncResult → reducer.

### Plans — a Closed Set

Plans are the only way side effects leave the pipeline. Extensions never define
their own plan kinds: extension file operations ride ResolveAndRun, and
extension content is loaded by the generic Read plan.

| Plan | Representative intents | purpose | Execution | payload |
|------|------------------------|---------|-----------|---------|
| ReadDir | NavigateTo, CursorRight/Left on a directory, Refresh, ToggleDotFiles | refresh | list a directory — or revalidate a filtered view — and compute the nav trail | Listing |
| Search | file-search; an owner-addressed deep search (LoadSearch) | search | walk + two-stage affinity re-rank | Ranked |
| Read | LoadContent | content | one bounded byte chunk from an offset (files) or the entries (directories) | Read (owner + path + Content) |
| ResolveAndRun | RunResolver, Open, extension resolver plans | resolver | Background: detached process, completion notification only | OpDone |
| Execute | Execute | execute | Captured: stdout streamed line by line into the Exec frame | Exec |
| Suspend | Edit | execute | Suspended: leave the alternate screen, run the child in the foreground, restore | OpDone |
| EvalScheme | Eval (the `repl` extension's `@/repl`), Configure | scheme | evaluate one expression — or one rendered store write — in the resident Scheme session | Scheme (owner, printed result or error, change set) |

Background, Captured, and Suspended are the only three external-process
execution forms. Only Execute shows output in the function panel.

**ReadDir.** The plan names a source: a **directory** (`requested`), or the
**paths of a filtered view** being refreshed. The payload is
`Listing { requested, dir, entries, trail, gone }`.

- Directory source: when `requested` is not a directory the worker lists its
  parent — `dir` is the directory actually listed and the reducer focuses
  `requested` in it. `trail` holds, for each ancestor of `dir`, its sibling
  directory names (the nav panel's columns), computed from the Vfs for
  virtual paths.
- Filtered-view source: the worker re-stats exactly those paths; `entries`
  are the survivors with fresh metadata, in the same order.
- Either way the worker also stats the selected paths not in the result;
  `gone` lists the selected paths that no longer exist.

`cwd` changes only when a Listing lands: while a navigation is in flight the
panel keeps showing the previous directory, and the reducer accepts a Listing
only for the request it last issued (`AppState.pending_listing`). A failed
directory read (the path does not exist, permission denied) is reported in the
notice area and the reducer navigates to `env.home` instead.



**Search.** The payload is `Ranked { root, owner, entries }`. With no owner it
is the file-search Select's result; with an owner it is handed to that
owner's `accept_content(root, Content::Dir)`, exactly like a Read. A walk is
expensive and a query is not: TaskManager caches the
walked candidate pool per (root, show_hidden, mode generation), so typing in one
file-search walks once and re-ranks per keystroke. Ranking runs in two stages:
a tail-biased fuzzy pass with no I/O keeps the top `RERANK_N.max(max_results)`
survivors, then those survivors are stat'ed and adjusted by the affinity
weights (tail bonus, depth / length penalty, exec bonus, preferred / noise
extensions, noise directories). `MatcherConfig` is built from the OptionStore
at spawn; `matcher.max-results` bounds both the walk and the result. The cancel
flag is checked inside the walk and the ranking loops, so a superseded search
stops early.

**Read.** Content-agnostic: the worker reads one bounded chunk and returns it
undecoded. The owning extension decodes on the main thread in `accept_content`
and pages further chunks itself by re-issuing LoadContent with the next offset
([EXTENSION.md](EXTENSION.md)).

**EvalScheme.** Evaluates in the environment `config.scm` and the extension
preludes built, so the store procedures, the environment procedures, and every
user and prelude definition are available ([SCHEME.md](SCHEME.md)). The
payload carries the **change set** — the store entries whose effective value
the evaluation changed. The kernel first applies the change set to the
projections (for every Scheme result, owned or not), then hands the printed
value or error to the owner's `accept_content("", Content::Value)` when there
is an owner. A `Configure` has no owner; a failed write is reported to
`@/messages`.

**Suspend.** Completion is an ordinary `(execute, OpDone)` result: the reducer
returns to File mode and refreshes, so an edit shows on return.

### AsyncResult — One Schema

| Field | Content |
|-------|---------|
| request_id | unique per spawn |
| purpose | one of content, search, resolver, execute, scheme, refresh — a fixed namespace, never extended |
| mode_generation | the mode instance id current at spawn time |
| status | Ok, Cancelled, Failed, StaleDiscarded |
| payload | Listing, Ranked, Read, OpDone, Exec, Scheme |

Each plan has exactly one purpose and one payload shape. The reducer consumes
results by the pair (purpose, payload); completion handling never branches per
intent. The one exception is a result **addressed to an owner** — a Read, a
Ranked with an owner, or a Scheme result: the kernel hands it to the owner's
`accept_content` as a `Content` (`Bytes`, `Dir`, or `Value`) — the reducer
cannot call extensions; a cursor-follow read also resets the view
(`ResetContentView`), a form owner's own read does not.

Execute is the one streaming plan: several `execute` results under one
request_id, one per stdout line and a final one carrying the exit code.
KillProcess cancels the child.

### Rules

- **One live task per key, for the unique purposes.** `content`, `search`,
  and `refresh` are unique; their key is the purpose, plus the owner for
  owner-addressed plans (`content`, and `search` with an owner). A new spawn cancels the previous task with
  the same key; handlers never hold task ids. A cursor-follow read and another
  extension's deep search therefore never cancel each other. `resolver`,
  `execute`, and `scheme` tasks are never cancelled by a later spawn (two
  copies both run and both report); KillProcess alone stops an Execute.
- **Staleness.** A result whose `mode_generation` no longer matches a
  mode-bound consumer (a Select's search) is StaleDiscarded. A cursor-follow
  read whose path is no longer under the cursor is dropped on arrival.
- **Notification.** Only Failed produces a user-visible error. Cancelled and
  StaleDiscarded are silent.
- A late completion for a cancelled request id is ignored.

## Resolver Contract

All external command execution resolves through the resolver chain: an
operation key plus arguments is turned into a concrete argv, then executed in
the Background form.

### Schema

| Shape | Fields |
|-------|--------|
| ResolverRequest | operation key (core operations, or extension-prefixed keys) plus args: src, dst, path, paths, opts, label |
| ResolvedCommand | argv with placeholders expanded, plus the entry's confirm policy |

Entries are declared in Scheme ([SCHEME.md](SCHEME.md)): a key, a **confirm**
policy, optional declared **options** (token + label), and an ordered list of
candidates. A candidate is a kind — uutils, native, native-macos,
native-linux — plus argv elements that are literal strings or the placeholder
symbols src, dst, path, paths, opts. The chain merges embedded defaults, then
extension entries; later entries win. `config.scm` has no resolver form.

**Option extraction.** Besides the declared options, the chain extracts options
from each entry's resolved command's `--help` output (falling back to its `man`
page). Extraction runs once per command on a worker thread started when the
chain is finalized at boot, into a shared memo. `CommandLookup` reads only the
memo: until the extracted options land (or when extraction yields nothing) the
declared options are used.

### Expansion Rules

- src and path expand to the single target's absolute path. dst expands to the
  confirmed text, made absolute against the current directory when relative.
  paths expands the target list directly into argv.
- A single-target placeholder receiving multiple targets is a resolution error.
- With an empty selection, the cursor entry is the single target.
- Open, Edit, and Rename abort with a notification when multiple items are
  selected.

### Failure Behavior

There is no retry UI. Unresolved operation, spawn failure, or a non-zero exit
surface as a Failed result with an error notification. Chain fallback happens
at resolution time only: a candidate whose kind does not match the OS, or whose
executable is not runnable, yields to the next. A non-zero exit after spawn is
final.

### Confirmation

Whether a resolver plan is confirmed through Policy mode is declared on its
entry, never decided by the op's name:

| confirm | Policy is pushed when |
|---------|-----------------------|
| `never` | never |
| `always` | always (e.g. delete) |
| `overwrite` | the request would overwrite an existing path: its `dst` (or `path`), or — for a directory destination — any `dst/basename(src)`, already exists. Evaluated by the kernel's confirmation gate at dispatch time against the real filesystem (the `cp`/`mv -i` condition) |

The embedded defaults declare copy / move / rename as `overwrite`, delete
(`rm`) as `always`, and mkdir / touch / open as `never`.

### Who May Add Entries

Embedded defaults, then extensions. Raw Execute (an arbitrary argv) does not
go through the resolver; it is reachable only from user configuration — a
`bind!` or a `command!` (a user palette command) whose intent is `Execute` — and
from the built-in command builder, where the user edits the command line
(CmdOp).

## State Ownership

| Store | Contents | Mutation |
|-------|----------|----------|
| Settings store | Options, bindings, layouts, commands, display orders — layered (`default` / `config` / `custom` / `session`) Steel values in the session | Store procedures only (`config.scm`, `custom.scm`, preludes, the REPL, `Configure`); the kernel's projections follow via change sets ([SCHEME.md](SCHEME.md)) |
| Mode stack | Mode-defining specs: SelectSpec, InputFormSpec, the Policy pending intent, extension mode state; each frame tagged with its generation | Reducer only, through `ModeStack` push / pop / replace, which assign generations — the stack is not writable field by field |
| AppState | Everything that affects behavior: cwd, the pending listing request, entries, cursor, path selection, the nav trail, the list layout and measured geometry, the filter stack, `FunctionPanelState`, Select and InputForm runtime state, the input capture, the boot-time `env` | Reducer only, **except** the render-measured geometry the kernel copies in after each draw (`list_geom`, the function scroll bounds) |
| UiManager | Render-only caches: function-panel layout and image cells, notice timers, measured geometry | Imperative; never consulted for a behavioral decision |
| Extensions | Their own content and view state (interior mutability) | The owning extension, from its hooks |

If a value influences what an intent does, it lives in AppState or the Mode
stack — never in UiManager. The kernel's gates act by reducing intents; the
kernel writes AppState directly only for the post-draw geometry feedback.
`AppState` exposes read accessors for the mode stack; extensions receive
`&AppState` and never mutate it.

## Module Layout

mod.rs is forbidden. A module with submodules has a facade file named after
the directory, placed next to it. The facade contains only module
declarations, re-exports, and the module's minimal public surface (trait
definitions, small shared types, registration lists); implementations live
inside the directory.

| Module | Facade owns | Directory contents |
|--------|-------------|--------------------|
| core | re-exports | `state` (AppState, FunctionPanelState, PanelSearch, SelectState, InputFormState, ModeStack); `intent`; `mode` (Mode, SelectSpec, OnConfirm, InputFormSpec); `event` (AppEvent, Plan, AsyncResult, PanelView, Content); `extension_value`; `option` (OptionStore); `theme` (Theme, Role); `line_edit` (the one line editor); `rank` (the one local subsequence ranker); `reducer/` — `nav`, `select`, `input_form`, `function`, `result`, with `reducer.rs` as its facade holding `apply_intent` / `apply_result` dispatch |
| app | re-exports | the event loop; the kernel (`dispatch_intent`, gates, plan issuance, broadcasts, extension registration); terminal init/restore (Kitty keyboard flags) and the Suspended form; `cmdop/` — the built-in command builder (`CmdOp`) |
| services | type surfaces | CommandRegistry; ResolverChain (+ option memo); MatcherBackend / SkimMatcher / MatcherConfig; MessageLog; Persister; FileWriter; Monitor |
| ui | re-exports | UiManager (render caches); the compositor (`render`); `top/` (nav, prop, monitor view); `middle/` (file list, select, function panel host + PanelView layout engine); `chrome` (connected borders, titles, scrollbars); `bottom` (help row) |
| extension | the Extension trait, `Host`, `SettingsNode`, and the bundled-extension list — the only place bundled extensions are named | ExtensionRegistry; the dynamic loader; one directory per bundled extension |
| script | re-exports | the resident steel session and the settings store's Scheme side (store procedures, layers, validation, change sets, the `custom.scm` dump); the change-set applier (projections); the s-expression reader; the codec (the only place intent / key / mode strings — and store calls — are parsed or formatted); core data decoders (keymaps, commands, resolvers, options, layouts) and the `config.scm` template; condition AST + evaluator; KeymapRegistry |
| exec, fs, runtime, vfs | — | the external-command executor and `--help`/`man` option extraction; directory reading, the recursive walk, owner / group name resolution; TaskManager; the Vfs router |

`wasdf-sdk` holds the ABI-stable surface shared with dynamic extensions: the
entry-point wrapper, chunked reading, formatting helpers, file signature and
MIME detection, and the `Settings` snapshot.

## Startup Sequence

1. Capture the boot environment (`$HOME`, `$EDITOR`, the config directory) and
   spawn the resident Scheme session (its stdlib loads on its own thread).
2. Read the embedded defaults (keymaps, commands, resolvers, core options,
   layouts) and `custom.scm` with the s-expression reader — both are data.
   `custom.scm` is read this early because two of its values are needed
   before anything else: the extension enable toggles and the session boot
   timeout.
3. Construct the built-in components (CmdOp) and the extensions (bundled, then
   optional in manifest file-name order; an id collision disables the later
   one; an extension disabled in `custom.scm` is not constructed), and
   register them through one path — keymaps, commands, option declarations,
   resolver entries, VFS providers, conditions, settings nodes, preludes.
4. Wait for the session (bounded by `scheme.boot-timeout-ms`), then build the
   settings store in it: load the `default` layer (embedded defaults and every
   extension's declarations), load the preludes, evaluate `config.scm` form by
   form into the `config` layer (writing the template first if it is absent),
   and load `custom.scm` into the `custom` layer ([SCHEME.md](SCHEME.md)). If
   the session is not ready, use the fallback instead: defaults plus the
   literal calls of `custom.scm`, applied natively.
5. Build the projections from the store's effective values: the OptionStore,
   the keymap (the User layer from the store's bindings), the CommandRegistry,
   the layouts, the display orders.
6. Finalize the resolver chain (starting option extraction) and hand every
   extension the `Host` (`Extension::bind`).
7. Report keymap collisions to `@/messages`; issue the initial ReadDir.

After boot every change to the store — a `Configure` from the UI, a REPL
evaluation — returns a change set, which the kernel applies to the projections
at once (rebuilding the User keymap layer with collision detection when
bindings changed); a change to the `custom` layer also wakes the Persister.
