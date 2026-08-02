# Extensions

Features beyond the minimal filer are implemented as extensions. The kernel
boots with zero extensions registered: browsing, selection, file operations,
search, the palette, and the confirmation dialog all work; the cursor entry's
metadata is always shown in the prop panel ([UI.md](UI.md)), so with no content
extension the function panel simply shows a placeholder; extension entry keys
and commands do not exist.

## Guiding Goal — thin core, fat extension

> Minimize core's responsibility and size; give extensions the *large*
> responsibility. The more a swapped/replaced extension changes, the better.
> Core is a thin host; the extension is where behavior lives. Anything that
> *can* live in the extension *should*.

The target capability: a feature added as an extension — written against
`wasdf-sdk`, built as a dynamic library and dropped into the extensions directory
alongside its `.scm` manifest — that, with **zero further edits to
`crates/wasdf/src/`**:

1. **joins the processing pipeline** (emits/handles intents, can load content),
2. **receives key input** (binds keys that route to it), and
3. **renders inside the function panel** (and only there).

Bundled extensions use the **same** mechanism; `PreviewExtension` is the
reference client that proves the design (dogfooding).

The function panel **chrome** (border, title placement, the panel rect, the
Exec frame) is the kernel's. The panel **content** — what is drawn, its
appearance, syntax color, search, scroll/h-scroll, line numbers — is the
**owning extension's**. Core hosts, blits, routes keys, reads bytes off-thread,
and stores the gated match list; the extension renders and interacts.

## Kinds

| Kind | Loading | Examples |
|------|---------|----------|
| Bundled | Statically linked; named only in the bundled-extension list inside the extension facade | PreviewExtension, ArchiveExtension, CmdOpExtension, ThemeExtension |
| Optional | A `.scm` **manifest** (the glue declarations) in the user's extensions directory — its own folder `<id>/init.scm` (or a flat `<id>.scm`) — optionally naming a native library via `(lib …)`; the kernel reads the manifest as data and loads the named `.so`/`.dylib` (libloading) for the handlers. A manifest with no `(lib …)` is purely declarative (glue-only). ABI-stable types come from the wasdf-sdk crate | third-party extensions; `exampleext` (manifest + minimal `cdylib`) |

Both kinds implement the same Extension trait.

### Optional extension: the `.scm` manifest + native library

An optional extension is **two paired artifacts** in the extensions directory: a
`.scm` manifest (the *what/when* — declarations, read as data) and the native
library it names (the *how* — handlers). The manifest is the entry point; the
kernel discovers manifests, reads the declarations, and loads the `(lib …)`
library for the handlers.

Each extension gets its **own folder** — `<extensions>/<id>/init.scm` (plus its
library and any assets) — mirroring the bundled `<name>/` layout. A flat
`<extensions>/<id>.scm` is also accepted for a single-file extension.

```scheme
(extension "id"
  (lib "crate_lib_name")            ; bare lib name; loader adds lib…/.so/.dylib
  (commands (…)) (resolvers (…)) (keymaps (…)) (options (…)))  ; all sections optional
```

The `(options …)` section declares configurable settings as
`(option key category kind default label)` forms — the same shape a bundled
extension's `option_scheme` uses. The loader tags them with the extension id, so
they appear **editable + persisted** under `@/settings/Extensions/<id>`, and the
extension reads them back through the shared value snapshot (a
`wasdf_sdk::Settings`) — full settings parity with a bundled extension. Every
registered extension (bundled or optional) also shows an **About** card there
(name / version / description from `Extension::about()`), so a user extension
appears in `@/settings/Extensions` exactly like a bundled one.

The manifest is **data, not code**: it never runs. It is read by the codec's
s-expression reader into palette commands, resolver entries, and Extension-layer
key bindings. A binding's `(ext "id" "intent")` form is linked to Rust at **intent
dispatch over the C ABI** (by id), not by the manifest. A manifest with **no**
`(lib …)` is a purely declarative extension — it wires existing core intents with
no Rust at all.

