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
| Bundled | Statically linked; named only in the bundled-extension list inside the extension facade | PreviewExtension, ArchiveExtension |
| Optional | A `.scm` **manifest** (the glue declarations) in the user's extensions directory that names a native library via `(lib …)`; the kernel reads the manifest as data and loads the named `.so`/`.dylib` (libloading) for the handlers. ABI-stable types come from the wasdf-sdk crate | third-party extensions |

Both kinds implement the same Extension trait.

### Optional extension: the `.scm` manifest + native library

An optional extension is **two paired artifacts** in the extensions directory: a
`.scm` manifest (the *what/when* — declarations, read as data) and the native
library it names (the *how* — handlers). The manifest is the entry point; the
kernel discovers `.scm` files, reads the declarations, and loads the `(lib …)`
library for the handlers.

```scheme
(extension "id"
  (lib "crate_lib_name")            ; bare lib name; loader adds lib…/.so/.dylib
  (commands (…)) (resolvers (…)) (keymaps (…)))   ; all sections optional
```

The manifest is **data, not code**: it never runs. It is read by the codec's
s-expression reader into palette commands, resolver entries, and Extension-layer
key bindings. A binding's `(ext "id" "intent")` form is linked to Rust at **intent
dispatch over the C ABI** (by id), not by the manifest. A manifest with **no**
`(lib …)` is a purely declarative extension — it wires existing core intents with
no Rust at all.

The named library exports these C entry points (string-based bridge — Scheme
datums, no Rust types cross the boundary):

| Symbol | Signature | Role |
|--------|-----------|------|
| `wasdf_abi_version` | `() -> u32` | Must equal `wasdf_sdk::API_VERSION` exactly, else the library is skipped |
| `wasdf_handle_intent` | `(intent, data) -> *const c_char` | Optional. Receives the intent id and its ExtensionValue payload (as a Scheme datum) and returns follow-up intents as a Scheme list, decoded back into core intents |
| `wasdf_on_cursor_changed` | `(path) -> *const c_char` | Optional. Subscribes to the cursor-changed event: receives the cursor path and returns follow-up intents (same datum form as `wasdf_handle_intent`, e.g. `show-function-content`) — reacts to cursor movement with no kernel edits |

The bridge carries commands, resolver entries, **keymaps** (entry bindings into
core modes, whose intents are typically `(ext …)` forms; `:when` is limited to
core predicates since a dynamic extension cannot register native predicates),
intent handling, **the cursor-changed event** (`wasdf_on_cursor_changed`), and
**function-panel content**: a `handle_intent` (or `on_cursor_changed`) reply may
include `(show-function-content "owner" (lines ("text" RUN…)…))` where
`RUN = (LEN R G B)` (foreground) or `(LEN R G B BR BG BB)` (with a background),
which the kernel decodes into `PanelContent` and renders generically (styled
text; images are bundled-only — raw RGB does not cross the ABI), and
`(update-function-view VALUE)`, which stores opaque per-extension view state.
A dynamic extension can *write* that view state but does not yet *receive* it as
input (the bridge passes only the intent id and its data). Custom modes and
panels are not carried across the ABI. `API_VERSION` is 0 (the initial prototype
ABI): the glue lives in the `.scm` manifest, and the library exports only
`wasdf_abi_version` plus the optional `wasdf_handle_intent` /
`wasdf_on_cursor_changed` handlers the manifest's intents route to.

Dynamic content reaches the panel via this **push** path (`show-function-content`
from `handle_intent` or `on_cursor_changed`). The **pull** render hook (`render_function`, below) is a
bundled optimization for viewport-windowing large content; exposing it across
the ABI — passing `ext_view` into a per-frame dynamic call — is deferred as
speculative until a dynamic extension genuinely needs viewport-aware rendering.
Likewise a dynamic `accept_content` (off-thread byte read for a dynamic
extension) is deferred until a dynamic extension reads files. Images stay a
bundled capability (raw RGB does not cross the ABI; dynamic content = styled
text lines only).

## Zero-Edit Principle

