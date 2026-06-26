# UI, Modes & Keymap

The UI is a panel-based layout system. Rust renders; Scheme declares layouts
and panel placement ([SCHEME.md](SCHEME.md)). The UI layer holds render-only
caches; anything that affects behavior lives in AppState or the Mode stack and
is mutated through the reducer ([ARCHITECTURE.md](ARCHITECTURE.md)). This
document covers the layouts and panels, the mode stack, and the keymap.

## Layouts

Core built-in layouts are exactly two: file and select. Policy renders as an
overlay on whatever layout is active. Extension custom modes may use a layout
hint (Select, File, or TwoPanel) or define additional layouts of their own;
those do not count as core built-ins.

### The file layout

| Row | Content |
|-----|---------|
| top | the top panel, holding nav and prop side by side; height per vertical size class below |
| main | the file panel; while the function panel is visible, file at ratio-left and function at ratio-right, otherwise file takes the full width |
| bottom | help |

The function panel cycles hidden → 2:1 → 1:1 → 1:2 → hidden via
CycleFunctionPanel (the comma key in File mode).

### File-list layouts

The file panel arranges its entries one of three ways, cycled by `v`
(CycleListLayout, Rows → Columns → Grid → Rows; the choice is `AppState`,
persisted for the session):

| Layout | Arrangement |
|--------|-------------|
| Rows | one entry per line, full width (the default) |
| Columns | `ls -C`: column-major, the most columns that fit, each sized to its own entries (per-column variable width) |
| Grid | uniform square-ish cells (rows ≈ cols), column-major |

All three fill **column-major** (down a column, then across). `w`/`a`/`s`/`d`
move the cursor up / left / down / right by what is drawn, so where the cursor
lands is predictable. Edge rules: the **left wall** escapes to the parent
directory (all layouts); the **right wall** acts on the entry (enter / show content) in **Rows
only** — in Columns/Grid it clamps, and `Enter` (Activate) descends from any
cell. The vertical unit is a visual row: the file scrollbar and scroll offset
count rows, not entries.

The reducer is geometry-free: the renderer is the single source of the
`(cols, rows)` geometry and the per-column widths, and the kernel copies the
measured `(cols, rows)` into `AppState.list_geom` after each draw so the pure
reducer's 2-D cursor movement matches the screen (the same render→behavior seam
as the function-panel scroll clamp; see [ARCHITECTURE.md](ARCHITECTURE.md)).

**Acting on the entry (`d` vs `Enter`).** `d` = *look*, `Enter` = *do*:

- In **Rows**, `d` on a directory enters it; `d` on a file **cycles the content view**
  — it makes the function panel visible in the Content sublayout and cycles its
  width (`2:1 → 1:1 → 1:2 → 2:1`), never hiding, keeping the shared ratio on first
  open, and leaving focus on the file panel. (This differs from `,`
  CycleFunctionPanel, which hides and resets the ratio.) In Columns/Grid the right
  wall just clamps; show its content there with `,`.
- `Enter` (Activate) acts on the entry from any cell/layout: enter the directory,
  or open the file via the resolver. It does not show content.

**Content search focus.** While a file’s content is shown, `/` opens the search
input and **borrows focus** to the function panel so you can type; pressing
`Enter` submits (jumps to the first match) and **releases focus** back to the file
list, where `n`/`p` step matches. `Esc` cancels and releases focus. So focus only
lives in the function panel while a query is being typed.

### The select layout

One layout serves every picker.

| Row | Content |
|-----|---------|
| top | the top panel (nav and prop) |
| main | a left column at ratio-left — the select input (collapsed when the instance has no input) above the candidate list — and the function panel at ratio-right while the spec sets a function hint |
| bottom | help |

### Screen chrome (the connected frame)

The screen is one connected frame: adjacent panels share the line between them.
nav and prop share the top panel's frame; the file and function panels meet on a
single shared vertical line; cycling the ratio (the comma key) slides that line
left or right. The three ratio variants:

```
┌─// wasdf ver. 0.1 //─────────────────┬[1.90, 2.26, 2.73]─[26-05-28 20:25:47]─┐
│/                                     │                                       │
│├─ Users                              │                                       │
│  ├─ szk                              │                                       │
│    ├─ github                         │                                       │
└─[ /Users/szk/github/wasdf ]──────────┼─[ README.md ]─────────────────────────┤   1:1
  📁 crates                            │  1 # wasdf                            │
  📁 doc                               │                                       │
  📄 Cargo.toml 449 B                  │                                       │
 j/↓  scroll down   k/↑  scroll up   ,  cycle ratio   Enter  open   Esc  back
```

The file panel's title shows the full current-directory path. The function
panel's title shows the name of the file being shown. The shared vertical line's
junction with the top frame slides as the ratio cycles (2:1 / 1:1 / 1:2); the
bottom-left junction tucks under the file panel rather than drawing a separate
box.

## Borders and Scrollbars

Borders are single lines, joined with box-drawing junctions (`┬` `┴` `├` `┤`
`┼`), never nested boxes that double the border between neighbours.

The right border of each panel is a vertical ratatui `Scrollbar` drawn in place
of the border line, indicating the panel's visible region. The thumb's **length**
is the ratio of the visible viewport height to the total content height, and its
**position** is the scroll offset — the top of the visible window — not the
cursor or selection index. Only the track and thumb are rendered; the begin/end
arrow symbols are disabled (no `▲` / `▼`). Where a shared vertical line
separates two panels, that line carries the scrollbar of the panel on its left.
A panel whose content fits its area shows a full-height thumb, equivalent to a
plain border line. The scrollbar occupies the panel's existing right-border
column and never consumes interior width.

A panel's top border may carry a left-aligned title (`[ title ]`, or an
unbracketed banner) and a right-aligned info string (e.g. the prop panel's load
and clock readout). Both are drawn over the border and flow around junction
cells, so the connected frame and its junctions stay intact under long text.

### Cursor rendering

One convention across every panel and form: the cursor at the **focused** location
is **reverse video**; a cursor elsewhere is **underlined**. So the file-list cursor
reverses while the file panel is focused and underlines otherwise; the Select
candidate list reverses in the Navigate phase (focused) and underlines in the
Input phase; and an input form (the Select `search`/`Argument` field, the `/`
content search) draws a reverse-video block at the **real caret position** while
focused, an underlined block when focus is elsewhere.

## Panels

Panels nest: the **top** panel is a container whose children are **nav** and
**prop**, side by side.

| Panel | Description | Provider |
|-------|-------------|----------|
| top | Container for the top row; holds the nav and prop panels side by side | kernel UI |
| nav | Child of top. A macOS Finder-style column (Miller) directory navigator: the path from root to the current directory runs along the panel's horizontal axis, each level's sibling directories stack vertically in its column (the path element centered, `─` linking adjacent levels), the current directory is highlighted, and columns are right-aligned on overflow so it stays visible. Its title is the app banner `// wasdf ver.X //` (unbracketed); the full current-directory path is on the file panel's title | kernel UI |
| prop | Child of top. The cursor entry's properties — PATH, SIZE (human + bytes), PERM (permissions, uid, gid), and the C/M/A times — each truncated to width. Its top border carries a live readout, right-aligned: the system load averages and the current local time | kernel UI |
| file | The file list. Its title shows the full current-directory path | kernel UI |
| function | Renderer container (see the frame table below) | kernel UI + extensions |
| help | Key hints, drawn in reverse video (negative) | kernel UI |
| select-input | Select-mode input field | kernel UI |
| select-candidates | Select-mode candidate list | kernel UI |

The Panel trait consists of an id, a visibility test against the current
context, a render method, and a focusability flag. Panels have no event
handling: every key decision goes through the KeymapRegistry (panel-scoped
bindings plus RawKeyEvent). A panel that needs key behavior ships keymap
definitions; there is no second input path.

Panels and renderers are stateless. They read a context bundle containing the
read-only AppState, the render caches, and the focused panel id.

## The Function Panel

### Visibility and sublayout