The reference **`exampleext`** (source: `crates/wasdf-example-ext`) **toggles** a
cursor-info feature with `o`: the first press turns it ON — it pushes an **overlay
mode** (so `wasd` still navigates the file list underneath) and shows the cursor
entry's info as a logical document that *follows the cursor* — and the next press
turns it OFF by popping that mode, so the previewer reclaims the panel (the state
before ON). Every toggle notifies the configured greeting with a `: ON` / `: OFF`
suffix the extension appends from its own state. It owns all of this **entirely in
its own minimal library**, with *no* feature-specific edits to core / the codec /
the keymap. A purely-declarative (scm-only) version is not possible here: glue is
data and cannot *compute* — reading the cursor path, `stat`-ing it, describing the
metadata, and keeping the toggle state is code. So `exampleext` is a small
`cdylib`:

```scheme
(extension "exampleext"
  (lib "exampleext")
  (keymaps ((file file     (("o" (ext "exampleext" "show-info"))))
            (file function (("o" (ext "exampleext" "show-info"))))))
  (options ((option "exampleext.greeting" "exampleext" text "hello" "greeting")
            (option "exampleext.verbose"  "exampleext" bool "#f"    "verbose"))))
```

It rides only **generic ABI seams** — no feature-specific kernel surface. Its
`wasdf_on_cursor_changed` records the path now under the cursor and, *while the
feature is ON*, returns a `show-function-content` reply so the panel follows the
cursor (while OFF it returns nothing). Pressing `o` dispatches
`(ext "exampleext" "show-info")` to its `wasdf_handle_intent`, which flips an
internal ON/OFF flag and returns a `(notify "<greeting>: ON|OFF")` plus, on ON, a
`(push-overlay "exampleext" "info")` and a `show-function-content` reply — a
logical `(document (field …) …)` core lays out (it `stat`s the cursor path and
describes the metadata with `wasdf_sdk::format_size` / `format_permissions`); on
OFF, `pop-mode` (core clears the overlay's content and the previewer reclaims the
panel on the mode-change broadcast). Because ON focuses the function panel, `o` is
bound on both panels so the next `o` resolves regardless of focus (the overlay
falls through to the file bindings). The greeting reaches the library because the
kernel hands every `handle_intent` the extension's own live option values as its
data payload — the `wasdf_sdk::Settings` snapshot (`Settings::from_scheme_alist`)
— so editing `greeting` under settings changes the notice with no rebuild. Deploy
`init.scm` + `libexampleext.{so,dylib}` into `~/.config/wasdf/extensions/exampleext/`;
its options + About appear under `@/settings/Extensions/exampleext`.

> `(notify "text")`, `(push-overlay "ext" "mode")`, and `pop-mode` are generic
> kernel intents (like `show-function-content` / `hide-function-panel`): any
> extension can emit them; they are not tied to `exampleext`. An **overlay** mode
> sits on the mode stack and lets unbound keys fall through to the mode beneath,
> so the file list keeps responding while the overlay owns the panel.

The named library exports these C entry points (string-based bridge — Scheme
datums, no Rust types cross the boundary):

