# UI, Modes & Keymap

The UI is a panel-based layout system. Rust renders; Scheme declares layouts
and panel placement ([SCHEME.md](SCHEME.md)). The UI layer holds render-only
caches; anything that affects behavior lives in AppState or the Mode stack and
is mutated through the reducer ([ARCHITECTURE.md](ARCHITECTURE.md)). This
document covers the layouts and panels, the mode stack, and the keymap.

## Layouts

Core built-in layouts are exactly two: file and select. Both are declared in
Scheme (`app/layout.scm`) and may be redefined by the user's configuration,
per environment if desired ([SCHEME.md](SCHEME.md), Role 3); the tables below
describe the embedded definitions. Policy and InputForm render as overlays on
whatever layout is active; an extension mode renders over the file layout.

### The file layout

| Row | Content |
|-----|---------|
| top | the top panel, holding nav, prop, and monitor side by side; height per vertical size class below |
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

- In **Rows**, `d` on a directory enters it; `d` on a file **shows its content**
  — it makes the function panel visible in the Content sublayout at the current
  ratio, leaving focus on the file panel. It never changes the ratio and never
  hides the panel; a second `d` does nothing more. The ratio is cycled only by
  `,`. In Columns/Grid the right wall just clamps; show content there with `,`.
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
| top | the top panel (nav, prop, and monitor) |
| main | a left column at ratio-left — the select input (collapsed when the instance has no input) above the candidate list — and the function panel at ratio-right while the spec sets a function hint |
| bottom | help |

### Screen chrome (the connected frame)

The screen is one connected frame: adjacent panels share the line between them.
nav, prop, and monitor share the top panel's frame; the file and function panels meet on a
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
 wasd cursor  jk scroll  hl ◂▸  / search  n line#  gG top/bottom  Enter open  q hide