Adding *another* extension edits nothing in core, app, script, or exec.
It does **not** forbid building the one-time generic framework that hosts
extensions. Acceptance test: searching the tree for an extension's id outside
its own directory matches only the bundled-extension list, and nothing else.

## Loading Rules (optional extensions)

| Rule | Content |
|------|---------|
| Discovery | the kernel scans the extensions directory for `.scm` manifests (not libraries); each names its library via `(lib …)` |
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
| Declarations | id, commands, keymaps, scheme_source, resolver_entries, register_conditions | Palette commands; native-built keymaps and entry bindings into core modes (Extension layer); a declarative Scheme source evaluated at registration (its keymap groups merge into the Extension layer — see [SCHEME.md](SCHEME.md)); resolver entries; `:when` predicates |
| Function-panel content | provides_function_content, accept_content, render_function | The extension is the active content provider; the kernel reads a path (Read plan) and hands the bytes/entries to `accept_content`, which decodes + stashes them inside the extension; `render_function(ctx) -> FunctionDraw` draws the stash for the viewport |
| Intent handling | handle_intent, on_cursor_changed | `handle_intent` handles an extension intent addressed to this extension; `on_cursor_changed(&AppState)` reacts to the kernel's generic cursor-changed broadcast (every extension is called when the cursor target / panel visibility changes). Both return the standard list of follow-up intents — the content provider returns `LoadContent` to follow the cursor |

`FunctionRenderCtx` carries the viewport size, focus, and the function-panel view
state (scroll, search, line numbers, `ext_view`); `FunctionDraw` carries the
styled lines + title + scroll metadata + optional search prompt that core blits.

Condition predicates are name-plus-native-function pairs registered into the
condition evaluator; they run on the input path and must be cheap. Core
predicates are pre-registered by the kernel; extensions add their own (for
example, the archive extension's archive test, or preview's
`function-searching` / `function-has-matches`) without touching core.

> Future trait surface — custom modes/handlers, panels, and per-panel renderers
> — is not implemented; today an extension renders into the **function panel**
> only, via `render_function` (bundled) or pushed `PanelContent` (dynamic).

## Extension Intents and ExtensionValue

Extension intents travel as data — the core Intent enum never grows. An
extension intent carries the extension id, an intent id, and a structured
ExtensionValue payload (Nil, Bool, Int, String, Path, List, or string-keyed
Map, with structural accessors; no downcasting). Values are generated directly
from Scheme expressions and consumed structurally.

Two data keys are reserved and have pipeline-level meaning:

| Key | Meaning |
|-----|---------|
| item | Filled by the Emit confirm action with the confirm shape ([UI.md](UI.md)): a single item, a list of marked items, or the confirmed text |
| resolver (with args) | Marks the intent as a plan. It is not dispatched to the extension; the kernel executes it through the resolver chain, with Policy confirmation when the entry is destructive ([ARCHITECTURE.md](ARCHITECTURE.md)) |

An intent without the resolver key is only dispatched to its owning extension
and can merely return more intents; dispatch therefore terminates
structurally, with a re-dispatch depth cap as a backstop.

## Prefer Select over Custom Modes

Most extension UIs are pickers. Those need no custom mode: push Select with a
Static source and an Emit confirm action; the chosen item or entered text is
substituted under the item key. Emit with Path input also covers extension
path and text entry, so extensions never need PendingOp variants. Register a
full custom mode only for UIs that are genuinely not pickers, such as a
multi-panel dashboard.

## Rust / Scheme Split

Rust is the how: trait implementation, content decode, intent handling, plan
construction, producing `PanelContent`. Scheme is the what and when: keymaps,
conditions, command bindings — declared in the extension's Scheme source and
evaluated at registration (compiled to native tables; never on the input or
render path).

## Function-Panel Content Model

The content frame is the one place an extension renders. The flow:

```
key ──(Extension-layer binding)──▶ Intent::Extension{owner, intent, data}
   ▼ kernel dispatch ──▶ owner.handle_intent(...)            (sync, in extension)
        returns: ShowFunctionContent{owner, content} | UpdateFunctionView(value)
                 | any core intent (refresh, push Select, resolver plan, …)
   ▼ reducer stores into FunctionPanelState (content_owner / ext_view; visible; sublayout)
   ▼ render: core blits PanelContent, applying the kernel-owned view geometry
```

For file-backed content (the cursor-follow), the content provider returns
`LoadContent{owner, path}` from `on_cursor_changed`; the kernel turns it into the
generic `Plan::Read{owner, path}`; the worker reads bytes/entries off-thread;
`Payload::Read` carries `ReadResult::{Bytes, Dir}`; the kernel hands the result
to `owner.accept_content`, which decodes + stashes (main thread). Staleness /
cancellation follow the standard rules (one live `content` task; the cursor-changed
broadcast fires only on an actual change) — see [ARCHITECTURE.md](ARCHITECTURE.md).

`PanelContent` is the extension-agnostic drawable the kernel blits dumbly:
`Lines { lines, styles }` (where `styles[i]` tiles `lines[i]` with `StyleRun`s)
or `Image { width, height, rgb }`. `StyleRun { len, fg:(u8,u8,u8),
bg:Option<(u8,u8,u8)> }` — the extension owns *all* color (search highlight
included); core never adds color.

### What lives where

| Concern | Home |
|---------|------|
| Panel chrome (border, title, rect), the Exec frame + its scroll | core |
| `PanelContent` vocabulary + a dumb blitter (StyleRun→Span, image→cells) | core |
| Storing `content_owner` + `ext_view`; routing keys; calling the render hook; off-thread **byte read** + handing bytes to the extension | core |
| Holding the **raw bytes + decoded representation** of the content | extension (interior mutability) |
| Decoding bytes (MIME, syntax highlight) — on the main thread in `accept_content` | extension |
| Rendering the content for a viewport (windowing, syntax color, search highlight, h-scroll, line numbers, prompt) | extension (render hook) |
| The search matcher; cursor-follow policy | extension |
| Key bindings, conditions, params | extension, declared in Scheme |

Core knows the `PanelContent` *shape* and how to paint it; it knows nothing
about *search*, *scroll*, or *highlight* per se — those are reflected in what
the extension's render hook returns. **Core stores no content** (no
`content_cache`): only `content_owner` + `ext_view` + the gated match list.

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
`PanelContent`.

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
`accept_content` (decode + stash, main thread), and `render_function` later draws
the stash for the viewport. Images use Chafa's symbol-rendering core
(chafa-syms-rs): a decoded RGB buffer becomes a grid of truecolor symbol cells.

The line/image *painting* is the kernel's generic blitter (`ui/content`, shared
by every content extension); the extension just feeds it `PanelContent`. The
function panel's *view geometry* (scroll, h-scroll, line numbers) and the search
**match list** stay kernel-owned because the panel is kernel UI ([UI.md](UI.md)),
the vertical scroll is shared with the Exec frame, and the match list must be in
`AppState` for `:when`. The extension supplies the keys that emit the intents and
the matcher; the kernel stores the results.

**ArchiveExtension** is fully self-contained pack and unpack. Its Scheme source
declares entry bindings into File mode (pack when a selection exists, unpack
when the cursor entry is an archive) and the matching palette commands. Its flow
uses two Select instances in sequence — a Static format picker, then a path
input — both confirmed through Emit; the final intent carries the resolver key
and executes as a plan through the chain. No custom mode, no custom handler, no
custom panel, and no edits outside its own directory.

## Registration

At boot the kernel, for each extension, merges its `keymaps()` and its
`scheme_source` keymap groups into the Extension layer (collision detection +
user override), appends its palette commands and resolver entries, and registers
its `:when` predicates; then `ExtensionRegistry::register` stores the extension.
The active **content provider** (first `provides_function_content`) receives
cursor-follow Read results via `accept_content` and is asked to `render_function`
when the function panel draws. Extension dispatch in the pipeline routes
extension intents to their owner by id.