| Symbol | Signature | Role |
|--------|-----------|------|
| `wasdf_abi_version` | `() -> u32` | Must equal `wasdf_sdk::API_VERSION` exactly, else the library is skipped |
| `wasdf_handle_intent` | `(intent, data) -> *const c_char` | Optional. Receives the intent id and a Scheme-datum payload — the intent form's own ExtensionValue when it carried one, otherwise the extension's live option snapshot as a `(("key" "value") …)` alist (read via `wasdf_sdk::Settings::from_scheme_alist`) — and returns follow-up intents as a Scheme list, decoded back into core intents |
| `wasdf_on_cursor_changed` | `(path) -> *const c_char` | Optional. Subscribes to the cursor-changed event: receives the cursor path and returns follow-up intents (same datum form as `wasdf_handle_intent`, e.g. `show-function-content`) — reacts to cursor movement with no kernel edits |
| `wasdf_vfs_list` | `(path) -> *const c_char` | Optional. Gives this extension its own VFS mount at `@/<id>` (no manifest field — the mount name is always the extension's id): receives the virtual path being listed and returns a Scheme list of `(dir "name" "path-segment")` / `(leaf "name" "path-segment" (ext "id" "intent" [data]))` entries |

The bridge carries commands, resolver entries, **keymaps** (entry bindings into
core modes, whose intents are typically `(ext …)` forms; `:when` is limited to
core predicates since a dynamic extension cannot register native predicates),
intent handling, **the cursor-changed event** (`wasdf_on_cursor_changed`),
**overlay modes** (`(push-overlay "ext" "mode")` / `pop-mode` — an overlay sits on
the mode stack and lets unbound keys fall through to the mode beneath), and
**function-panel content**: a `handle_intent` (or `on_cursor_changed`) reply may
include `(show-function-content "owner" (document BLOCK…))` — a **logical**,
layout-free view. Each BLOCK is `(field "label" (SPAN…))`, `(line (SPAN…))`,
`(heading (SPAN…))`, or `(rule)`, and each SPAN is `(span "text")`,
`(span "text" (role "name"))` (themed), or `(span "text" (rgb R G B))` /
`(rgb R G B BR BG BB)` (literal). **Core owns all layout** — label alignment, line
wrapping / h-scroll, vertical scroll, line numbers, theming, and search highlight;
the extension never assembles styled lines. `(update-function-view VALUE)` stores
opaque per-extension view state (a dynamic extension can *write* it but does not
yet *receive* it). Custom panels are not carried across the ABI. `API_VERSION` is
0 (the initial prototype ABI): the glue lives in the `.scm` manifest, and the
library exports only `wasdf_abi_version` plus the optional `wasdf_handle_intent` /
`wasdf_on_cursor_changed` / `wasdf_vfs_list` handlers the manifest's intents
route to.

Dynamic content reaches the panel via this **push** path (`show-function-content`
from `handle_intent` or `on_cursor_changed`). Bundled extensions instead implement
the **pull** hook `view(ctx) -> PanelView` (below); both produce the same logical
`PanelView`, so they share one core layout path — the extension describes *what*,
core renders. Images (`PanelView::Image`, raw RGB) stay a bundled capability since
raw pixels do not cross the ABI; dynamic content is documents of styled spans. A
dynamic `accept_content` (off-thread byte read for a dynamic extension) is
deferred until a dynamic extension reads files.

## Zero-Edit Principle

Adding *another* extension edits nothing in core, app, script, or exec.
It does **not** forbid building the one-time generic framework that hosts
extensions. Acceptance test: searching the tree for an extension's id outside
its own directory matches only the bundled-extension list, and nothing else.

## Loading Rules (optional extensions)

| Rule | Content |
|------|---------|
| Discovery | the kernel scans the extensions directory for manifests — a folder `<id>/init.scm` (its own directory, holding its library + assets) or a flat `<id>.scm`; each may name a library via `(lib …)` |
| ABI | when a manifest names a library, it is loaded only when its API version exactly matches the kernel's; a mismatch is skipped with a warning. A manifest with no `(lib …)` loads no library (purely declarative) |
| Extension id | unique; on collision the later one is disabled with a warning |
| Order | bundled first, then optional in manifest file-name order |
| Reload | unsupported in the MVP |
| Faults | initialization failure or a panic disables the extension and the application continues; out-of-process faults such as a segfault are not survivable |
| Disable | a user option keyed by the extension id prevents loading |

## The Extension Trait

One interface (`extension::Extension`), collected once at registration. Every
method defaults to empty/none, so an extension declares only what it provides.

| Group | Methods | Role |
|-------|---------|------|
| Declarations | id, about, commands, keymaps, scheme_source, resolver_entries, register_conditions | `about` → the name / version / description card shown under `@/settings/Extensions/<id>` (defaults to the id + app version); palette commands; native-built keymaps and entry bindings into core modes (Extension layer); a declarative Scheme source evaluated at registration (its keymap groups merge into the Extension layer — see [SCHEME.md](SCHEME.md)); resolver entries; `:when` predicates |
| Settings | option_scheme | Declares configurable options (`(option …)` forms), tagged with the extension id → editable + persisted under `@/settings/Extensions/<id>`; read back via the shared `OptionStore` (bundled) or a `wasdf_sdk::Settings` snapshot (dynamic) |
| Function-panel content | provides_function_content, accept_content, view | The extension is the active content provider; the kernel reads a path (Read plan) and hands the bytes/entries to `accept_content`, which decodes + stashes them inside the extension; `view(ctx) -> PanelView` returns the stash as a **logical** view (blocks + spans) that **core lays out** — the extension no longer assembles lines |
| Intent handling | handle_intent, on_cursor_changed | `handle_intent` handles an extension intent addressed to this extension; `on_cursor_changed(&AppState)` reacts to the kernel's generic cursor-changed broadcast (every extension is called when the cursor target / panel visibility / **mode** changes). Both return the standard list of follow-up intents — the content provider returns `LoadContent` to follow the cursor; `LoadSearch{owner, root, query}` is `LoadContent`'s deep-search sibling, an owner-addressed `Plan::Search` that completes through the same `accept_content` path (CmdOp's browse-directory search — see below) |
| VFS | vfs_providers | Contributes a `VfsProvider` for this extension's own mount — conventionally named after `id()` — added to the router after the embedded filesystem default. Bundled extensions implement `VfsProvider::list` directly in Rust (e.g. the bookmarks extension listing its saved paths); a dynamic extension gets the same capability for free by exporting `wasdf_vfs_list` (below) — no separate registration path, no core awareness of any specific extension |

`FunctionRenderCtx` carries the viewport size, focus, and the function-panel view
state (scroll, search, line numbers, `ext_view`). An extension returns a logical
`PanelView`; core lays it out into styled lines (label alignment, h-scroll, line
numbers, theming, search highlight) and adds the title + scroll chrome.

Condition predicates are name-plus-native-function pairs registered into the
condition evaluator; they run on the input path and must be cheap. Core
predicates are pre-registered by the kernel; extensions add their own (for
example, the archive extension's archive test, or preview's
`function-searching` / `function-has-matches`) without touching core.

> Future trait surface — custom panels and per-panel renderers — is not
> implemented; today an extension renders into the **function panel** only, via
> `view() -> PanelView` (bundled) or a pushed `PanelView` (dynamic,
> `show-function-content`). Both are the same logical model, laid out by core.

## Extension Intents and ExtensionValue

Extension intents travel as data — the core Intent enum never grows. An
extension intent carries the extension id, an intent id, and a structured
ExtensionValue payload (Nil, Bool, Int, String, Path, List, or string-keyed
Map, with structural accessors; no downcasting). Values are generated directly
from Scheme expressions and consumed structurally.

Two data keys are reserved and have pipeline-level meaning:

| Key | Meaning |
|-----|---------|
| item | Filled by `Mode::InputForm`'s confirm action with the typed text ([UI.md](UI.md)) |
| resolver (with args) | Marks the intent as a plan. It is not dispatched to the extension; the kernel executes it through the resolver chain, with Policy confirmation when the entry is destructive ([ARCHITECTURE.md](ARCHITECTURE.md)) |

An intent without the resolver key is only dispatched to its owning extension
and can merely return more intents; dispatch therefore terminates
structurally, with a re-dispatch depth cap as a backstop.

## Prefer Select or Input Form over Custom Modes

Most extension UIs are pickers or one-line prompts. Neither needs a custom
mode: push Select with a Static source and an on_confirm the reducer already
knows, or — for a single line of free text (a name, a color, an option value)
— push `Mode::InputForm`, whose confirm substitutes the typed text under the
item key. Between the two, extensions never need PendingOp variants. Register
a full custom mode only for UIs that are genuinely not pickers or prompts,
such as a multi-panel dashboard.

## Rust / Scheme Split

Rust is the how: trait implementation, content decode, intent handling, plan
construction, producing a `PanelView`. Scheme is the what and when: keymaps,
conditions, command bindings — declared in the extension's Scheme source and
evaluated at registration (compiled to native tables; never on the input or
render path).

## Function-Panel Content Model

The content frame is the one place an extension renders. The flow:

```
key ──(Extension-layer binding)──▶ Intent::Extension{owner, intent, data}
   ▼ kernel dispatch ──▶ owner.handle_intent(...)            (sync, in extension)
        returns: ShowFunctionContent{owner, view} | UpdateFunctionView(value)
                 | push-overlay / pop-mode | any core intent (refresh, Select, …)
   ▼ reducer stores into FunctionPanelState (content_owner / ext_view; visible; sublayout)
   ▼ render: core lays out the PanelView, applying the kernel-owned view geometry
```

For file-backed content (the cursor-follow), the content provider returns
`LoadContent{owner, path}` from `on_cursor_changed`; the kernel turns it into the
generic `Plan::Read{owner, path}`; the worker reads bytes/entries off-thread;
`Payload::Read` carries `ReadResult::{Bytes, Dir}`; the kernel hands the result
to `owner.accept_content`, which decodes + stashes (main thread). Staleness /
cancellation follow the standard rules (one live `content` task; the cursor-changed
broadcast fires only on an actual change) — see [ARCHITECTURE.md](ARCHITECTURE.md).

`PanelView` is the extension-agnostic **logical** content core lays out:
`Document(Vec<Block>)` — where a `Block` is `Field { label, value }`,
`Line(spans)`, `Heading(spans)`, or `Rule` — or `Image { width, height, rgb }`.
A `TextSpan { text, style }` carries `SpanStyle::{Default, Role(name), Rgb{fg,bg}}`:
the extension tags spans with a theme **role** (re-themes automatically) or a
literal color (syntax highlight); it never wraps, scrolls, numbers, or aligns —
**core owns layout**, including search-match highlight.

### What lives where

| Concern | Home |
|---------|------|
| Panel chrome (border, title, rect), the Exec frame + its scroll | core |
| `PanelView` vocabulary + the layout engine (label alignment, wrap/h-scroll, line numbers, theming, search highlight, image→cells) | core |
| Storing `content_owner` + `ext_view`; routing keys; calling `view()`; off-thread **byte read** + handing bytes to the extension | core |
| Holding the **raw bytes + decoded representation** of the content | extension (interior mutability) |
| Decoding bytes (MIME, syntax highlight) — on the main thread in `accept_content` | extension |
| **Describing** the content as a logical `PanelView` (blocks + role/color spans) | extension (`view()`) |
| The search matcher; cursor-follow policy | extension |
| Key bindings, conditions, params | extension, declared in Scheme |

Core knows the `PanelView` *shape* and how to lay it out (including search
highlight, keyed by block index); the extension owns only *what* to show and the
search *matcher*. **Core stores no content** (no `content_cache`): only
`content_owner` + `ext_view` + the gated match list.

### Two constraints that shape the seam

1. **`:when` predicates read only `&AppState`.** Any state that **gates a key**
   must live in `AppState`. `n`/`p` are gated by has-matches, so the search
   **match list stays in `AppState`** (as generic `PanelSearch`), even though the
   *matcher* and *navigation algorithm* are the extension's. The extension
   computes (in `handle_intent`); the reducer stores the result via the generic
   setter intents (`SetSearchMatches`, `SetScroll`).
2. **The reducer can't call extensions** (`apply_intent` has no registry).
   Extension-owned search/scroll therefore routes through the **kernel**
   (`handle_intent`), which returns generic intents the pure reducer applies.

Consequence of "core holds no content": because the closed `Payload` ferries
only core types, only raw bytes cross the worker→main boundary; the extension
decodes + highlights in `accept_content` on the **main thread**, bounded by the
read limit. Off-thread decode is rejected because it would force core to hold a
decoded `PanelView`.

## Bundled Extensions

**PreviewExtension** provides file preview on the function panel — the reference
"fat" content extension. It owns everything content-specific: decoding the bytes
the kernel reads (MIME dispatch via wasdf-sdk → text / hex / image / dir), syntax
highlighting (syntect, keyed by file extension), the less-style search matcher
(`extension/preview/search.rs`), and its own keybindings + `:when` predicates
declared in `scheme_source` — both in the **function** scope (overriding the core
Enter/Esc/n bindings by layer priority while a search is active) and in the
**file** scope, so the preview can be searched without focusing it: `/` (when the
preview is visible) starts the search and the reducer borrows focus to type, then
returns it to the file panel on submit/cancel where `n`/`p` step the matches
([UI.md](UI.md)). The decoded
content lives **inside the extension** (interior mutability / `RefCell`): the
kernel's Read plan fetches bytes/entries off-thread, hands them to
`accept_content` (decode + stash, main thread), and `view()` later returns the
stash as a logical `PanelView` — text as `Line` blocks of syntax-colored spans,
hex/dir as plain/role-styled lines, images as `PanelView::Image`. Images use
Chafa's symbol-rendering core (chafa-syms-rs): core turns the decoded RGB buffer
into a grid of truecolor symbol cells at layout time.

The *layout* — wrapping/h-scroll, line numbers, theming, and search-match
highlight — is core's generic engine (`ui/middle/function.rs` + `ui/content`,
shared by every content extension); the extension just describes the `PanelView`.
The function panel's *view geometry* (scroll, h-scroll, line numbers) and the
search **match list** stay kernel-owned because the panel is kernel UI
([UI.md](UI.md)), the vertical scroll is shared with the Exec frame, and the match
list must be in `AppState` for `:when`. The extension supplies the keys that emit
the intents and the matcher; the kernel stores the results and paints the matches.

**ArchiveExtension** is fully self-contained pack and unpack. Its Scheme source
declares entry bindings into File mode (pack when a selection exists, unpack
when the cursor entry is an archive) and the matching palette commands. Pack
pushes a `Mode::InputForm` prefilled with `archive.tar.gz` for the destination
name; unpack needs no prompt at all, building its resolver plan directly from
the cursor entry. Either way the final intent carries the resolver key and
executes as a plan through the chain. No custom mode, no custom handler, no
custom panel, and no edits outside its own directory.

**CmdOpExtension** is the reference **form** extension (next to Preview's
reference **content** role): the copy/move/rename/mkdir/touch command
builder, hosted directly in the function panel rather than any popup or
custom mode. It owns its live state in interior mutability (a
`RefCell<Option<Form>>`), draws itself via `render_form(&self, frame, inner,
ctx) -> Option<String>` — a real `&mut Frame` handed to the extension, since
bundled (statically linked) extensions can take one; the returned string
becomes the panel's title, core still owns the border/chrome — and reads its
browse directory through the generic `LoadContent` → `accept_content` path
used by content extensions too. `AppState.function.form_owner` names which
extension (if any) is hosting the panel this way; `Intent::SetFunctionForm
(Some(id)/None)` claims/releases it (shows + focuses the panel on claim,
returns focus to the file panel on release). A `:when` predicate reading only
`form_owner` (`"cmdop-active"`) gates the extension's own keybindings,
declared in its own `scheme_source` (`extension/cmdop/keymap.scm`) at the
Extension layer in the **file function** scope — high enough priority to
override the core "file function" scope's own bindings for the same keys
while the form is active. Copy/move/rename deliberately keep `wasd` on the
core file-cursor bindings even so (using `hjkl` for their own
builder/listing navigation instead), so the sources they're building a
command for keep following the file cursor.

Its destination browser's `/` is **two-stage**, deep-then-shallow (see
[UI.md](UI.md)'s "Deep search vs. shallow filter"): the first `/` in a freshly
browsed directory opens a submit-only input (`purpose: "search-deep"`) and, on
submit, emits `Intent::LoadSearch{owner: ID, root: list_cwd, query}` — a
recursive walk of the browsed subtree, ranked the same way file-search ranks
its results. Landing entries (`accept_content`, gated the same as any other
content load by `list_cwd == path`) flips `Form.deep_results`, and every `/`
after that opens today's live substring filter (`purpose: "filter"`) over
those landed results instead of issuing another walk. Changing the browsed
directory (`descend`/`parent`) resets `deep_results`/`awaiting_deep` and
clears `deep_query`, so `/` goes back to deep the moment the subtree it would
search changes. `Form.deep_query` is display-only — the title shows
`"<op> — search: <query>"` while deep results are active — and is kept
separate from `filter_query` on purpose: reusing one field would re-narrow
fuzzy-ranked deep results by strict literal substring, silently hiding
matches whose names don't contain it verbatim.

Its **create** mode (`M`, combining mkdir and touch) is shaped differently:
no destination browser, no options — every name is created directly in
`state.cwd` — so `start_create` skips `start`'s shared setup entirely and
opens a **multi-line** `FunctionInput` (`purpose: "create-lines"`,
`multiline: true`) instead. `multiline` is a small generic addition to
`Intent::BeginInput`/`FunctionInput` (not a `create`-specific hack): when
set, Enter inserts `\n` unless the line the caret is on is already empty, in
which case it submits — a blank line terminates the batch, the free-text
analogue of `InputKind::List`'s row-add/row-confirm split. `create_targets`
parses the submitted buffer once non-blank lines are extracted: a trailing
`/` picks `mkdir -p NAME` (slash stripped), otherwise `touch NAME`; `run_
create` emits one `Intent::RunResolver` per line (no single resolver op
covers "one or the other, per line"). `create_preview` (the same line
parser, so the preview can never disagree with what actually runs) drives
the same CLI-builder area copy/move/rename use for their own resolved
command line, so typing shows the exact `mkdir`/`touch` invocations about to
run. The palette's `mkdir`/`touch` commands both point at `start-create`
too — one combined flow accessible under either familiar name.

**ThemeExtension**'s per-role editor is **cursor-follow**, not an explicit
open/confirm action: landing the file cursor on a custom theme's role leaf
(`@/settings/Display/Themes/<theme>/<role>`) calls `on_cursor_changed`, which
seeds a `ColorEdit { theme_name, role_tok, fg_rgb, fg_nil, bg_rgb, bg_nil,
mods, channel }` in its own `RefCell` from the role's *resolved* fg, bg, and
three style modifiers, and hosts the panel via
`Intent::HostFunctionForm(Some("theme"))` — the **passive** sibling of
`SetFunctionForm` (below) that shows the panel without moving focus, so the
file panel keeps browsing with `wasd` while `hjkl` edits whatever role is
currently under the cursor. Moving the cursor off a role (a preset's role, a
non-role leaf, another category) releases it the same way, mirroring how the
content provider's own cursor-follow drops a stale preview. A role's fg, bg,
and its style are **one setting**, edited together: every role carries both a
foreground and a background — `Color::Reset` is "nil", literally no color, not
merely "unset" — plus Bold/Italic/Underline. `channel` walks eleven focusable
controls **top-to-bottom, matching what's drawn**: an "FG nil" toggle
(`FG_NIL_CHANNEL`), fg R/G/B (`FG_CHANNEL_BASE..`), the three modifiers
(`MOD_CHANNEL_BASE..`), a "BG nil" toggle (`BG_NIL_CHANNEL`), then bg R/G/B
(`BG_CHANNEL_BASE..=MAX_CHANNEL`) — so there is no separate leaf per attribute
to fall out of sync with the rest, and `j`/`k` always moves to the next
control actually visible below the current one. A nil toggle doesn't discard
the RGB sliders underneath it; it only decides whether they're *applied* —
toggling nil off again restores whatever the sliders last held. Its
`render_form` draws, in that same order: the "FG nil" row, a horizontal
`BarChart` (fg R/G/B, the focused channel bold/underlined), three stacked
`Name      [x]`/`Name      [ ]` modifier rows — left-padded to a fixed label
width so every `[ ]` lines up in a column — a blank separator row, the "BG
nil" row, a second `BarChart` (bg R/G/B), and a live one-line swatch (a
`Paragraph` styled with the role's actual resolved `fg`/`bg`/modifiers, not a
blank color block, so what's shown is exactly what the role will look like)
directly into the panel's inner rect, degrading gracefully (each section only
draws if there's room) in a short panel. Its
own `extension/theme/keymap.scm`, gated by a `"theme-color-active"` predicate
(`form_owner == Some("theme")`), binds `hjkl` **in the file panel's own
scope** (not `function` — the function panel has nothing else to scroll while
showing this editor, so repurposing those keys is the point, the same way
`FuncUp`/`FuncDown`/`FuncLeft`/`FuncRight` already let the file panel drive
the function panel without a focus switch, just read-only): `j`/`k` (or
Down/Up) move the focused control, `h`/`l` (or Left/Right) step it by 1 (or
flip the focused toggle — a checkbox has no continuous value), `H`/`L` step
by 10 (or also flip it). **There is no confirm or cancel** — every keypress
writes the literal `#rrggbb` (or `nil`) and every modifier flag straight
through via `apply_role` and persists immediately; the editor never buffers a
value the real theme doesn't already have. An edit that lands exactly back on
`registry::default_style(role)` — the role's built-in look, `DEFAULT_BOLD_ROLES`
included — removes the override entirely rather than storing a redundant one.
`edit-role` (the intent a leaf's `Enter` activation still carries) calls the
same seed-and-host logic and is kept only as an idempotent manual fallback —
cursor-follow is the primary path.

On disk, a role's whole override is always one `define-theme` line, the full
`(role fg bg bold italic underline)` shape — `fg`/`bg` a quoted color token or
the bare symbol `nil` for "no color" (`Color::Reset`), the three modifiers
`#t`/`#f` — e.g. `(entry-dir "#83a598" nil #t #f #f)`. There is no
partial/merged/standalone form the way bold alone once had: a role's whole
look is one setting, written and parsed as one line
(`script::config::parse_role_style_overrides` / `ThemeWriter::upsert_theme`).

**Base and the nil fallback chain.** `@/settings/Display/Themes/*/base`
(`Role::Base`) is what the whole frame is painted with before any more
specific role draws on top (see `ui/render.rs`) — its `fg` and `bg` are the
base pair every other role falls back onto. Unlike every other role, where
`bg` only ever means "this role's own background", `Base`'s `bg` slot *is*
the fallback background directly — used as-is, not borrowed from anywhere
else, so editing it in the cursor-follow editor (fg R/G/B, bg R/G/B) needs no
special-casing there either. Every other role's `Theme::style`
(`core/theme.rs`'s `resolved_fg_bg`) falls back onto base wherever it's nil:

- A role's own explicit (non-nil) fg or bg always wins outright.
- A nil bg falls back to base's bg.
- A nil fg falls back to base's bg **if** the role set its own bg (not nil) —
  a "knockout" look, text reading in base's bg color against the role's own
  background, like it was punched out of it. This is the replacement for the
  old reverse-video "Negative" toggle: the same badge/chip look, built from
  two real colors instead of a modifier.
- Otherwise (the role's bg is nil too) a nil fg falls back to base's plain fg.
- If the fallback itself lands on nil (base has no override of its own
  either), the result stays nil — the terminal's own color shows through,
  exactly as it always did.

`Role::Base` terminates the chain: asking for its own style resolves straight
to its own fg/bg, no fallback.

## Registration

At boot the kernel, for each extension, merges its `keymaps()` and its
`scheme_source` keymap groups into the Extension layer (collision detection +
user override), appends its palette commands, resolver entries, and VFS
providers, and registers its `:when` predicates; then
`ExtensionRegistry::register` stores the extension.
The active **content provider** (first `provides_function_content`) receives
cursor-follow Read results via `accept_content` and is asked for its `view()`
when the function panel draws. Extension dispatch in the pipeline routes
extension intents to their owner by id.