```

The file panel's title shows the full current-directory path. The function
panel's title comes from whatever drew it (see Borders and Scrollbars). The shared vertical line's
junction with the top frame slides as the ratio cycles (2:1 / 1:1 / 1:2); the
bottom-left junction tucks under the file panel rather than drawing a separate
box.

Where each panel sits comes from the layout ([SCHEME.md](SCHEME.md), Role 3),
so the frame follows these rules rather than fixed positions:

- **Open edges.** A panel has no left border where it meets the frame's left
  edge below the top row, and no bottom border where it rests on an unboxed
  panel (`help`) or on the frame's bottom edge. Everything else is boxed.
- **The path and the file name.** The path is written on the file panel's top
  border — in the select layout, on the top border of the panel in its place
  (`select-input`, else `select-candidates`). When the function panel's top
  border continues the same line to its right, the two labels share that line
  by the five cases below (the path may flow past the junction; the file
  name, shown while the function panel previews a file, follows it). A
  function panel anywhere else keeps its own title on its own border.
- Border labels are drawn after every panel title, so on a shared line they
  win over a title that bled onto it.

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
unbracketed banner) and a right-aligned info string (e.g. the top row's status
readout). Both are drawn over the border and flow around junction
cells, so the connected frame and its junctions stay intact under long text.

The `[ ` / ` ]` brackets carry their own theme role, distinct from the title
text, and follow focus: the **active** (focused) panel draws them in
`title-bracket-focused` and every other panel in `title-bracket` (the title text
itself uses `title-focused` / `title`). An unbracketed banner is the `nameplate`
role. The bracket roles default to the title colors, so a theme that does not set
them looks unchanged.

A panel's **bottom** border carries up to two labels, both on the top area's
bottom border (shared with the main panels' top border): a left-aligned **path**
(the current directory, belonging to the **file panel**) and, while the function
panel is visible, a right-aligned **function title**. The function title has one
source: whatever drew the function panel's frame this pass supplies it — the
content provider (the name of the file being shown), a form (the title its
`render_form` returned), the Exec frame, or the command summary. An empty
directory supplies none. Both labels are themed focus-aware, each following
**its own** panel's focus, so exactly the focused side brightens.

The two labels lay out across the shared row by five cases, driven by whether the
function panel is visible and how the path length compares to the junction column:

1. **No func panel, path fits** — the path fills from the left; no title.
2. **Func visible, path shorter than the junction** — the `┬` junction is kept
   and the title starts just after it.
3. **Func visible, path longer than the junction** — the path overwrites the `┬`
   and flows past it (the vertical divider still descends in the row below); the
   title follows after a one-cell rule separator.
4. **As case 3, the title no longer fits** — it is `…`-truncated in brackets.
5. **The path alone overflows the width** — it is `…`-truncated; no title.

A real label character always wins over the border line, so the path and title
stay intact; the rule padding and separators between and after the labels restore
the crossed junction glyphs and erase any function title bled onto this shared
row.

### Cursor rendering

The row cursor of the two navigable panels — the **file list** (PANEL_FILE) and
the **function** rows (PANEL_FUNCTION) — is drawn with explicit theme
colours that follow the panel's focus: `cursor-active-fg` / `cursor-active-bg`
while that panel is focused, and the subdued `cursor-fg` / `cursor-bg` while it is
not. The cursor is therefore always visible, brightening on focus rather than
appearing/disappearing. (The Select candidate list and the text-input carets —
the Select `search`/`Argument` field, the `/` content search — use a
reverse-video block at the real caret while focused.)

## Panels

Panels nest: the **top** panel is a container whose children are **nav**,
**prop**, and **monitor**, side by side.

| Panel | Description | Provider |
|-------|-------------|----------|
| top | Container for the top row; holds the nav, prop, and monitor panels side by side | kernel UI |
| nav | Child of top. A Finder-style column (Miller) directory navigator: the path from root to the current directory runs along the panel's horizontal axis, each level's sibling directories stack vertically in its column (the path element centered, `─` linking adjacent levels), the current directory is highlighted, and columns are right-aligned on overflow so it stays visible. The columns are `AppState.trail`, computed by the ReadDir worker — the panel reads no directory itself. Its title is the app banner `/ wasdf vX /` (unbracketed) | kernel UI |
| prop | Child of top. The cursor entry's properties — PATH, SIZE (human + bytes), PERM (permissions, owner, group — names resolved in the worker), and the C/M/A times — each truncated to width | kernel UI |
| monitor | Child of top. System metrics as gauges with rolling sparklines, top to bottom CPU / MEM / NET / DSK — a short panel loses rows from the bottom, so CPU/MEM/NET stay longest — drawn from the Monitor service's latest snapshot; the sampler runs on its own thread at `monitor.*` cadence. Toggled by `top.monitor-visible` | kernel UI |

**Status readout.** The rightmost top panel's border carries, right-aligned
and clear of the corner, `[WiFi:76%]─[BAT:87%]─[YY-MM-DD HH:MM:SS]─`: the
clock at the right end, the battery (`+` while charging) and WiFi signal to its
left, each only when the machine reports it (battery: Linux
`/sys/class/power_supply`, macOS `pmset`; WiFi: Linux `/proc/net/wireless`,
sampled every 30 s). Too narrow, it gives up the date, the seconds, WiFi,
then BAT. Refreshed every
`prop.refresh-ms` (default 1000).
| file | The file list. Its title shows the full current-directory path | kernel UI |
| function | Renderer container (see the frame table below) | kernel UI + extensions |
| help | Key hints, drawn in reverse video (negative). **Generated from the labelled bindings actually in force** — every chip is a key that would really fire, resolved through the same walk `handle_key` uses, so the row cannot drift from the keymap ([HELP.md](HELP.md)). While an extension holds the raw keyboard the row shows the capture's release key instead, since nothing else is reachable | kernel UI |
| select-input | Select-mode input field | kernel UI |
| select-candidates | Select-mode candidate list | kernel UI |

Panels have no event handling: every key decision goes through the
KeymapRegistry (panel-scoped bindings plus RawKeyEvent). A panel that needs key
behavior ships keymap definitions; there is no second input path.

Panel renderers are functions of the read-only AppState, the render caches, and
shared snapshots (theme, render options, monitor). They perform no I/O
([ARCHITECTURE.md](ARCHITECTURE.md), Render Contract).

## The Function Panel

### Visibility and sublayout

| Rule | Content |
|------|---------|
| ratio | FunctionPanelState.ratio is always kept and shared by File and Select (cycling happens only via the comma key in File mode) |
| Visibility in File | FunctionPanelState.visible |
| Visibility in Select | whether the active SelectSpec sets a function hint. The File-side visible flag is never consulted in Select |
| sublayout | The type of the display frame, not a renderer kind. Exactly two: Content (the viewing frame) and Exec |
| Content frame | hosts the content owner's content, an extension's form, the command summary, and the file-search directory listing. While `form_owner` is set the form is drawn; otherwise the content owner's `view()` |
| Exec frame | hosts only the streamed output of Execute (line-by-line) |

In File mode, while the function panel is visible, the content frame shows the
content of the file under the cursor and follows the cursor automatically — no
key press is required (the entry's metadata lives in the prop panel, not here).
The kernel reads the cursor path (the Read plan) and hands the bytes/entries to
the content provider's `accept_content`, which decodes them (MIME dispatch, off
the kernel's hands); the provider's `view()` then returns a shared, logical
`PanelView` that core lays out — once per content revision and width, drawing
only the visible rows each frame. Loading a different file resets the scroll
positions and any active search (`ResetContentView`, dispatched when new content
arrives).

**Empty directory (no entry under the cursor).** When the directory is empty (or
nothing is otherwise under the cursor) while the panel is following, there is
nothing to preview: the provider **drops its stashed content** so the previous
file's content and title do not linger, and the content frame shows the `(empty)`
placeholder (matching the prop panel). This is a property of *following with no
target*, not of hiding the panel — a panel that is merely hidden keeps its stash,
so toggling it back does not force a reload. The file panel itself shows a
muted `Empty Dir` (a filter that matched nothing stays blank — that is not an
empty directory). A **directory under the cursor** that has nothing in it
previews as the same muted `Empty Dir`.

A content read that was already **in flight** when the cursor left its target
is **dropped on arrival**: the kernel discards any cursor-follow completion whose
path is no longer under the cursor, so a late read cannot re-populate the
preview the clear just emptied ([ARCHITECTURE.md](ARCHITECTURE.md)).

### Renderers

The content frame draws one of these. Extension content — always the content
owner's `view()`, for bundled and dynamic extensions alike — is a logical
`PanelView` (a `Document` of blocks + role/color spans, or an
`Image`), laid out by the kernel's layout engine (`ui/middle/function` +
`ui/content`) with the shared label-alignment / wrap / scroll / search-highlight /
line-number handling. The extension describes the content; core lays it out and
paints it. The division of labor (thin core, fat extension) is specified in
[EXTENSION.md](EXTENSION.md).

| Renderer | Content | Provider | Frame |
|----------|---------|----------|-------|
| extension content | text (syntax-highlighted via syntect), hex, directory listing, or image (Chafa truecolor symbol cells) — produced by the active content extension and painted by `ui/content` | extension + kernel blitter | Content |
| command summary | the command Select's live command line: the resolver template with the targets, marked options, and destination spliced in (in-memory; no I/O at render) | kernel UI | Content |
| form | an extension's interactive form (`render_form`) while `form_owner` names it; takes precedence over extension content | extension + kernel chrome | Content |
| dir-listing | a path-valued Select's result (`function_hint: dir-listing`): the directory under the result cursor | kernel UI | Content |
| exec | captured Execute output | kernel UI | Exec |

When no content extension is registered, or the active one declines the
cursor entry (nothing worth previewing for that file), the function panel
shows `N/A` (file metadata lives in the prop panel) — distinct from `(empty)`
(nothing under the cursor at all) and `(loading…)` (a read is in flight).


### Images: `image-protocol`

`@/settings/Display/image-protocol` picks how image previews (a picture file,
a Markdown image) reach the display: `off` (default) draws them as symbol
cells, which works everywhere; `sixel` / `kitty` use that graphics protocol.
The picture is scaled — up or down, aspect kept — until its longer side meets
the image area. It is placed only when it is wholly visible and no overlay
covers the panels; otherwise its symbol cells show.

Whether the display can draw the chosen protocol is known at start: upstream
probes its terminal once (a kitty graphics query answered `OK`, and DA1
parameter `4` for sixel); a backend that draws itself declares it (cz-wasdf's
framebuffer: kitty only). When it cannot — or the terminal reports no cell
size in pixels — the image area's first row reads
`image-protocol: <name> not available` above the symbol cells. Kitty pixels
travel through POSIX shared memory (`t=s`), inline base64 over ssh; see
`ui/graphics.rs`.
## Vertical Responsiveness

Two size classes. A layout's `(size N M)` gives the Large and Compact sizes; the
embedded file layout uses these for the top row:

| Class | Terminal height | Top area | Content rows |
|-------|-----------------|----------|--------------|
| Large | at least 25 rows | 7 rows (5 content + 2 border) | 5 |
| Compact | under 25 rows | 5 rows (3 content + 2 border) | 3 |

## UiManager

Owns the render-only caches: the function-panel layout cache (lowered rows, the
block → row map, and content width per content revision, width, line-number
flag, and theme), image
cells, the notice slot and its expiry deadline, and the measured geometry the
kernel copies back into AppState after each draw. No field of UiManager is ever
read by the reducer or a handler to make a behavioral decision.

---

# Modes

A Mode determines how key input is interpreted and which layout is shown.
Modes form a stack; push enters, pop returns.

## Mode Catalog

| Mode | Carried data | Description |
|------|--------------|-------------|
| File | — | File list. The function panel belongs to File mode as view state, not as a mode |
| Select | SelectSpec | The one generic picker for every modal UI. Pushed by a core intent or an extension |
| Policy | pending intent | Confirmation dialog titled **Confirm**, its key hints (`y / Enter : confirm`, `n / Esc : cancel`) drawn inside the box. y or Enter allows and resumes the pending intent; n or Esc denies. No selection state |
| InputForm | InputFormSpec | A small popup asking for one value (Text / Bool / Choice / Int / List), sharing its chrome with Policy's Confirm box |
| Extension | extension id, mode id, state | Extension-defined UIs that are genuinely not pickers |

Mode ids are colon-separated strings (file; select:file-search; policy;
input-form; extension modes as extension-id:mode-id). Keymap scopes match by
longest id prefix on segment boundaries.

## File Mode and the Function Panel

FunctionPanelState lives in AppState and is reducer-managed:

| Field | Values | Meaning |
|-------|--------|---------|
| visible | bool | Whether the panel shows in File mode (Select uses its own rule above) |
| sublayout | Content, Exec | The active display frame |
| ratio | 2:1, 1:1, 1:2 | file : function split; always kept and shared across modes |
| scroll, hscroll, show_line_numbers | — | Generic content view geometry (vertical scroll is shared with the Exec frame) |
| content_owner | extension id | The one extension whose `view()` the panel shows and to which content search is routed. Starts as the content provider; changes on `ShowFunctionContent{owner}` or another extension's cursor-follow `LoadContent`; returns to the provider when that extension's overlay pops or the panel is hidden ([EXTENSION.md](EXTENSION.md), The content owner) |
| form_owner | extension id | The extension hosting an interactive form in the panel (SetFunctionForm / HostFunctionForm) |
| input | FunctionInput | An open function-panel text input: text, caret, purpose, multiline |
| ext_view | ExtensionValue | Opaque per-extension view state, written via UpdateFunctionView |
| search | PanelSearch | The content-search input (query, caret, active), its help-row hint, and the match list (sorted) + current index |
| exec | ExecOutput | The Exec frame's streamed lines, finished flag, exit code |

A bundled extension's decoded content is not in AppState — it lives inside the
owning extension ([EXTENSION.md](EXTENSION.md)).

Function panel intents:

| Intent | Behavior |
|--------|----------|
| SetSubLayout | Show the panel with the given frame |
| CycleFunctionPanel | hidden → 2:1 → 1:1 → 1:2 → hidden (File mode only) |
| HideFunctionPanel | Hide and refocus the file panel |
| ShowFunctionContent / UpdateFunctionView | Make an extension the content owner and show the panel / store its opaque view state (generic) |
| FunctionSearch{Start,Submit,Next,Prev,Cancel}, SetSearchMatches, SetScroll, ResetContentView | Less-style content search + view reset (see [ARCHITECTURE.md](ARCHITECTURE.md) catalog) |

In the Rows layout, `d` (CursorRight) on a file shows the content (the function
panel in the Content frame at the current ratio; see "File-list layouts"); the
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
| on_confirm | Commit, RunCommand, or Resolve. Resolve carries a `ResolverRequest` template + a `fill` (Dst / Path). One Select (Static option candidates + Path input) is the whole "command op": the input fills dst/name, the Space-marked candidates fill opts, then `RunResolver` runs it |
| initial_query | Optional initial query text |
| input_hint | Optional keyless help-row entry describing what the input is for ([HELP.md](HELP.md)) |
| function_hint | Optional renderer for the function panel (`dir-listing`, `command:summary`); absent hides the panel |
| command_line | For a Resolve Select: the resolver entry's argv template, from which the command summary splices the live command line |

### Built-in instances

| Instance | source | input | on_confirm | function_hint | Entry |
|----------|--------|-------|------------|---------------|-------|
| file-search | FileWalk (deep) | Fuzzy | Commit | dir-listing | f on a **real** cwd (unavailable once `filter-maxed` — see below). Hits are kept as an `EntryList` (not flattened to candidates) and rendered + navigated as the file panel's entry grid — same layouts, cursor, 2-D moves, `v`. Confirm commits the matches directly into **File mode** (see "Filtering the file panel" below) |
| filter-list | Entries (shallow) | Fuzzy | Commit | dir-listing | f on a **virtual** cwd (`@/...` — nothing for a walk to read), or f again once already inside a file-search/filter-list result view (`select-has-view`, replacing the Select in place — see "Deep search vs. shallow filter"). Otherwise identical to file-search: same `EntryList` rendering, same commit path |
| command-palette | Commands | Fuzzy | RunCommand | none | z |
| command | Static (option checkboxes from the command's `--help`/`man`) | Path | Resolve (fill Dst / Path) | command:summary | No core binding; pushed by an extension that wants this shape (initial_query prefills the input). The **Argument** input is the destination/name (free text, never filters); the **Options** list is the command's options, toggled with Space; the function panel shows the live **Command** line — the **actual external command** resolved from the chain (e.g. `cp -R …`, `mv …`), not the op key, with the marked options and target spliced in |

### Phases

Select has two phases so that typing and list navigation never conflict:

- Input phase (only when the instance has an input): printable keys edit the
  query (see Line editing); Up/Down and Ctrl-n/Ctrl-p
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
  never nests. The generic **command Select** (`SelectSpec::command`) is one
  screen combining a destination/name input with option checkboxes, driven by
  `OnConfirm::Resolve` — available to any extension that wants that shape. The
  built-in file operations use CmdOp's function-panel form instead
  ([CMDOP.md](CMDOP.md)).
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
| CmdOp's destination browser (function panel) | `/` | Two-stage, deep then shallow — [CMDOP.md](CMDOP.md) |

`Intent::FilterListing`/`Intent::SelectRefine` carry no data of their own (the
keymap codec builds intents statically, without state); the reducer takes the
`entries`/view snapshot itself when handling them. Shallow ranking uses the one
local ranker (`core::rank`, subsequence-scored — the same one the palette and
Static pickers use, keyed on `Entry::name` here and `Candidate::label` there).
It borrows the pool from the mode stack and keeps the ranked order, never
copying the pool per keystroke. It is synchronous — no `Plan`, no async round
trip — so it works identically on `@/settings`, `@/messages`, or any other
virtual listing.

### Filtering the file panel (commit)

Confirming a deep search (`FileWalk`) or a shallow filter/refine (`Entries`)
— both `on_confirm: Commit` — does not produce a `NavigateTo`: the Select
pops, and its matched `EntryList` becomes the
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
- **Navigating to a directory clears the whole stack** (a fresh `cd`, a
  bookmark jump, `@`, `~`): the saved levels describe listings the user has
  left.
- **Refreshing keeps the stack.** A refresh while filtered — `r`, the dotfiles
  toggle, or the automatic refresh after a completed file operation —
  revalidates the view shown now: its entries are re-stat'ed by path, vanished
  ones drop out, survivors get fresh metadata, and nothing new is added (a
  filter's membership is decided when it is committed). The frames below keep
  their snapshots and are revalidated the same way when Esc returns to them.
- The file panel's title reflects the active query when filtered (e.g.
  `<cwd> — filter: alpha`, or `<cwd> — filtered` for an empty query), so Esc's
  meaning at any moment is visible, not just inferred from behavior.

## The Confirm Contract

Confirm produces exactly one of three result shapes. Whether a value came from
a candidate or from free input is never distinguished by type.

| Shape | Produced when | Content |
|-------|---------------|---------|
| Single (item) | Enter (input Fuzzy or None) | the current candidate |
| InputOnly (text) | any instance with Path input | the typed text, as-is |
| View | Enter in a path-valued instance (file-search, filter-list) | the result `EntryList` |

| on_confirm | Consumes | Action |
|------------|----------|--------|
| Commit | View | pop, then commit the view into File mode (see "Filtering the file panel" above) |
| RunCommand | Single | pop, then re-inject the chosen command's registered intent |
| Resolve | InputOnly (Dst/Path) | pop, fill dst/name from the typed input and `opts` from the marked option candidates, then run `RunResolver(template)` |

Rules:

- Marks (Space, in Navigate phase) are read directly off `SelectState` at
  confirm time — only the command Select uses them, to fill `opts` from the
  Static option checkboxes (even though its input is Path). In a path-valued
  Select, Space toggles the path in the file selection instead, which confirm
  does not read. There is no marks-driven confirm shape.
- Path-input instances always confirm as InputOnly. `command`
  (`SelectSpec::command`) is the one Path-input instance core defines.
  Extensions that just need a single value use `Mode::InputForm` (below), not
  Select.
- Do not extend on_confirm; new confirm behavior is absorbed by the consumers
  of the three shapes.

## Policy Mode

The dialog renders the pending operation's summary as an overlay. y or Enter
allows: the pending intent resumes past the confirmation gate. n or Esc
denies and pops. The mode carries only the pending intent; there is no
toggleable selection state. Policy is pushed when a `RunResolver` (or an
extension resolver-key plan) targets an entry whose confirm policy requires it:
`always` (e.g. delete), or `overwrite` when the request would overwrite an
existing path, checked against the real filesystem at dispatch time
([ARCHITECTURE.md](ARCHITECTURE.md), Resolver Contract).

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
| title | The popup's border title (e.g. `"new theme name"`) |
| input_hint | Optional keyless help-row entry describing what the value is for ([HELP.md](HELP.md)) |
| initial | The prefilled value, encoded per `kind` (see the table below) |
| kind | Which widget renders and how it edits (`InputKind`) |
| template | The `ExtensionIntent` re-injected on confirm |

| Kind | Widget | Edited with | Wire value under `KEY_ITEM` |
|------|--------|-------------|------------------------------|
| Text | caret over typed text | Line editing (below) | the raw string |
| Bool | a 2-row List, `On`/`Off` | wasd/arrows (any direction) or Space — flips | `"#t"` / `"#f"` |
| Choice | a List, one row per option | w/s or Up/Down | the chosen option string |
| Int | a Gauge over `min..max` | a/d or Left/Right ±1, w/s or Up/Down ±10, W/S or Home/End → min/max, or type digits directly | the decimal string |
| List | a Table (items + "+ Add" + "* Confirm" rows) | w/s or Up/Down browse, `a` add, `d`/Delete/Backspace remove, Enter edits/adds/confirms depending on the row | comma-joined |

Runtime state lives in `AppState::input_form`, one `InputFormState` variant
per kind holding just what that widget needs. Typing (Text, and List's nested
per-item edit) goes through the same line editor as every other text input.

A color is **not** an `InputKind` — theme's role editor is a function-panel
form ([THEME.md](THEME.md)).

The structured navigation above is **not** hardcoded in the reducer — it is
the ordinary `Cursor{Up,Down,Left,Right,Top,Bottom}`/`ToggleSelect` intents,
reinterpreted per `InputFormState` variant, plus
the InputForm-only `AddItem`/`RemoveItem` for the `List` kind's rows.
`keymap.scm`'s `input-form` scope binds wasd (+ arrows) to these, gated by the
`input-form-typing` condition (true for `Text` and a `List` row's nested edit)
so that while typing those keys fall through as `RawKeyEvent` — the same way
Select's Input/Navigate phases gate its wasd bindings. `a`/`d` additionally split on a
`"input-form-list-browsing"` condition: cursor-left/right everywhere else,
add/remove-row for `List` while browsing (`List` has no left/right concept).
Only `Int`'s digit-typing and the two typing paths above stay as raw,
un-enumerated keys — the same reason Select's own query input isn't
keymap-declared either.

Enter confirms: pop the mode, then re-inject `template` with the encoded
value inserted under the reserved `KEY_ITEM` key, regardless of kind. (`List` is the one
exception: Enter is contextual on which row is selected — it edits/adds a
row on every row except the trailing "* Confirm" one, since `keymap.scm`
binds Enter to `Intent::Confirm` for every InputForm instance and there is
no other way to give Enter both a row-local meaning and a way to actually
confirm.) Esc always cancels the **whole** popup regardless of any such
nested state: pop the mode and clear `input_form`, emitting nothing.

## Selection and Cursor Rules

- The selection is a set of paths, never of indices.
- A selected path is dropped from the selection only when it no longer exists
  (reported by a Listing's `gone`) — never merely because it is not in the
  listing on screen, so a selection made across directories or filters
  survives refreshes and navigation.
- If the cursor's entry disappears, move to the nearest surviving neighbor;
  if none, the first entry; if the listing is empty, no cursor.
- ToggleSelect (Space) marks the file-panel cursor entry. Path-valued Selects
  (file-search) mark into the same path selection. A form extension binds Space
  to its own action while active (CmdOp: [CMDOP.md](CMDOP.md)).
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
  extension dispatch, the confirm-policy → Policy gate) and broadcasts the
  **cursor-changed event** — when the cursor signature changes
  (cursor path, panel visibility, function scroll offset, or mode id), it
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

Default bindings exist only as the embedded keymap source. The `config.scm`
template lists every one of them as a commented-out `bind!`, so the User layer
contains only what the user actively set ([SCHEME.md](SCHEME.md)).

## Focus Follows the Action

Within the middle area, keyboard focus follows the intent that is dispatched: an
intent that operates on the **file** list focuses the file panel; one that
operates on the **function** panel focuses the function panel. This is a single,
uniform rule in the reducer (`intent_focus`), applied before every intent is
handled — not per-intent focus code:

- **File panel** ← `Cursor{Up,Down,Left,Right,Top,Bottom}` (the `wasd` keys and,
  from the file panel, the arrows) and `Activate`.
- **Function panel** ← `Func{Up,Down,Left,Right}` (the `hjkl` keys from the file
  panel), `Scroll*` / `Page*`, and `Open` — only while the function panel is on
  screen (beside a Select that gives it a hint, otherwise when shown). A hidden
  panel never takes focus: its keys would apply with nothing to show them.
- **Exception — an active form keeps focus.** While an extension holds the
  function panel as an active form (`SetFunctionForm`) and the panel has focus,
  keys bound to file-targeting intents (`wasd`, `W`/`S`, `Enter` → Activate) are
  not resolved to those intents; they go to the form as raw keys
  ([ARCHITECTURE.md](ARCHITECTURE.md), key routing). `Tab` (FocusNextPanel) is how
  the user steps back to the file panel without closing the form. A passive
  form (`HostFunctionForm`) keeps the ordinary rule.
- Every other intent (mode stack, initiators that manage focus themselves —
  rename/edit, function search, the explicit `FocusPanel`/`FocusNext` ops, async
  results, quit) leaves focus unchanged.

So pressing `hjkl` from the file panel both operates the function panel **and**
moves focus there; pressing `wasd` from the function panel both moves the file
cursor **and** returns focus to the file panel. To make this reciprocal, the
function panel also binds `wasd → Cursor*` (except under an active form, above)
and `h`/`l → FuncLeft/Right`
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

Core layer (`app/keymap.scm`):

| Key | Intent | Description |
|-----|--------|-------------|
| w, Up | CursorUp | move the cursor up one cell (clamp at the column top) |
| s, Down | CursorDown | move the cursor down one cell (clamp at the column bottom) |
| a, Left | CursorLeft | move the cursor one column left; at the leftmost column, parent directory |
| d, Right | CursorRight | move the cursor one column right; at the **Rows** right wall, dir → enter, file → show its content (Columns/Grid clamp) |
| Enter, e | Activate | enter the directory, or open the file via the resolver (any cell, any layout) |
| W, S | CursorTop, CursorBottom | first / last entry |
| v | CycleListLayout | Rows → Columns → Grid → Rows |
| k, j | FuncUp, FuncDown | scroll the function panel |
| h, l | FuncLeft, FuncRight | h-scroll the function panel |
| Space | ToggleSelect | toggle selection; the cursor does not advance |
| Ctrl-a | SelectAll | select all |
| f | push `file-search` (`:when (and (not filter-maxed) (not cwd-virtual))`), or FilterListing (`:when (and (not filter-maxed) cwd-virtual)`) | deep search on a real directory, shallow filter on a virtual one — see "Deep search vs. shallow filter" |
| z | push `command-palette` | command palette |
| E | StartEdit | edit in the user's editor (Suspended form) |
| Backspace, Delete | DeleteSelected | delete the targets with the resolver's `delete` entry (`rm`), Policy-confirmed |
| c, m, R, M | start copy / move / rename / create | the built-in CmdOp builder ([CMDOP.md](CMDOP.md)) |
| , | CycleFunctionPanel | hidden → 2:1 → 1:1 → 1:2 → hidden |
| . | ToggleDotFiles | toggle hidden files |
| @ | ToggleVfsRoot | jump to the VFS root, or back to the real filesystem |
| ~ | `(navigate-to "~")` | the home directory (`env.home`, captured at boot) |
| Esc | ClearFilter (`:when has-filter`), else ClearSelection | pop one filter level while filtered; else clear the selection |
| r | Refresh | reload |
| Tab, Shift-Tab | FocusNextPanel, FocusPrevPanel | panel focus |
| q, Ctrl-c | Quit | quit |

Extension layer, bundled extensions (user-overridable):

| Key | Binding | Extension |
|-----|---------|-----------|
| x, X | resume a shell / open a new one | pty ([PTY.md](PTY.md)) |
| e | the editor in a PTY (`:when (option "pty.edit-host")`), overriding Activate | pty |
| / ; n, p | start content search (`:when (and function-visible sublayout-content)`); next / previous match (`:when function-has-matches`) | preview |
| B | bookmark the current path | bookmarks |
| Backspace, Delete | remove the bookmark under the cursor (`:when bookmarks-cursor`) | bookmarks |
| p, P | pack the selection (`:when has-selection`); unpack the cursor archive (`:when cursor-is-archive`) | archive |
| j k h l (H L) | drive the role color editor (`:when theme-color-active`) | theme ([THEME.md](THEME.md)) |

## Function Panel (panel-scoped bindings, function panel focused)

Core bindings (embedded defaults): vertical scroll, line numbers, open/back,
kill, cycle, hide must work with zero extensions, so they live in core. Scroll,
jump, cycle, hide, and focus work in **both** frames (Content and Exec — the
vertical scroll is shared); the rest are frame-specific.

| Key | Intent | Frame | Description |
|-----|--------|-------|-------------|
| w, a, s, d | CursorUp, CursorLeft, CursorDown, CursorRight | both | move the **file** cursor; returns focus to the file panel (Focus Follows the Action) |
| j, Down; k, Up | ScrollDown, ScrollUp | both | vertical scroll |
| g, G | ScrollTop, ScrollBottom | both | jump to the top / bottom |
| PageUp, PageDown | PageUp, PageDown | both | scroll by a page |
| , | CycleFunctionPanel | both | cycle ratio, hide at the end |
| Esc, q | HideFunctionPanel | both | hide and refocus the file panel |
| Tab, Shift-Tab | FocusNextPanel, FocusPrevPanel | both | panel focus |
| h, Left; l, Right | FuncLeft, FuncRight | content | horizontal scroll |
| n | ToggleLineNumbers | content | toggle line numbers |
| Enter | Open | content | open externally via the resolver |
| Enter | SetSubLayout Content | exec | back to the content frame |
| Ctrl-c | KillProcess | exec | kill the process; the panel stays visible |

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
(cancel) is delivered to the query as a RawKeyEvent (Line editing, below), so
the other function-panel bindings do not fire during entry.
Search is less-style: `/` opens the input, Enter runs it and jumps to the first
match, and `n`/`p` step forward/back through matches (wrapping). Matching is
ASCII case-insensitive. The owner reports matches in logical coordinates
(block, byte range); core maps them to panel rows, so a wrapped paragraph's
match lands on the row it was wrapped onto.

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

## Line editing

Every text input — the Select query, the function-panel search input, a
function-panel form input (`BeginInput`), InputForm's `Text` kind and `List`
item edit — is edited by the one line editor (`core::line_edit`), applied by
the reducer to a (text, caret) pair on char boundaries:

| Key | Action |
|-----|--------|
| printable | insert at the caret |
| Ctrl-a, Home; Ctrl-e, End | beginning / end of line |
| Ctrl-k, Ctrl-u | kill to end / beginning of line |
| Ctrl-w | delete the word before the caret |
| Ctrl-b, Left; Ctrl-f, Right | move one character |
| Ctrl-h, Backspace; Ctrl-d, Delete | delete backward / forward |

A multiline form input additionally inserts a newline on Enter unless the
caret's line is empty, in which case Enter submits.

## Policy Mode

| Key | Intent |
|-----|--------|
| y, Enter | Confirm (allow; resume the pending intent) |
| n, Esc | Cancel (deny) |

## Input Form Mode

| Key | Intent | Condition |
|-----|--------|-----------|
| Enter | Confirm (pop, re-inject `template` with the encoded value under `KEY_ITEM` — except `List`, where Enter edits/adds a row unless the cursor is on the trailing "* Confirm" row) | |
| Esc | Cancel (pop, emit nothing, regardless of kind or any nested row-edit state) | |
| w, Up; s, Down | CursorUp, CursorDown | not input-form-typing |
| Left; Right | CursorLeft, CursorRight | not input-form-typing |
| a | CursorLeft | not input-form-typing, not input-form-list-browsing |
| d | CursorRight | not input-form-typing, not input-form-list-browsing |
| W, Home; S, End | CursorTop, CursorBottom | not input-form-typing |
| Space | ToggleSelect | not input-form-typing |
| a | AddItem | input-form-list-browsing |
| d, Delete, Backspace | RemoveItem | input-form-list-browsing |
| any other key | RawKeyEvent — Line editing for `Text` and `List`'s nested item edit; direct digit entry for `Int` | |

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
| User | highest | the settings store's bindings — its `config`, `custom`, and `session` layers ([SCHEME.md](SCHEME.md)) |

Later layers win. Within the Core layer the later definition wins (source
order). Within the User layer the store's layering decides: `session` over
`custom` over `config`, and an `unbind!` hides the bindings below it. Within the Extension layer,
two extensions binding the same (scope, key) are decided by **extension
priority**: the extension with the lower priority rank wins. A user rebind in
the User layer overrides either. Settings edits to bindings apply immediately:
the kernel rebuilds the User layer and re-runs collision detection.

**Extension priority.** Every registered extension has a rank: its position in
the store's `extensions` display order, starting at 1. The default order is
registration order (bundled extensions in `bundled()` order, then optional
extensions in manifest file-name order); `(order! 'extensions '(…))` — or the
settings UI — reorders it, and extensions it does not list follow in
registration order. A lower rank means
higher priority, everywhere one extension must be chosen over another: a key
collision within the Extension layer, the content provider (the lowest-ranked
extension with `provides_function_content`), `entry_style` (the
lowest-ranked extension returning `Some`), and the cursor-changed broadcast
(extensions are called highest rank number first, so the intents of the
highest-priority extension are dispatched last and prevail). The rank is shown as
`@/settings/Extensions/<id>/Priority`, and editable there.

Collisions are checked at startup (and whenever the User layer is rebuilt)
across all layers and emit a warning naming both layers. A user can rebind or
disable (`unbind!`) any extension-provided key. Deleting the user
configuration — `config.scm` and `custom.scm` — reproduces exactly the
embedded defaults.

The Core layer is the embedded Scheme keymap, read by the s-expression reader
and decoded by the codec. The User layer is the projection of the store's
bindings ([SCHEME.md](SCHEME.md)) — panel `#f` means any focused panel. Scope
specificity still applies before layer priority, so a User binding overrides a
Core one only when its panel scope is at least as specific.

Conditional bindings use `:when` clauses — the closed predicate grammar,
evaluated natively with no Scheme round trip on the input path
([SCHEME.md](SCHEME.md)).

## Key Model

A `Key` is a code, modifiers, and an event kind:

- **code** — a character, Enter, Esc, Tab, BackTab, Backspace, Delete, Insert,
  the arrows, Home/End, PageUp/PageDown, or F1–F35. `Shift-Tab` and `BackTab`
  are two spellings of the same key (the terminal reports Shift+Tab as
  BackTab); both are accepted in key strings and resolve to one binding;
- **mods** — ctrl, shift, alt, super, hyper, meta (the last three reported only
  under the Kitty keyboard protocol). Shift is folded into a character and kept
  only for non-character codes;
- **kind** — Press, Repeat, Release.

wasdf asks its own terminal for the Kitty keyboard protocol at startup
(disambiguated escapes, event types, alternate keys, all keys as escapes) and
pops it on exit and around a Suspended child; a terminal without support stays
on legacy keys. Only Press events resolve through the keymap. An input capture
receives every event, losslessly encoded ([EXTENSION.md](EXTENSION.md)).
