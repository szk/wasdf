# Architecture

wasdf is a TUI filer (Rust / ratatui / crossterm) built around a single intent
pipeline: key → Intent → plan → kernel async → AsyncResult → reducer → render.
This document defines the engine: the component model, the pipeline, the intent
catalog, the async contract, the resolver contract, state ownership, the module
layout, and the startup sequence. User-facing layout, modes, and keymaps are in
[UI.md](UI.md); extensions in [EXTENSION.md](EXTENSION.md); Scheme configuration
in [SCHEME.md](SCHEME.md).

No module interprets a junction point on its own. MVP closure takes priority
over future extensibility. The default failure behavior everywhere is: disable,
notify, continue.

## Component Model

There are exactly two kinds of components. There is no plugin tier and there
are no per-service plugin traits.

| Kind | What it is | Examples |
|------|-----------|----------|
| Kernel service | A concrete struct inside the kernel. One implementation each, configured by data; no registration machinery | KeymapRegistry, CommandRegistry, ResolverChain, the Scheme session |
| Extension | Implements the single Extension trait. Bundled (statically linked) or optional (dynamically loaded) | PreviewExtension, ArchiveExtension (bundled); third-party extensions (optional) |

The only service trait is MatcherBackend (fuzzy ranking; one shipped
implementation, SkimMatcher — the trait is a test seam, not an extension
point). Services are defined by the embedded Scheme defaults and extended
through the Extension trait ([EXTENSION.md](EXTENSION.md)).

## The Intent Pipeline

Every keystroke flows through one ordered path. There are no mode-specific or
extension-specific side channels.

1. KeymapRegistry resolves (mode, focused panel, key) to an Intent — the only
   key-to-Intent path. Scopes match by longest mode-id prefix, then panel
   scope, then layer priority (Core < Extension < User). Dynamic `:when`
   conditions are evaluated natively here ([SCHEME.md](SCHEME.md)). When no
   binding matches (Select mode, or the function-panel search input), the key is
   delivered as a RawKeyEvent.
2. `dispatch_intent` (in the kernel) processes the intent in ordered stages:
   - Policy gate — while a Policy overlay is up, only Confirm/Cancel act
     ([UI.md](UI.md)).
   - Extension dispatch — an extension intent goes to its owning extension's
     `handle_intent`, which returns a list of follow-up intents that are
     re-dispatched (depth-capped). If its data carries the reserved resolver
     key, it is instead a plan the kernel executes (never re-dispatched).
   - Confirmation gate — Delete, and plans whose resolver entry is marked
     destructive, push Policy mode.
   - Reduction — the pure, synchronous reducer (`apply_intent`) applies the
     state transition and returns Effects: plans to spawn, notices to show, and
     follow-up intents to re-dispatch (e.g. an initiator like StartCopy expands
     into a Select push). Side effects leave the pipeline only as one of the
     closed plan kinds.
3. Render.

Async completions return as AsyncResult events. The reducer handles them by
the pair (purpose, payload) only; completion handling never branches per intent.
(The `content` purpose is the one kernel-handled exception — see Async Contract.)

Rules:

- The event loop and `dispatch_intent` contain no per-intent special-casing
  beyond the structural gates above. "Do Z before X" is expressed by the reducer
  returning follow-up intents in Effects.
- The reducer is pure and synchronous. All I/O happens in kernel async tasks.
- Extensions are always called synchronously (`handle_intent`, `accept_content`,
  `render_function`, `on_cursor_changed`) and return data/intents; the kernel owns
  all execution and all state mutation (via the reducer).
- **Cursor-changed is a generic broadcast.** When the cursor target or panel
  visibility changes, the kernel calls every registered extension's
  `on_cursor_changed(&AppState)` and re-dispatches the intents they return — the
  kernel hardcodes no extension. The content provider is just one subscriber: it
  returns `LoadContent{owner,path}` to follow the cursor; other extensions may
  react too (e.g. push their own content). This is the one place a non-key event
  enters the pipeline, and it does so as ordinary re-dispatched intents.

## Intent Catalog

