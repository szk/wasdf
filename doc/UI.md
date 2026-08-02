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

**Rows: right-aligned property columns.** In the Rows layout only, each row may
show `PERM OWNR SIZE TIME` right-aligned after the filename, in that column
order. The set of fields shown is a level chosen from the clearance left over
after the filename column, not a fixed layout:

| Level | Fields shown |
|-------|--------------|
| Max | PERM OWNR SIZE TIME |
| Large | PERM SIZE TIME |
| Medium | SIZE TIME |
| Small | TIME |
| None | (name only, full width) |

The filename column width is the **longest filename in the current listing**
(not each row's own length), so every row's property block lines up regardless
of how short that row's name is. Clearance is `panel width − (longest name +
gap)`; the renderer picks the widest level whose fields fit that clearance,
falling back one level at a time (Max → Large → Medium → Small → None) until
one does. OWNR and SIZE columns size themselves to their own widest value in
the listing; PERM and TIME are fixed width. This is purely a render-time
concern — the reducer and `AppState` are untouched, matching the geometry-free
principle above. All four fields draw through one shared role,
`file-row-props` (`@/settings/Display/Themes/*/file-row-props`) — distinct
from the top prop panel's own per-field `prop-perm-value` / `prop-ownr-value`
/ `prop-size-value` / `prop-time-value` roles, so recoloring the file list's
property column never touches the prop panel's look, or vice versa. Themable
like any other role — including its own background color and
Bold/Italic/Underline modifiers, not just fg.

**The file-size bar.** Past the fields themselves, a logarithmic bar
visualizes each file's size relative to the largest *file* in the current
listing (directories don't carry a meaningful size, so they set no scale and
draw no bar of their own). `@/settings/Display/file-row-size-graph` toggles it
on/off (default on). It reuses the property area rather than a separate
widget: the bar is drawn by painting whatever text already occupies its
columns with a `bg` color sampled from a smooth gradient between the two ends
of `Role::FileRowSizeGraph` (fg the gradient's left end, bg its right end,
themable like any other role at `@/settings/Display/Themes/*/file-row-size-graph`
— either end left "nil" turns the bar off too, same as the option). The
gradient is fixed to **each row's own drawn bar**, not the property area as a
whole: the bar's own left edge is always the left-end color, its own right
edge (the zero point) is always the right-end color, whatever the bar's
length — so a short bar still shows the gradient's full range, left end to
right end, just compressed into fewer columns, not only a sliver of whichever
color the property area's fixed edge happens to be. The bar's zero point
itself is anchored to the **right edge** of the property area, so a bar
always grows *leftward* as a file's size grows, however many fields are
currently shown; the largest file in the listing fills the entire property
width, one end of the gradient at each edge, and everything smaller scales
logarithmically (an order-of-magnitude jump moves the bar far more for a
small file than for one already near the top of the scale).

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

The `[ ` / ` ]` brackets carry their own theme role, distinct from the title
text, and follow focus: the **active** (focused) panel draws them in
`title-bracket-focused` and every other panel in `title-bracket` (the title text
itself uses `title-focused` / `title`). An unbracketed banner is the `nameplate`
role. The bracket roles default to the title colors, so a theme that does not set
them looks unchanged.

A panel's **bottom** border carries up to two labels, both on the top area's
bottom border (shared with the main panels' top border): a left-aligned **path**
(the current directory, belonging to the **file panel**) and, when the function
panel is previewing file content, a right-aligned **filename** (the cursor entry,
which *is* the function panel's title — the name of the file being shown). Both
are themed focus-aware, each following **its own** panel's focus — the path the
file panel's, the filename the function panel's — so exactly the focused side
brightens (they never light up together). The filename is omitted for non-content
frames (Command / Exec / Select, or an empty directory), where the function panel
keeps its own mode title.

The two labels lay out across the shared row by five cases, driven by whether the
function panel is visible and how the path length compares to the junction column:

1. **No func panel, path fits** — the path fills from the left; no filename.
2. **Func visible, path shorter than the junction** — the `┬` junction is kept
   and the filename starts just after it.
3. **Func visible, path longer than the junction** — the path overwrites the `┬`
   and flows past it (the vertical divider still descends in the row below); the
   filename follows after a one-cell rule separator.
4. **As case 3, the filename no longer fits** — it is `…`-truncated in brackets.
5. **The path alone overflows the width** — it is `…`-truncated; no filename.

A real label character always wins over the border line, so the path and filename
stay intact; the rule padding and separators between and after the labels restore
the crossed junction glyphs and erase any function title bled onto this shared
row.

### Cursor rendering

The row cursor of the two navigable panels — the **file list** (PANEL_FILE) and
the **function** rows (PANEL_FUNCTION) — is drawn with explicit theme
colours that follow the panel's focus: `cursor-active-fg` / `cursor-active-bg`
while that panel is focused, and the subdued `cursor-fg` / `cursor-bg` while it is
not. The cursor is therefore always visible, brightening on focus rather than
appearing/disappearing. (The Select candidate list and the input-form carets —
the Select `search`/`Argument` field, the `/` content search — still use a
reverse-video block at the real caret while focused.)

## Panels

Panels nest: the **top** panel is a container whose children are **nav** and
**prop**, side by side.

| Panel | Description | Provider |
|-------|-------------|----------|
| top | Container for the top row; holds the nav and prop panels side by side | kernel UI |
| nav | Child of top. A macOS Finder-style column (Miller) directory navigator: the path from root to the current directory runs along the panel's horizontal axis, each level's sibling directories stack vertically in its column (the path element centered, `─` linking adjacent levels), the current directory is highlighted, and columns are right-aligned on overflow so it stays visible. Its title is the app banner `/ wasdf vX /` (unbracketed); the full current-directory path is on the file panel's title | kernel UI |
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
the kernel's hands); the provider's `view()` then returns a logical `PanelView`
that core lays out. There is no Scheme round trip at render time. Loading a different file resets the scroll
positions and any active search (`ResetContentView`, dispatched when new content
arrives).

**Empty directory (no entry under the cursor).** When the directory is empty (or
nothing is otherwise under the cursor) while the panel is following, there is
nothing to preview: the provider **drops its stashed content** so the previous
file's content and title do not linger, and the content frame shows the `(empty)`
placeholder (matching the prop panel). This is a property of *following with no
target*, not of hiding the panel — a panel that is merely hidden keeps its stash,
so toggling it back does not force a reload. The file panel itself simply shows
no rows.

A content read that was already **in flight** when the cursor left its target
(e.g. it was loading the previous file when you stepped into the empty directory)
is **dropped on arrival**: the kernel discards any content completion whose path
is no longer under the cursor, so a late read cannot re-populate the preview the
clear just emptied. This is the same stale-result discipline the async round-trip
contract applies elsewhere (doc/ARCHITECTURE.md).

### Renderers

The content frame draws one of these. Extension content (whether a bundled
extension's `view()` return or a dynamic extension's pushed `ShowFunctionContent`)
is always a logical `PanelView` (a `Document` of blocks + role/color spans, or an
`Image`), laid out by the kernel's layout engine (`ui/middle/function` +
`ui/content`) with the shared label-alignment / wrap / scroll / search-highlight /
line-number handling. The extension describes the content; core lays it out and
paints it. The division of labor (thin core, fat extension) is specified in
[EXTENSION.md](EXTENSION.md).

| Renderer | Content | Provider | Frame |
|----------|---------|----------|-------|
| extension content | text (syntax-highlighted via syntect), hex, directory listing, or image (Chafa truecolor symbol cells) — produced by the active content extension and painted by `ui/content` | extension + kernel blitter | Content |
| command summary | the resolver op, targets, chosen options, and destination, resolved synchronously at render time | kernel UI | Content |
| exec | captured Execute output | kernel UI | Exec |

When no content extension is registered, or the active one declines the
cursor entry (nothing worth previewing for that file), the function panel
shows `N/A` (file metadata lives in the prop panel) — distinct from `(empty)`
(nothing under the cursor at all) and `(loading…)` (a read is in flight).

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
| InputForm | InputFormSpec | A small popup text-entry dialog, sharing its chrome with Policy's Confirm box. The unified replacement for asking an extension for one line of free text (a name, a hex color, an option value) |
| Extension | extension id, mode id, state | Extension-defined UIs that are genuinely not pickers |

Mode ids are colon-separated strings (file; select:file-search; policy;
input-form; extension modes as extension-id:mode-id). Keymap scopes and
handler registration match by longest id prefix on segment boundaries.

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
| source | FileWalk (deep search — a recursive walk, ranked async), Entries (shallow filter — a fixed `Entry` pool ranked synchronously, no I/O; see "Deep search vs. shallow filter" below), Commands, or Static (caller-supplied items, carried inside the spec) |
| input | Fuzzy, Path, or None |
| on_confirm | Navigate, RunCommand, or Resolve. Resolve carries a `ResolverRequest` template + a `fill` (Dst / Path). One Select (Static option candidates + Path input) is the whole "command op": the input fills dst/name, the Space-marked candidates fill opts, then `RunResolver` runs it |
| initial_query | Optional initial query text (Rename prefills the current file name) |
| function_hint | Optional renderer for the function panel; absent hides the panel |

### Built-in instances

| Instance | source | input | on_confirm | function_hint | Entry |
|----------|--------|-------|------------|---------------|-------|
| file-search | FileWalk (deep) | Fuzzy | Navigate | dir-listing | f on a **real** cwd (unavailable once `filter-maxed` — see below). Hits are kept as an `EntryList` (not flattened to candidates) and rendered + navigated as the file panel's entry grid — same layouts, cursor, 2-D moves, `v`. Confirm does **not** navigate anywhere: it commits the matches directly into **File mode** (see "Filtering the file panel" below), never reaching a Navigate phase of its own |
| filter-list | Entries (shallow) | Fuzzy | Navigate | dir-listing | f on a **virtual** cwd (`@/...` — nothing for a walk to read), or f again once already inside a file-search/filter-list result view (`select-has-view`, replacing the Select in place — see "Deep search vs. shallow filter"). Otherwise identical to file-search: same `EntryList` rendering, same commit path |
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
  never nests. The generic **command Select** (`SelectSpec::command`, still
  core infrastructure) is one screen combining a destination/name input with
  option checkboxes, driven by `OnConfirm::Resolve`; copy/move/rename/mkdir/
  touch no longer route through it in practice (all five are now the cmdop
  extension's own function-panel forms — see "CmdOp" below and
  [EXTENSION.md](EXTENSION.md)'s Bundled Extensions section), but it remains
  the mechanism a Path-input Select uses, available to any extension that
  wants the same single-input-plus-checkboxes shape.
- The core command flow uses `OnConfirm::Resolve` carrying a `ResolverRequest`
  template that the pickers fill (target + options + dst/path); there is no
  per-operation intent. Extensions that need one line of free text push
  `Mode::InputForm` instead, whose confirm re-injects the next intent with the
  typed text under the reserved item key ("Input Form Mode" below); a
  multi-step extension flow is expressed by that re-injected intent carrying
  the resolver key, never by stacked pickers or stacked InputForm instances.
- Spec data (including static candidates and the resolver template) is immutable
  on the mode stack; runtime state (query, phase, ranked results, marks) lives in
  AppState and is mutated only by the reducer.

### Deep search vs. shallow filter

Two distinct operations share the vocabulary "filtering", kept structurally
separate rather than folded into one handler branch:

- **Deep search** (`SelectSource::FileWalk`): a recursive walk from a root
  path, ranked asynchronously via `Plan::Search`. Needs a real directory to
  read; a virtual path (`@/...`) has nothing for it to walk.
- **Shallow filter** (`SelectSource::Entries`): ranks a fixed `Entry` pool
  already in hand — synchronously, in the reducer, no I/O, no plan. Works on
  any pool, virtual or real, since it never touches the filesystem.

Both share the same `EntryList`-grid rendering, the same commit path into
File mode (below), and the same `filter_stack`/Esc unwinding — the distinction
is purely in *how the candidates are produced*, not in how they're shown or
confirmed. `f` and `/` resolve to one or the other depending on where they're
pressed:

| Surface | Key | Behavior |
|---|---|---|
| File panel, real cwd | `f` | Deep: pushes `file-search` (`FileWalk` from `cwd`) |
| File panel, virtual cwd (`@/...`) | `f` | Shallow: `Intent::FilterListing` pushes `filter-list` (`Entries`, snapshotting the panel's current `entries`) — nothing for a walk to read, so filtering the listing already on screen is what `f` means here |
| Inside a Select with a non-empty result view (`select-has-view`) — deep file-search results, or an earlier filter/refine | `f` | Shallow refine: `Intent::SelectRefine` **replaces** the Select in place (`push_mode`'s existing same-mode-replaces rule) with a fresh `filter-list` over the frozen result set. No refine-undo — Esc cancels the whole Select, same as always |
| CmdOp's destination browser (function panel) | `/` | Two-stage — see "CmdOp" below |

`Intent::FilterListing`/`Intent::SelectRefine` carry no data of their own (the
keymap codec builds intents statically, without state); the reducer takes the
`entries`/view snapshot itself when handling them. Shallow ranking
(`filter_entries`, scored the same subsequence way as the palette's
`fuzzy_rank`, just against `Entry::name` instead of `Candidate::label`) is a
plain synchronous function — no `Plan`, no async round trip, so it works
identically while the file panel is showing `@/settings`, `@/messages`, or
any other virtual listing.

### Filtering the file panel (commit)

Confirming a deep search (`FileWalk`) or a shallow filter/refine (`Entries`)
— both `on_confirm: Navigate` — does not produce a `NavigateTo`. Instead the
reducer's `select_confirm` special-cases `on_confirm: Navigate` before even
reading the Select's phase (no other Select ever uses `Navigate`, so keying on
it is equivalent to keying on the source) and hands off to
`commit_file_search`: the Select pops, and its matched `EntryList` becomes the
file panel's own `entries`/`cursor`, in plain **File mode**. The listing the
panel showed a moment ago (real directory, an outer filter, or the pre-refine
result view) is pushed onto `AppState.filter_stack`; `AppState.filter_query`
holds the query that produced what's now showing (`Some("")` for an empty
query, `None` when unfiltered).

This is the point of the design: a committed filter is not a distinct mode or
a parallel set of bindings — it is the file panel's normal listing, sourced
from a search or a shallow rank instead of a directory read. Every existing
File-mode key (copy, move, rename, delete, bookmark, hjkl content scroll,
cursor moves, `v`) reads `entries`/`cursor` directly and needs no awareness of
where the listing came from. Two consequences fall out for free, with no
extra plumbing:

- **Bookmark a filtered result**: `B` bookmarks the cursor entry exactly as it
  would in an unfiltered listing.
- **Filter within a filter**: `f` on an already-filtered real-dir listing
  pushes a fresh deep `file-search` Select (still a full `FileWalk` from
  `cwd`, not a narrowing of the current matches — deep search is defined the
  same way at every level); on a virtual listing it pushes a fresh shallow
  `filter-list` instead, per the table above. Either way, confirming commits
  again, pushing another `filter_stack` frame. Capped at `MAX_FILTER_DEPTH`
  (`core/state.rs`): `f` is gated off (`:when (not filter-maxed)`) once the
  stack is at the cap, so nesting can't grow unboundedly.

Unwinding:

- **Esc pops one level** (`Intent::ClearFilter`, bound `:when has-filter` —
  it beats the unconditioned Esc→ClearSelection binding by specificity):
  restores the popped frame's `entries`/`cursor`/`filter_query` verbatim. With
  no filter active, Esc falls through to its ordinary `ClearSelection` meaning.
- **Any real directory (re)load clears the whole stack.** `load_entries` — the
  single landing point for a fresh `cd`, a refresh, a bookmark jump, `@` — always
  drops `filter_stack`/`filter_query`: the saved levels describe a listing that
  read is about to replace anyway, so there is nothing to unwind back to.
- The file panel's title reflects the active query when filtered (e.g.
  `<cwd> — filter: alpha`, or `<cwd> — filtered` for an empty query), so Esc's
  meaning at any moment is visible, not just inferred from behavior.

## The Confirm Contract

Confirm produces exactly one of two result shapes. Whether a value came from
a candidate or from free input is never distinguished by type.

| Shape | Produced when | Content |
|-------|---------------|---------|
| Single (item) | Enter (input Fuzzy or None) | the current candidate |
| InputOnly (text) | any instance with Path input | the typed text, as-is |

| on_confirm | Consumes | Action |
|------------|----------|--------|
| Navigate | Single | pop, then navigate to the item's path — **except** file-search, whose `FileWalk` source is special-cased ahead of this table entirely: it commits its matches into File mode rather than reaching a Single shape or a NavigateTo (see "Filtering the file panel" above) |
| RunCommand | Single | pop, then re-inject the chosen command's registered intent |
| Resolve | InputOnly (Dst/Path) | pop, fill dst/name from the typed input and `opts` from the marked option candidates, then run `RunResolver(template)` |

Rules:

- Marks (Space, in Navigate phase) are read directly off `SelectState` at
  confirm time — only the command Select uses them, to fill `opts` from the
  Static option checkboxes (even though its input is Path). Elsewhere Space
  is inert; there is no marks-driven confirm shape.
- Path-input instances always confirm as InputOnly. `command`
  (`SelectSpec::command`) is the one Path-input instance the core mode system
  defines; no bundled feature currently pushes it (copy/move/rename/mkdir/
  touch all moved to cmdop's own function-panel forms — see "Structural
  rules" above), but it remains available to any extension that wants its
  shape. Extensions that just need a single line of free text use
  `Mode::InputForm` instead (below), not Select.
- Do not extend on_confirm; new confirm behavior is absorbed by the consumers
  of the two shapes.

## Policy Mode

The dialog renders the pending operation's summary as an overlay. y or Enter
allows: the pending intent resumes past the confirmation gate. n or Esc
denies and pops. The mode carries only the pending intent; there is no
toggleable selection state. Policy is pushed whenever a `RunResolver` (or an
extension resolver-key plan) targets an op whose entry is marked
**destructive** (e.g. delete), or — for copy/move/rename specifically — would
silently overwrite a path that already exists (the `cp`/`mv -i` condition,
checked dynamically against the real filesystem, not a fixed per-op flag)
([ARCHITECTURE.md](ARCHITECTURE.md)).

## Input Form Mode

`Mode::InputForm(InputFormSpec)` is the mechanism an extension uses to ask
the user for one value through a small popup, sized to its content and
centered over the frame — the same `popup_rect` geometry Policy's Confirm
box uses, styled with the neutral `Role::Base`/`Role::Border` roles rather
than Policy's destructive-action warning colors. The popup's **widget**
depends on `spec.kind` (`InputKind`); every kind shares the same Enter/Esc
contract (below) and applies **only on Confirm** — no kind emits anything
to the extension before that, so Cancel never needs to roll anything back.

| Field | Meaning |
|-------|---------|
| title | The popup's border title (may carry a value hint, e.g. `"coloring (none \| LS_COLORS \| Light \| Dark \| Custom)"`) |
| initial | The prefilled value, encoded per `kind` (see the table below) |
| kind | Which widget renders and how it edits (`InputKind`) |
| template | The `ExtensionIntent` re-injected on confirm |

| Kind | Widget | Edited with | Wire value under `KEY_ITEM` |
|------|--------|-------------|------------------------------|
| Text | caret over typed text (`tui-input`) | printable keys, Backspace/Delete/arrows/Home/End, Ctrl-a/e/k/u/w/b/f/h/d | the raw string |
| Bool | a 2-row List, `On`/`Off` | wasd/arrows (any direction) or Space — flips | `"#t"` / `"#f"` |
| Choice | a List, one row per option | w/s or Up/Down | the chosen option string |
| Int | a Gauge over `min..max` | a/d or Left/Right ±1, w/s or Up/Down ±10, W/S or Home/End → min/max, or type digits directly | the decimal string |
| List | a Table (items + "+ Add" + "✓ Confirm" rows) | w/s or Up/Down browse, `a` add, `d`/Delete/Backspace remove, Enter edits/adds/confirms depending on the row | comma-joined |

`Text`'s runtime state lives in `AppState::input_form` as `InputFormState::Text { value, cursor }`,
mirroring how `SelectState` sits beside `Select`; every other kind has its
own variant (`Bool`, `Choice`, `Int`, `List`) holding just what that
widget needs (see `core::state::InputFormState`). Typing (Text, and List's
nested per-item edit) goes through the `tui-input` crate's backend-agnostic
`Input::handle(InputRequest)`, translated from this codebase's own
`Key`/`KeyCode`/`Mods` — keeping `core::reducer` free of any terminal-backend
dependency.

A color is **not** an `InputKind` — theme's role editor hosts its own
BarChart-and-swatch editor directly in the **function panel** instead of a
popup, the same `render_form`/`form_owner` seam CmdOp's builder uses, but
opened passively by cursor-follow rather than an explicit action — landing
on a role auto-opens it, hjkl edits write straight through with no confirm
step ([EXTENSION.md](EXTENSION.md)'s Bundled Extensions section); `InputForm`
only covers Text/Bool/Choice/Int/List.

The structured navigation above is **not** hardcoded in the reducer — it is
the ordinary `Cursor{Up,Down,Left,Right,Top,Bottom}`/`ToggleSelect`/
`PageUp`/`PageDown` intents (the same ones File mode and Select already use),
reinterpreted per `InputFormState` variant, plus two new InputForm-only
intents (`AddItem`/`RemoveItem`) for the `List` kind's row management.
`keymap.scm`'s `input-form` scope binds wasd (+ arrows, matching the rest of
the app's convention) to these, gated by a `"input-form-typing"` condition
that's true for the `Text` kind and a `List` row's nested item edit — so
those keys fall through as `RawKeyEvent` to `tui-input` instead of being
intercepted as navigation, exactly like Select's own Input/Navigate phase
gating already works for its wasd bindings. `a`/`d` additionally split on a
`"input-form-list-browsing"` condition: cursor-left/right everywhere else,
add/remove-row for `List` while browsing (`List` has no left/right concept).
Only `Int`'s digit-typing and the two typing paths above stay as raw,
un-enumerated keys — the same reason Select's own query input isn't
keymap-declared either.

Enter confirms: pop the mode, then re-inject `template` with the encoded
value inserted under the reserved `KEY_ITEM` key — exactly the same contract
`Select`'s old `OnConfirm::Emit` used, so every `*-apply` handler on the
extension side needs no changes regardless of kind. (`List` is the one
exception: Enter is contextual on which row is selected — it edits/adds a
row on every row except the trailing "✓ Confirm" one, since `keymap.scm`
binds Enter to `Intent::Confirm` for every InputForm instance and there is
no other way to give Enter both a row-local meaning and a way to actually
confirm.) Esc always cancels the **whole** popup regardless of any such
nested state: pop the mode and clear `input_form`, emitting nothing.

## Selection and Cursor Rules

- The selection is a set of paths, never of indices.
- After any reload (refresh, dotfiles toggle, post-operation reload), paths no
  longer present in the listing are dropped from the selection.
- If the cursor's entry disappears, move to the nearest surviving neighbor;
  if none, the first entry; if the listing is empty, no cursor.
- ToggleSelect (Space) marks the item under the **active interaction target's**
  cursor: normally the file-panel cursor entry, but while a CmdOp panel is open it
  is that panel's unified cursor — an option checkbox, or a listing entry into the
  same shared path selection — regardless of which panel holds keyboard focus
  (the CmdOp cursor is also what wasd/hjkl drive). Path-valued Selects
  (file-search) likewise mark into the shared file selection.
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

## Focus Follows the Action

Within the middle area, keyboard focus follows the intent that is dispatched: an
intent that operates on the **file** list focuses the file panel; one that
operates on the **function** panel focuses the function panel. This is a single,
uniform rule in the reducer (`intent_focus`), applied before every intent is
handled — not per-intent focus code:

- **File panel** ← `Cursor{Up,Down,Left,Right,Top,Bottom}` (the `wasd` keys) and
  `Activate`.
- **Function panel** ← `Func{Up,Down,Left,Right}` (the `hjkl` keys from the file
  panel), `Scroll*` / `Page*`, `Open`, and CmdOp listing navigation
  (`Cmd{CursorDown,CursorUp,Descend,Parent}`).
- Every other intent (mode stack, initiators that manage focus themselves —
  rename/edit, function search, the explicit `FocusPanel`/`FocusNext` ops, async
  results, quit) leaves focus unchanged.

So pressing `hjkl` from the file panel both operates the function panel **and**
moves focus there; pressing `wasd` from the function panel both moves the file
cursor **and** returns focus to the file panel. To make this reciprocal, the
function panel also binds `wasd → Cursor*` (always) and `h`/`l → FuncLeft/Right`
(content), so the same keys mean the same panel from either side. The panel ids
are the shared `core::PANEL_FILE` / `PANEL_FUNCTION` constants — the single source
of truth the reducer writes and the middle renderer recognizes.

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
| f | push Select file-search on a real cwd, or FilterListing (push Select filter-list) on a virtual cwd (`:when (not filter-maxed)` either way) | deep search on a real directory, shallow filter of the current listing on a virtual one (`@/...`); either way, confirm filters the file panel in place — see "Deep search vs. shallow filter" |
| x | push Select command-palette | command palette |
| W | CursorTop | first entry |
| S | CursorBottom | last entry |
| Ctrl-a | SelectAll | select all |
| c | `(ext "cmdop" "start-copy")` | begin copy — the cmdop extension's function-panel builder (options + a browsable destination listing) |
| m | `(ext "cmdop" "start-move")` | begin move — same builder as copy |
| R | `(ext "cmdop" "start-rename")` | begin rename — a name prompt in the function panel, prefilled with the current name |
| M | `(ext "cmdop" "start-create")` | begin mkdir/touch — a multi-line name buffer in the function panel; a trailing `/` makes a directory, otherwise a file, one per line — see "CmdOp" below |
| e | StartEdit | edit in the user's editor (Suspended form) |
| , | CycleFunctionPanel | hidden → 2:1 → 1:1 → 1:2 → hidden |
| . | ToggleDotFiles | toggle hidden files |
| @ | ToggleVfsRoot | jump to the VFS root (or back to the real filesystem) |
| ~ | `(navigate-to "~")` | jump to the home directory (`$HOME`, falling back to `/`) |
| Esc | ClearFilter (`:when has-filter`), else ClearSelection | pop one filter level while filtered — see "Filtering the file panel"; else clear selection |
| r | Refresh | reload |
| Tab, Shift-Tab | FocusNextPanel, FocusPrevPanel | panel focus |
| q, Ctrl-c | Quit | quit |

Extension entry keys merge into this table at the Extension layer and are
user-overridable. Example: the archive extension binds p to start-pack when a
selection exists, and P to start-unpack when the cursor entry is an archive;
the bookmarks extension binds B to add the current path as a bookmark.

## Function Panel (panel-scoped bindings, function panel focused)

Core bindings (embedded defaults): vertical scroll, line numbers, open/back,
kill, cycle, hide must work with zero extensions, so they live in core (vertical
scroll is also used by the Exec frame).

| Key | Intent | Condition | Description |
|-----|--------|-----------|-------------|
| w, a, s, d | CursorUp, CursorLeft, CursorDown, CursorRight | | move the **file** cursor; returns focus to the file panel (Focus Follows the Action) |
| j, Down; k, Up | ScrollDown, ScrollUp | | vertical scroll |
| h, Left; l, Right | FuncLeft, FuncRight | content | horizontal scroll |
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
| f | SelectRefine (replace this Select in place with a shallow `filter-list` over its own current result view — see "Deep search vs. shallow filter") | Navigate phase, and the view holds at least one entry (`select-has-view`) |
| Space | ToggleSelect (token: mark/unmark a candidate, read at confirm only by the command Select's `opts`; path: toggle into the file selection) | Navigate phase |
| q | Cancel | Navigate phase |

Returning to the Input phase needs no binding: any unbound printable key in
the Navigate phase is delivered as RawKeyEvent; the reducer switches back to
Input and applies the character.

Input fields use readline-style editing, applied by the reducer:

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

## Input Form Mode

| Key | Intent | Condition |
|-----|--------|-----------|
| Enter | Confirm (pop, re-inject `template` with the encoded value under `KEY_ITEM` — except `List`, where Enter edits/adds a row unless the cursor is on the trailing "✓ Confirm" row) | |
| Esc | Cancel (pop, emit nothing, regardless of kind or any nested row-edit state) | |
| w, Up; s, Down | CursorUp, CursorDown | not input-form-typing |
| Left; Right | CursorLeft, CursorRight | not input-form-typing |
| a | CursorLeft | not input-form-typing, not input-form-list-browsing |
| d | CursorRight | not input-form-typing, not input-form-list-browsing |
| W, Home; S, End | CursorTop, CursorBottom | not input-form-typing |
| Space | ToggleSelect | not input-form-typing |
| PageUp; PageDown | PageUp, PageDown | not input-form-typing |
| a | AddItem | input-form-list-browsing |
| d, Delete, Backspace | RemoveItem | input-form-list-browsing |
| any other key | RawKeyEvent — `tui-input`'s `Input::handle` for `Text` (same Ctrl chords as Select's input fields, above), `List`'s nested item edit, and `Int`'s direct digit-typing | |

`input-form-typing` is true for the `Text` kind and a `List` row's nested
edit; `input-form-list-browsing` is true for the `List` kind while not
mid-item-edit. Both are registered in `script::condition`, read `AppState.input_form`,
and follow the exact same "same key, different meaning per predicate" pattern
Select's own `select-phase-navigate`/`select-phase-input` already use.

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
any Scheme session round trip on the input path — see [SCHEME.md](SCHEME.md).