| Rule | Content |
|------|---------|
| ratio | FunctionPanelState.ratio is always kept and shared by File and Select (cycling happens only via the comma key in File mode) |
| Visibility in File | FunctionPanelState.visible |
| Visibility in Select | whether the active SelectSpec sets a function hint. The File-side visible flag is never consulted in Select |
| sublayout | The type of the display frame, not a renderer kind. Exactly two: Content (the viewing frame) and Exec |
| Content frame | hosts the active content extension's content, the command summary, and the file-search directory listing |
| Exec frame | hosts only the streamed output of Execute (line-by-line) |

In File mode, while the function panel is visible, the content frame shows the
content of the file under the cursor and follows the cursor automatically — no
key press is required (the entry's metadata lives in the prop panel, not here).
The kernel reads the cursor path (the Read plan) and hands the bytes/entries to
the content provider's `accept_content`, which decodes them (MIME dispatch, off
the kernel's hands); the provider's `render_function` then draws them. There is
no Scheme round trip at render time. Loading a different file resets the scroll
positions and any active search (`ResetContentView`, dispatched when new content
arrives).

### Renderers

The content frame draws one of these. Extension content (whether a bundled
extension's `render_function` output or a dynamic extension's pushed
`ShowFunctionContent`) is always `PanelContent` (styled `Lines` or an `Image`),
painted by the kernel's generic blitter (`ui/content`) with the shared
scroll / search-highlight / line-number handling. The extension produces the
content; core paints it. The division of labor (thin core, fat extension) is
specified in [EXTENSION.md](EXTENSION.md).

| Renderer | Content | Provider | Frame |
|----------|---------|----------|-------|
| extension content | text (syntax-highlighted via syntect), hex, directory listing, or image (Chafa truecolor symbol cells) — produced by the active content extension and painted by `ui/content` | extension + kernel blitter | Content |
| command summary | the resolver op, targets, chosen options, and destination, resolved synchronously at render time | kernel UI | Content |
| exec | captured Execute output | kernel UI | Exec |

When no content extension is registered the function panel shows a placeholder
(file metadata lives in the prop panel).

## Vertical Responsiveness

Two size classes; the class exposes its top-row and content-row counts.

| Class | Terminal height | Top area | Content rows |
|-------|-----------------|----------|--------------|
| Large | at least 25 rows | 7 rows (5 content + 2 border) | 5 |
| Compact | under 25 rows | 5 rows (3 content + 2 border) | 3 |

## UiManager

Owns the layout engine, the panel registry, and the render-only caches (image
protocol state, notifications, computed layout). Its surface is: render from
the read-only AppState, recompute the layout on mode change, and register
panels. No field of UiManager is ever read by the reducer or a handler to make
a behavioral decision.

---

# Modes

A Mode determines how key input is interpreted and which layout is shown.
Modes form a stack; push enters, pop returns.

## Mode Catalog

| Mode | Carried data | Description |
|------|--------------|-------------|
| File | — | File list. The function panel belongs to File mode as view state, not as a mode |
| Select | SelectSpec | The one generic picker for every modal UI. Always pushed by a parent mode handler or extension |
| Policy | pending intent | Confirmation dialog titled **Confirm**, its key hints (`y / Enter : confirm`, `n / Esc : cancel`) drawn inside the box. y or Enter allows and resumes the pending intent; n or Esc denies. No selection state |
| Extension | extension id, mode id, state | Extension-defined UIs that are genuinely not pickers |

Mode ids are colon-separated strings (file; select:file-search; policy;
extension modes as extension-id:mode-id). Keymap scopes and handler
registration match by longest id prefix on segment boundaries.

## File Mode and the Function Panel

FunctionPanelState lives in AppState and is reducer-managed:

| Field | Values | Meaning |
|-------|--------|---------|
| visible | bool | Whether the panel shows in File mode (Select uses its own rule above) |
| sublayout | Content, Exec | The active display frame |
| ratio | 2:1, 1:1, 1:2 | file : function split; always kept and shared across modes |
| scroll, hscroll, show_line_numbers | — | Generic content view geometry (vertical scroll is shared with the Exec frame) |
| content_owner | extension id | The extension that owns the current content; the kernel calls its render hook and routes its reads |
| ext_view | ExtensionValue | Opaque per-extension view state, written via UpdateFunctionView |
| search | PanelSearch | The content-search input (query, caret, active) and the match list + current index |

The decoded *content itself* is not in AppState — it lives inside the owning
extension ([EXTENSION.md](EXTENSION.md)).

Function panel intents:

| Intent | Behavior |
|--------|----------|
| SetSubLayout | Show the panel with the given frame |
| CycleFunctionPanel | hidden → 2:1 → 1:1 → 1:2 → hidden (File mode only) |
| HideFunctionPanel | Hide and refocus the file panel |
| ShowFunctionContent / UpdateFunctionView | Store an extension's pushed content / opaque view state (generic) |
| FunctionSearch{Start,Submit,Next,Prev,Cancel}, SetSearchMatches, SetScroll, ResetContentView | Less-style content search + view reset (see [ARCHITECTURE.md](ARCHITECTURE.md) catalog) |

In the Rows layout, `d` (CursorRight) on a file cycles the content view (shows the
function panel in the Content frame and cycles its width; see "File-list layouts"); the
kernel then loads the file's content. While the function panel is visible in File
mode, the content follows the cursor automatically — no key press is required. The
cursor entry's metadata (name, size, permissions, modified, created, accessed) is
shown in the prop panel.

## Select Mode

Nearly every modal UI is "pick from a list, optionally filtered by typed
input". Select implements that once, as data. A parent constructs the full
SelectSpec, including what the function panel shows, because only the caller
knows what the selection is for.

### SelectSpec

| Field | Meaning |
|-------|---------|
| id | Colon-separated instance id; the full mode id prefixes it with the select segment |
| source | FileWalk, Commands, PathCompletion, or Static (caller-supplied items, carried inside the spec) |
| input | Fuzzy, Path, or None |
| on_confirm | Navigate, RunCommand, Resolve, or Emit. Resolve carries a `ResolverRequest` template + a `fill` (Dst / Path). One Select (Static option candidates + Path input) is the whole "command op": the input fills dst/name, the Space-marked candidates fill opts, then `RunResolver` runs it |
| initial_query | Optional initial query text (Rename prefills the current file name) |
| function_hint | Optional renderer for the function panel; absent hides the panel |

### Built-in instances

| Instance | source | input | on_confirm | function_hint | Entry |
|----------|--------|-------|------------|---------------|-------|
| file-search | FileWalk | Fuzzy | Navigate | dir-listing | f. Hits are kept as an `EntryList` (not flattened to candidates) and rendered + navigated as the file panel's entry grid — same layouts, cursor, 2-D moves, `v` |
| command-palette | Commands | Fuzzy | RunCommand | none | x |
| command | Static (option checkboxes from the command's `--help`/`man`) | Path | Resolve (fill Dst / Path) | command:summary | c, m, R (R prefills initial_query); palette Mkdir/Touch. The **Argument** input is the destination/name (free text, never filters); the **Options** list is the command's options, toggled with Space; the function panel shows the live **Command** line — the **actual external command** resolved from the chain (e.g. `cp -R …`, `mv …`), not the op key, with the marked options and target spliced in |

### Phases

Select has two phases so that typing and list navigation never conflict:

- Input phase (only when the instance has an input): printable keys edit the
  query (readline editing, applied by the reducer); Up/Down and Ctrl-n/Ctrl-p
  move the selection; Enter switches to Navigate — uniformly, in every
  instance; Esc cancels the whole Select.
- Navigate phase: w and s move the selection; Space toggles a mark (where
  marks are valid — see the confirm contract); Enter confirms; Esc or q
  cancels. Any unbound printable key returns to the Input phase and applies
  the character, resuming filtering. Instances without an input live entirely
  in the Navigate phase.

The phase is part of SelectState in AppState, so `:when` conditions and the
handler can branch on it.

### Structural rules

- A push of Select while a Select instance is topmost replaces it. Select
  never nests. Multi-step flows are expressed by Emit re-injecting the next
  intent, not by stacked pickers. The core copy/move/rename/mkdir/touch flow is a
  **single** command Select — the destination input plus the option checkboxes on
  one screen.
- The core command flow uses `OnConfirm::Resolve` carrying a `ResolverRequest`
  template that the pickers fill (target + options + dst/path); there is no
  per-operation intent. Extensions use Emit and carry the resolver key in their
  intent data instead.
- Spec data (including static candidates and the resolver template) is immutable
  on the mode stack; runtime state (query, phase, ranked results, marks) lives in
  AppState and is mutated only by the reducer.

## The Confirm Contract

Confirm produces exactly one of three result shapes. Whether a value came from
a candidate or from free input is never distinguished by type.

| Shape | Produced when | Content |
|-------|---------------|---------|
| Single (item) | Enter with zero marks (input Fuzzy or None) | the current candidate |
| Many (items) | Enter with one or more marks | all marked items |
| InputOnly (text) | any instance with Path input | the confirmed text: the current candidate applied if the list is non-empty, the text as typed otherwise |

| on_confirm | Consumes | Action |
|------------|----------|--------|
| Navigate | Single | pop, then navigate to the item's path |
| RunCommand | Single | pop, then re-inject the chosen command's registered intent |
| Resolve | InputOnly (Dst/Path) | pop, fill dst/name from the typed input and `opts` from the marked option candidates, then run `RunResolver(template)` |
| Emit | any shape | pop, then insert the shape under the reserved item key and re-inject the extension intent template |

Rules:

- Marks (Space, in Navigate phase) toggle candidates: for Emit instances the
  confirm shape becomes Many; for the command Select they toggle the option
  checkboxes, read at confirm into `opts` (even though its input is Path). Elsewhere
  Space is inert.
- Path-input instances always confirm as InputOnly. Rename is string
  confirmation, not candidate selection.
- Do not extend on_confirm; new confirm behavior is absorbed by the consumers
  of the three shapes.

## Policy Mode

The dialog renders the pending operation's summary as an overlay. y or Enter
allows: the pending intent resumes past the confirmation gate. n or Esc
denies and pops. The mode carries only the pending intent; there is no
toggleable selection state. Policy is pushed whenever a `RunResolver` (or an
extension resolver-key plan) targets an op whose entry is marked **destructive**
(e.g. delete) ([ARCHITECTURE.md](ARCHITECTURE.md)).

## Selection and Cursor Rules

- The selection is a set of paths, never of indices.
- After any reload (refresh, dotfiles toggle, post-operation reload), paths no
  longer present in the listing are dropped from the selection.
- If the cursor's entry disappears, move to the nearest surviving neighbor;
  if none, the first entry; if the listing is empty, no cursor.
- ToggleSelect does not advance the cursor. ClearSelection clears everything.
- Copy, Move, and Delete completion issues a refresh and applies the cursor
  correction above.
- Open, Edit, and Rename accept a single target only; with multiple items
  selected they notify and abort ([ARCHITECTURE.md](ARCHITECTURE.md)).

## Where mode behavior lives

There is no per-mode handler trait or registry. Mode-dependent behavior is
expressed in two places:

- The **reducer** (`apply_intent`) is the state machine. It branches on the
  current mode where needed (e.g. ToggleSelect marks a candidate in Select but a
  path in File; Confirm switches phase or confirms; initiators expand into
  follow-up intents). It is pure and returns Effects.
- The **kernel** (`dispatch_intent`) applies the structural gates (Policy,
  extension dispatch, the destructive→Policy confirmation) and broadcasts the
  **cursor-changed event** — when the cursor target or panel visibility changes it
  calls every extension's `on_cursor_changed` and re-dispatches the intents they
  return; the content provider returns `LoadContent` to follow the cursor
  ([ARCHITECTURE.md](ARCHITECTURE.md), [EXTENSION.md](EXTENSION.md)). Async
  completions are consumed by `(purpose, payload)`, with content reads routed to
  the owning extension.

Mode scoping for *keys* is handled by the KeymapRegistry (longest mode-id
prefix, then panel, then layer); see below.

---

# Keymap

The keymap service maps key events to intents. Home-position single keys
(WASD, Space, Enter) are preferred for the most frequent operations. Enter and
y mean OK; Esc and n mean cancel.

The KeymapRegistry is a kernel service, not a plugin. Resolution of
(mode, focused panel, key) is the only key-to-Intent path; core defaults,
extension keymaps, extension entry bindings into core modes, and user bindings
all live in the same layered registry, so layer priority and collision
detection apply uniformly. Scope specificity (longest mode-id prefix, then
panel scope) is applied before layer priority.

Default bindings exist only as the embedded keymap source, loaded at startup
into the Scheme session. It is also written out as the user's keybindings
template with every line commented out, so the User layer contains only lines
the user actively uncommented.

## Enter Semantics

The meaning of Enter is resolved per focused panel.

| Focused panel | Situation | Enter means |
|---------------|-----------|-------------|
| file | directory under the cursor | Activate: enter the directory |
| file | file under the cursor | Activate: open the file via the resolver (single target). Viewing the content is `d` / `,`, not Enter ("d = look, Enter = do") |
| function | Content frame | Open (via the resolver, single target); while the search input is open, instead submits the search |
| function | Exec frame | back to the Content frame. Not a process re-run |
| select panels | — | Confirm: Input phase switches to Navigate; Navigate confirms |
| Policy overlay | — | allow |

On the file panel, Enter acts on the entry: it enters a directory, or opens a
file through the resolver (viewing content is `d` / `,`). It is never a *raw* Execute
— external opening goes through the resolver's `open` op.

## File Mode (file panel focused)

| Key | Intent | Description |
|-----|--------|-------------|
| w | CursorUp | move the cursor up one cell (clamp at the column top) |
| s | CursorDown | move the cursor down one cell (clamp at the column bottom) |
| a | CursorLeft | move the cursor one column left; at the leftmost column, parent directory |
| d | CursorRight | move the cursor one column right; at the **Rows** right wall, dir → enter, file → cycle the content view (Columns/Grid clamp) |
| Enter | Activate | enter the directory, or open the file via the resolver (any cell, any layout) |
| v | CycleListLayout | cycle the file-list layout: Rows → Columns → Grid → Rows |
| k, Up | FuncUp | function cursor up (scroll the content up) |
| j, Down | FuncDown | function cursor down (scroll the content down) |
| h, Left | FuncLeft | scroll the content left |
| l, Right | FuncRight | scroll the content right |
| Space | ToggleSelect | toggle selection; the cursor does not advance |
| f | push Select file-search | file search |
| x | push Select command-palette | command palette |
| W | CursorTop | first entry |
| S | CursorBottom | last entry |
| Ctrl-a | SelectAll | select all |
| c | StartCopy | begin copy (one command Select: destination input + option checkboxes) |
| m | StartMove | begin move (one command Select: destination input + option checkboxes) |
| R | StartRename | begin rename (path input prefilled with the current name) |
| e | StartEdit | edit in the user's editor (Suspended form) |
| , | CycleFunctionPanel | hidden → 2:1 → 1:1 → 1:2 → hidden |
| . | ToggleDotFiles | toggle hidden files |
| Esc | ClearSelection | clear selection |
| r | Refresh | reload |
| Tab, Shift-Tab | FocusNextPanel, FocusPrevPanel | panel focus |
| q, Ctrl-c | Quit | quit |

Extension entry keys merge into this table at the Extension layer and are
user-overridable. Example: the archive extension binds p to start-pack when a
selection exists, and P to start-unpack when the cursor entry is an archive.

## Function Panel (panel-scoped bindings, function panel focused)

Core bindings (embedded defaults): vertical scroll, line numbers, open/back,
kill, cycle, hide must work with zero extensions, so they live in core (vertical
scroll is also used by the Exec frame).

| Key | Intent | Condition | Description |
|-----|--------|-----------|-------------|
| j, Down; k, Up | ScrollDown, ScrollUp | | vertical scroll |
| g, G | ScrollTop, ScrollBottom | | jump |
| n | ToggleLineNumbers | content | toggle line numbers |
| Enter | Open | content | open externally via the resolver |
| Enter | SetSubLayout Content | exec | back to content / info |
| Ctrl-c | KillProcess | exec | kill the process; the panel stays visible |
| , | CycleFunctionPanel | | cycle ratio, hide at the end |
| Esc, q | HideFunctionPanel | | hide and refocus the file panel |

The content extension contributes the text/hex interaction bindings at the
**Extension layer**, declared in its `scheme_source` (the `:when` predicates
`function-searching` / `function-has-matches` are registered by the extension).
By layer priority these override the core Enter/Esc/n bindings while their
condition holds:

| Key | Intent | Condition | Description |
|-----|--------|-----------|-------------|
| h, Left; l, Right | FuncLeft, FuncRight | | horizontal scroll of the text/hex content |
| / | FunctionSearchStart | content | open the less-style search input |
| Enter | FunctionSearchSubmit | search input open | run the search (hands the query to the owning extension), jump to the first match |
| Esc | FunctionSearchCancel | search input open | close the search input |
| n | FunctionSearchNext | search has matches | next match |
| p | FunctionSearchPrev | search has matches | previous match |

The same extension also binds the search into the **file-panel** scope (so a
content can be searched without focusing it): `/` (`:when (and function-visible
sublayout-content)`) starts the search and borrows focus to the function panel for
typing; on submit/cancel the reducer returns focus to the file panel, where `n`/`p`
(`:when function-has-matches`) step the matches. See "File-list layouts" above.

The function panel's view state (scroll, search, line numbers) and these intents
are kernel-owned (the panel is kernel UI); the extension owns the keys that emit
them, the matching algorithm, and the rendering.

While the search input is open, every key except Enter (submit) and Esc
(cancel) is delivered to the query as a RawKeyEvent (same readline editing as
Select inputs), so the other function-panel bindings do not fire during entry.
Search is less-style: `/` opens the input, Enter runs it and jumps to the first
match, and `n`/`p` step forward/back through matches (wrapping). Matching is
ASCII case-insensitive.

## Select Mode (shared by every picker; instance scopes override by longest prefix)

| Key | Intent | Condition |
|-----|--------|-----------|
| Enter | Confirm (Input phase: switch to Navigate; Navigate: confirm) | |
| Esc | Cancel | |
| Ctrl-n, Down; Ctrl-p, Up | CursorDown, CursorUp (move the results) | |
| w, s | CursorUp, CursorDown | Navigate phase |
| a, d; W, S | CursorLeft, CursorRight; CursorTop, CursorBottom (2-D grid moves over a path-valued result list) | Navigate phase |
| v | CycleListLayout (the file-search results are a file grid) | Navigate phase |
| Space | ToggleSelect (token: mark candidate for Emit instances; path: toggle into the file selection) | Navigate phase |
| q | Cancel | Navigate phase |

Returning to the Input phase needs no binding: any unbound printable key in
the Navigate phase is delivered as RawKeyEvent; the reducer switches back to
Input and applies the character.

Input fields use readline-style editing (tui-input), applied by the reducer:

| Key | Action |
|-----|--------|
| Ctrl-a, Ctrl-e | beginning / end of line |
| Ctrl-k, Ctrl-u | kill to end / beginning of line |
| Ctrl-w | delete word |
| Ctrl-b, Left; Ctrl-f, Right | move one character |
| Ctrl-h, Backspace; Ctrl-d, Delete | delete backward / forward |

## Policy Mode

| Key | Intent |
|-----|--------|
| y, Enter | Confirm (allow; resume the pending intent) |
| n, Esc | Cancel (deny) |

## Layers and Collision Detection

| Layer | Priority | Source |
|-------|----------|--------|
| Core | lowest | embedded defaults |
| Extension | middle | bundled and dynamically loaded extensions (keymaps and entry bindings) |
| User | highest | the user's own configuration files only |

Later layers win; within a layer, the last definition wins. Collisions are
checked at startup across all layers and emit a warning naming both layers.
A user can rebind or disable any extension-provided key. Deleting the user
configuration reproduces exactly the embedded defaults.

The Core layer is sourced from the embedded Scheme keymap, evaluated by the
session and decoded by the codec; on any failure it falls back to the native
default table. The User layer comes from `keybindings.scm`, one
`(bind <mode> <panel|#f> "<key>" <intent> [<when>])` form per (uncommented)
line — panel `#f` means any focused panel. Scope specificity still applies
before layer priority, so a User binding overrides a Core one only when its
panel scope is at least as specific.

Conditional bindings use `:when` clauses, evaluated in three classes without
any REPL round trip on the input path — see [SCHEME.md](SCHEME.md).