Core intents are a closed set; an extension feature must never add a core
variant. Extension intents travel as data ([EXTENSION.md](EXTENSION.md)).

| Category | Intents | Notes |
|----------|---------|-------|
| Navigation | CursorUp, CursorDown, CursorTop, CursorBottom, CursorLeft, CursorRight, Activate, NavigateTo | Geometric: the four Cursor moves shift the cursor over the file-list geometry (`AppState.list_geom`); the left wall escapes to the parent directory; the Rows right wall acts on the entry (dir → enter, file → cycle the preview), Columns/Grid clamp. Activate (Enter) enters a directory or **opens the file** via the resolver from any cell ([UI.md](UI.md), File-list layouts + Enter semantics) |
| Function cursor | FuncUp, FuncDown, FuncLeft, FuncRight | Operate the function panel from the file panel without moving focus: FuncUp/Down scroll vertically, FuncLeft/Right scroll the content horizontally |
| Selection | ToggleSelect, SelectAll, ClearSelection | Selection is a path set ([UI.md](UI.md)) |
| External commands | RunResolver, Open, Edit, Execute | RunResolver is the generic "build a command line and run it": copy / move / rename / mkdir / touch / delete are all RunResolver carrying a `ResolverRequest` (op + targets + dst/path + opts). Resolved and executed per the Resolver and Async contracts below |
| Initiators | StartCopy, StartMove, StartRename, DeleteSelected, StartEdit | Expanded by the reducer into the operations above (Effects follow-up intents) |
| Mode | PushMode, PopMode | Mode stack transitions |
| Function panel | SetSubLayout, CycleFunctionPanel, HideFunctionPanel, ShowFunctionContent, UpdateFunctionView, LoadContent | View state, not modes ([UI.md](UI.md)). ShowFunctionContent stores an extension's `PanelContent`; UpdateFunctionView stores an extension's opaque `ext_view`; LoadContent issues the generic cursor-follow `Plan::Read` for a named content owner. Generic mechanisms, not per-feature ([EXTENSION.md](EXTENSION.md)) |
| Scroll | ScrollUp, ScrollDown, ScrollTop, ScrollBottom, PageUp, PageDown, SetScroll | Function-panel vertical scroll; SetScroll jumps to an offset (e.g. a search match) |
| Content search | FunctionSearchStart, FunctionSearchSubmit, FunctionSearchNext, FunctionSearchPrev, FunctionSearchCancel, SetSearchMatches, ResetContentView | Less-style search over function-panel content. Submit hands the query to the owning extension's matcher; SetSearchMatches stores the result (match state lives in core so `:when` can gate `n`/`p`) |
| Dialog | Confirm, Cancel | Phase-interpreted in Select; allow / deny in Policy |
| Select | RawKeyEvent | RawKeyEvent delivers unbound printable keys; the reducer applies them to the Select query or the content-search input. Select results navigate through the shared `Cursor*` intents (no result-specific intents): a path-valued Select (file-search) keeps its hits as an `EntryList` and renders + navigates them as the file panel does (grid layouts, 2-D moves, `v`); token-valued Selects move ±1 in their candidate list |
| Panel | FocusNextPanel, FocusPrevPanel, FocusPanel | |
| View | ToggleDotFiles, ToggleLineNumbers, CycleListLayout | CycleListLayout cycles the file-list arrangement Rows → Columns → Grid ([UI.md](UI.md)) |
| Process | KillProcess | Exec frame only |
| Misc | Refresh, Quit, Noop | |
| Extension | Extension (with structured data), UpdateExtensionState | UpdateExtensionState is the single mechanism for extension-mode state updates |

## Async Contract

All I/O and heavy computation runs in kernel async tasks. Handlers and
extensions are called synchronously and only describe work; the kernel executes
it. The round trip is fixed: Intent → Plan → kernel task → AsyncResult → reducer.

### Sync / Async Split

| Work | Mode | Reason |
|------|------|--------|
| Reducer, keymap resolution, mode transitions, extension `handle_intent`, content decode + render hook | sync | immediate response; purity (decode runs on the main thread — see Read below) |
| Directory reads, content byte reads, matcher ranking, external commands, Scheme session queries | async | I/O or heavy compute |

### Plans — a Closed Set

Plans are the only way side effects leave the pipeline. Extensions never define
their own plan kinds: extension file operations ride ResolveAndRun, and an
extension's function-panel content is loaded by the generic Read plan.

| Plan | Representative intents | purpose | Execution | payload |
|------|------------------------|---------|-----------|---------|
| ReadDir | CursorRight on a directory, CursorLeft, NavigateTo, Refresh, ToggleDotFiles | refresh | async directory read | Entries |
| Search | the file-search walk; the kernel applies matcher ranking in a blocking task | search | walk + rank | Entries (ranked) |
| Read | cursor-follow content load (any function-panel content provider) | content | async read of a path: one **bounded byte chunk from an offset** (the owner pages further chunks by re-issuing `LoadContent` with a new offset), or a directory's entries | Read (owner + path + offset; ReadResult::Bytes{offset,bytes,eof} / Dir) |
| ResolveAndRun | RunResolver (copy/move/rename/mkdir/touch/delete), Open, extension plans | resolver | Background: detached process, no output capture, completion notification only | OpDone |
| Execute | Execute | execute | Captured: stdout streamed line-by-line into the Exec frame, stderr appended at exit | ExecOutput |
| Suspend | Edit (the user's editor) | execute | Suspended: leave the alternate screen, restore the cooked terminal, pause the event loop, run the child in the foreground; on exit restore the TUI and issue Refresh | OpDone |
| EvalScheme | dynamic arbitrary `:when` conditions, expression-valued intent arguments | scheme | async query to the resident Scheme session | SchemeValue |

Background, Captured, and Suspended are the only three external-process
execution forms. Only Execute shows output in the function panel; resolver
commands and Open report completion as a notification only.

The **Read** plan is content-agnostic: the kernel reads one bounded byte chunk
from the requested offset (files) or the entries (directories) off the UI thread
and returns them. It does **not** decode or assemble them — the owning
extension's `accept_content` decodes (MIME, syntax highlight, hexdump, …) on the
main thread and folds the chunk into its own growing window, which its render
hook later draws. **Chunk paging lives in the extension**, not in core: the
extension pages a large file by re-issuing `LoadContent` with the next offset
(driven by the cursor-follow / scroll signature, which includes the scroll
position) and stops at `eof` ([EXTENSION.md](EXTENSION.md)).

### AsyncResult — One Schema

Every completion carries the same fields. Do not add fields.

| Field | Content |
|-------|---------|
| request_id | unique per spawn |
| purpose | one of content, search, resolver, execute, scheme, refresh — a fixed namespace, never extended |
| mode_generation | the mode instance id current at spawn time (the reducer assigns a monotonically increasing id to every pushed mode) |
| status | Ok, Cancelled, Failed, StaleDiscarded |
| payload | Entries, Read (owner + path + ReadResult), OpDone, ExecOutput, SchemeValue |

The reducer consumes results by the pair (purpose, payload). Completion handling
never branches per intent. The one exception is `content`: a Read completion is
not reduced — the kernel hands its `ReadResult` to the owning extension's
`accept_content` (which can't be called from the pure reducer) and resets the
view. Everything else flows through the reducer.

Execute is the one streaming exception: it emits several `execute` results
under one request_id — one per stdout line (status Ok, `finished` false) and a
final one on exit carrying the exit code — so the Exec frame grows as output
arrives. KillProcess cancels the child. Every other plan returns exactly once.

### Rules

- spawn_unique(purpose, …): at most one live task per purpose; a new spawn
  cancels the previous task with the same purpose automatically. Handlers
  never hold task ids.
- Staleness is keyed primarily on mode_generation: a mismatch yields
  StaleDiscarded and the result is dropped. Content reads are deduped at spawn on
  the cursor path (`content_for`), and the unique `content` purpose cancels the
  previous in-flight read so a result for a path the cursor has left is dropped.
- Notification: only Failed produces a user-visible error notification.
  Cancelled and StaleDiscarded are silent (debug log only).
- A late completion for a cancelled request id is ignored.

## Resolver Contract

All external command execution resolves through the resolver chain: an
operation key plus arguments is turned into a concrete argv, then executed by
the kernel in the Background form. The resolver is the center of the MVP.

### Schema

Two shapes suffice; no dedicated module beyond the chain itself.

| Shape | Fields |
|-------|--------|
| ResolverRequest | operation key (core operations, or extension-prefixed keys such as an archive pack entry) plus args: src, dst, path, paths, opts |
| ResolvedCommand | argv with placeholders expanded, plus the destructive flag taken from the entry |

Entries are declared in Scheme ([SCHEME.md](SCHEME.md)): each entry has a key,
an optional destructive flag, an optional list of selectable **options** (token +
label, shown as checkboxes in the command Select — these are the *declared*
options; the kernel also parses options from the resolved command's `--help`
output, falling back to its `man` page, memoised per command), and an ordered
list of candidates. A
candidate is a kind — uutils, native, native-macos, native-linux — plus argv
elements that are either literal strings or the placeholder symbols src, dst,
path, paths, **opts** (the chosen option tokens, spliced in order). The chain
merges embedded defaults, extension entries, and user entries; later entries take
precedence.

### Operations

Copy / move / rename / mkdir / touch / delete are all `RunResolver` carrying a
`ResolverRequest`; the Select flow fills its fields (target + options + dst/path).

| Operation | Targets | Confirmed input | Placeholders | Notes |
|-----------|---------|-----------------|--------------|-------|
| Copy, Move | multiple allowed | one command Select: option checkboxes (Space) + InputOnly (destination) | opts, paths, dst | refresh and cursor correction on completion |
| Delete | multiple allowed | none | paths | Policy confirmation (destructive) |
| Rename | single only | InputOnly (new name) | src, dst | string confirmation is the purpose |
| Open | single only | none | path | Background form |
| Edit | single only | none | path | Suspended form; runs the user's editor, outside the resolver |
| Mkdir, Touch | no target | InputOnly (text) | path | entered via the path-input Select instance |

### Expansion Rules

- src and path expand to the single target's absolute path. dst expands to the
  confirmed text, made absolute against the current directory when relative.
  paths expands the target list directly into argv.
- A single-target placeholder (src, dst, path) receiving multiple targets is a
  resolution error.
- With an empty selection, the cursor entry is the single target. This is not
  an error.
- Open, Edit, and Rename abort with a notification when multiple items are
  selected.

### Failure Behavior

There is no retry UI. Failures are notified once and the operation ends.

- Unresolved operation, spawn failure, or a non-zero exit all surface as a
  Failed result with an error notification.
- Chain fallback happens at resolution time only: a candidate whose kind does
  not match the OS, or whose executable is not runnable, yields to the next
  candidate. A non-zero exit after spawn is a final failure — the chain is not
  re-entered. If no candidate resolves, the operation is unresolved.

### Destructive Confirmation

Policy mode confirms exactly two things before execution: the Delete operation,
and any resolver plan whose entry carries the destructive flag. Destructiveness
is declared on the resolver entry, never by the intent that requests it.

### Who May Add Entries

Embedded defaults, extensions, and user configuration may all contribute
entries; the chain is merged in that order with later entries winning. Raw
Execute (arbitrary command strings) does not go through the resolver; it is
reachable only from user configuration and the command palette, and its output
is captured into the Exec frame.

## State Ownership

| Store | Contents | Mutation |
|-------|----------|----------|
| Mode stack | Mode-defining specs: SelectSpec (including static candidates, pending-operation sources, initial query), the Policy pending intent, extension mode state | Immutable per mode instance; replaced via mode transitions and UpdateExtensionState (reducer) |
| AppState | Everything that affects behavior: cursor, path-based selection, directory entries, the file-list layout (`list_layout`) and last-rendered geometry (`list_geom`), FunctionPanelState (visibility, sublayout, ratio, vertical/horizontal scroll, line numbers, the active content owner id, an opaque per-extension `ext_view`, and `PanelSearch` — the content-search input + match list), Select runtime state (query, phase, ranked results, marks) | Reducer only, **except** the render-measured geometry the kernel refreshes after each draw — `function.scroll`/`hscroll` clamps and `list_geom` — so the pure reducer can scroll/move without seeing the viewport |
| UiManager | Render-only caches: image protocol state, notification timers, computed layout, and the measured scroll/list geometry it hands back to the kernel post-draw | Imperative; never consulted by the reducer or handlers for decisions |

If a value influences what an intent does, it lives in AppState or the Mode
stack — never in UiManager. Render-derived geometry that the reducer needs
(`list_geom`, the function-scroll bounds) therefore lives in AppState; UiManager
only *measures* it and the kernel copies it in after the frame.

## Module Layout

File placement convention: mod.rs is forbidden (Rust 2018 style). A module
with submodules has a facade file named after the directory, placed next to
it. The facade contains only module declarations, re-exports, and the module's
minimal public surface (registration functions, trait definitions, small
shared types); implementations live inside the directory.

| Module | Facade owns | Directory contents |
|--------|-------------|--------------------|
| core | re-exports | AppState (FunctionPanelState, PanelSearch, SelectState, path-based selection); Intent; reducer (the state machine — consumes (purpose, payload), query/search editing, phase switching); Mode, SelectSpec, OnConfirm (Resolve carries the ResolverRequest template + ResolveFill); ExtensionValue; AppEvent, Plan, AsyncResult; PanelContent, StyleRun, ReadResult |
| app | re-exports | The event loop; the kernel and `dispatch_intent` (extension dispatch, confirmation gate, plan issuance + reduction, the cursor-changed broadcast to extension subscribers, the option store); terminal init/restore and the Suspended execution form |
| services | resolver and matcher type surfaces | CommandRegistry; confirmation spec; ResolverChain; MatcherBackend with SkimMatcher |
| ui | re-exports | UiManager (render caches only); the frame compositor (`render`) and its top / middle / bottom bands; the top panel (nav and prop children); the main area (file list, select input + candidates, the function panel with its command summary); the generic content blitter (`content` — turns `PanelContent` + search/scroll into styled lines, shared by all content extensions); panel chrome (connected borders, titles, scrollbars); the help row |
| extension | the bundled-extension list — the only place bundled extensions are named | the Extension trait (`provides_function_content`, `accept_content`, `render_function`, `scheme_source`, `keymaps`, `handle_intent`) with `FunctionRenderCtx`/`FunctionDraw`; ExtensionRegistry (loading rules, disable, the active content provider); the dynamic loader; the preview extension (decode / highlight / search submodules) and archive |
| script | re-exports | The resident steel (`steel-core`) session (dedicated thread + mpsc channel; Engine is `!Send`/`!Sync`); the single string codec and a small s-expression reader (the only place intent/key/mode/datum strings are parsed or formatted); the config decoders (keymaps, commands, resolvers ← Scheme); condition AST and native predicates; KeymapRegistry |
| exec, fs, runtime | — | the external-command executor (Background/Captured forms — the resolver builds the argv, `exec` runs it) plus option extraction from a command's `--help`/`man`; async directory reading and the recursive walk; TaskManager (worker threads, one live task per unique purpose, streaming Execute, the generic Read) |

A separate wasdf-sdk crate holds the ABI-stable types shared with dynamically
loaded extensions: chunked reading, formatting helpers, file signature
detection, MIME detection.

## Startup Sequence

1. Kernel boot.
2. Spawn the resident Scheme session (steel engine on a dedicated thread) and
   wait briefly for it to be ready (stdlib load). Evaluate the embedded Scheme
   defaults (keymaps, commands, resolvers) through the session and decode them
   into the registries via the codec; if the session is not ready or evaluation
   fails, fall back to the native embedded defaults ([SCHEME.md](SCHEME.md)).
3. Register bundled extensions.
4. Load optional extensions per the loading rules in [EXTENSION.md](EXTENSION.md).
5. Load user configuration (User layer).
6. Merge: keymaps by layer with collision detection across all layers,
   commands by name (last wins), resolver entries appended to the chain
   (later wins), layouts by id.
