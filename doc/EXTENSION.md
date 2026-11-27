# Extensions

Features beyond the minimal filer are implemented as extensions. The kernel
boots with zero extensions registered: browsing, selection, file operations
(the built-in CmdOp builder for copy / move / rename / create, and delete via
the resolver), search, the palette, and the confirmation dialog all work; the prop panel shows
the cursor entry's metadata ([UI.md](UI.md)), and the function panel shows a
placeholder.

## Guiding Goal — thin core, fat extension

> Minimize core's responsibility and size; give extensions the *large*
> responsibility. Core is a thin host; the extension is where behavior lives.
> Anything that *can* live in the extension *should*.

A feature added as an extension — written against `wasdf-sdk`, built as a
dynamic library and dropped into the extensions directory alongside its `.scm`
manifest — with **zero edits to `crates/wasdf/src/`**:

1. **joins the processing pipeline** (emits / handles intents, loads content),
2. **receives key input** (binds keys that route to it), and
3. **renders inside the function panel** (and only there).

Bundled extensions use the same mechanism; `PreviewExtension` is the reference
client.

The function panel **chrome** (border, title placement, the panel rect, the
Exec frame) is the kernel's. The panel **content** — what is drawn, its
appearance, search, paging — is the owning extension's. Core hosts, lays out,
routes keys, reads bytes off-thread, and stores the gated match list.

## Zero-Edit Principle

Adding an extension edits nothing in core, app, script, services, ui, exec, or
vfs — including the embedded `.scm` sources under `app/`. No core module names
an extension's id, one of its intents or options, or a path under its mount.
This applies to every bundled extension without exception: none is
constructed, registered, or parsed for by the kernel, and core palette
commands and bindings reach only core intents.

### Coupling between extensions

An extension may use another extension through exactly two channels, and
nothing else — no Rust type, function, VFS path, or form of the other
extension:

| Channel | Form | When the other extension is absent |
|---------|------|------------------------------------|
| Option key | read `<id>.<key>` from the OptionStore | the key is undeclared and reads as its type's empty value (`false`, `""`, `0`); the reader treats that as "not available" |
| Intent name | emit `(ext "<id>" "<intent>" [data])` | an intent addressed to an unregistered owner is dropped (debug log only) |

The consumer checks the option before emitting the intent, so it never relies
on the drop, and it falls back to its own behavior when the other extension is
absent. The data carried by such an intent is part of the receiving
extension's documented contract.

Core may reach an extension the same way only through **user data**: a core
option whose value is an extension id (e.g. `cmdop.runner` — [CMDOP.md](CMDOP.md))
makes core emit `(ext "<id>" …)` against a contract core defines. The id is
never written in core's sources or defaults.

Shared seams that are not another extension's code — the Extension trait,
`Host`, `SettingsNode`, core intents — are core surface, not coupling. A
settings node never hardcodes where it is mounted: the host of the settings
tree hands each node its base path.

**Acceptance test.** Searching the tree for an extension's id outside its own
directory matches only (a) the bundled-extension list and (b) option keys
`"<id>.…"` and `(ext "<id>" …)` intent forms inside other extensions'
directories.

## Kinds

| Kind | Loading | Examples |
|------|---------|----------|
| Bundled | Statically linked; named only in `extension::bundled()` | preview, archive, pty, bookmarks, lscolors, theme, settings, repl |
| Optional | A `.scm` manifest in the user's extensions directory — `<id>/init.scm` (or a flat `<id>.scm`) — optionally naming a native library via `(lib …)`. ABI-stable types come from `wasdf-sdk` | third-party extensions; `exampleext` (`crates/wasdf-example-ext`) |

Both kinds implement the same Extension trait (a dynamic library is wrapped in
`loader::DynamicExtension`).

### Optional extension: manifest + native library

```scheme
(extension "id"
  (lib "crate_lib_name")            ; bare lib name; the loader adds lib…/.so/.dylib
  (commands (…)) (resolvers (…)) (keymaps (…)) (options (…))   ; all sections optional
  (prelude "prelude.scm"))          ; optional Scheme code for the session
```

The manifest is **data, not code**: it is read by the s-expression reader, never
evaluated, and decoded by the same core config decoders the embedded sources use
into palette commands, resolver entries, Extension-layer key bindings, and
option declarations. `(options …)` uses the shared
`(option key category kind default label)` form; the options appear editable and
persisted under `@/settings/Extensions/<id>` next to the extension's About card.
A binding's `(ext "id" "intent")` form is linked to the library at dispatch
time, by id. A manifest with no `(lib …)` is purely declarative: it wires
existing core intents only. Its `:when` clauses are limited to core predicates
(including `(option "key")`, which covers its own options).

The library exports these C entry points (string-based: Scheme datums cross the
boundary, never Rust types):

| Symbol | Signature | Role |
|--------|-----------|------|
| `wasdf_abi_version` | `() -> u32` | Must equal `wasdf_sdk::API_VERSION` exactly, else the library is skipped |
| `wasdf_handle_intent` | `(intent, data, options) -> *const c_char` | Optional. `data` is the intent's own ExtensionValue (`()` when it carried none); `options` is always the extension's live option snapshot as an alist (`wasdf_sdk::Settings::from_scheme_alist`). Returns follow-up intents as a Scheme list |
| `wasdf_on_cursor_changed` | `(path) -> *const c_char` | Optional. The cursor-changed broadcast; returns follow-up intents |
| `wasdf_vfs_list` | `(path) -> *const c_char` | Optional. The extension's own mount at `@/<id>`; returns `(dir "name" "segment")` / `(leaf "name" "segment" (ext "id" "intent" [data]))` entries |

Entry points are defined through the SDK's export wrapper, which catches a
panic inside the library and returns a null pointer. A reply of `()` and a
null reply both mean "nothing"; a null reply is additionally recorded in
`@/messages` (naming the extension and the entry point), so a panicking library
is visible without being treated differently. The reply string stays valid
until the next call into the library.

Replies may contain any core intent in datum form plus the generic ones below;
none is specific to an extension:

| Datum | Meaning |
|-------|---------|
| `(show-function-content "owner" (document BLOCK…))` | set the extension's content: the loader keeps the document inside the extension's wrapper (its `view()`) and emits `ShowFunctionContent{owner}`. `owner` must be the replying extension's own id; any other is ignored with an `@/messages` line |
| `(push-overlay "ext" "mode")` / `pop-mode` | an overlay mode lets unbound keys fall through to the mode beneath |
| `(notify "text")` | log a line to `@/messages` |

Each BLOCK is `(field "label" (SPAN…))`, `(line (SPAN…))`, `(para (SPAN…))`,
`(heading (SPAN…))`, or `(rule)`; each SPAN is `(span "text")`, `(span "text" (role "name"))`, or
`(span "text" (rgb R G B))` / `(rgb R G B BR BG BB)`.

There is one content path for every extension: core **pulls** `view()`. A
dynamic extension sends its content in a reply; the loader's wrapper stores it
and serves it as that extension's `view()`, so core never holds content.
Images, interactive forms, the `Host`, native predicates, `accept_content`,
`ext_view`, and settings nodes do not cross the ABI.

`API_VERSION` is 1.

`exampleext` is the reference optional extension: `o` toggles a cursor-info
overlay (`push-overlay` + `show-function-content` that follows the cursor via
`wasdf_on_cursor_changed`), notifying its configurable greeting on each toggle.

## Loading Rules (optional extensions)

| Rule | Content |
|------|---------|
| Discovery | the extensions directory (`$WASDF_EXTENSIONS_DIR`, else `<config>/extensions`): folders `<id>/init.scm`, or flat `<id>.scm` |
| ABI | a named library loads only when its API version matches exactly; otherwise the extension is skipped with a warning |
| Extension id | unique; on collision the later one is disabled with a warning |
| Order | bundled first, then optional in manifest file-name order |
| Reload | unsupported |
| Faults | a null reply is a no-op recorded in `@/messages`; the extension stays registered and the application continues. Load failures (missing library, ABI mismatch, malformed manifest) skip the extension with a warning. Out-of-process faults (a segfault) are not survivable |
| Disable | the core option `extensions.disabled` (a list of ids), set by the enable toggle under `@/settings/Extensions/<id>` and persisted in `custom.scm`; it takes effect at the next start. It is read from `custom.scm` before any extension is constructed, so only the `custom` layer counts — a `config.scm` or REPL write to it is reported and has no effect on loading |

## The Extension Trait

One interface, collected once at registration. Every method defaults to
empty / none, so an extension declares only what it provides.

| Group | Methods | Role |
|-------|---------|------|
| Identity | id, about | `about` is the name / version / description card under `@/settings/Extensions/<id>` |
| Declarations | commands, keymaps, scheme_source, option_scheme, prelude, resolver_entries, vfs_providers, register_conditions, settings_nodes | Collected once at registration. `scheme_source` is a quoted keymap-group form merged into the Extension layer; `option_scheme` declares options (including structured `record` / `map` kinds — the extension's whole configurable state); `prelude` is Scheme code loaded into the session to give users convenience procedures ([SCHEME.md](SCHEME.md), Preludes); `vfs_providers` contributes the extension's own mount (named after its id); `register_conditions` adds native `:when` predicates (they read `AppState` and the OptionStore — [SCHEME.md](SCHEME.md)); `settings_nodes` contributes `SettingsNode`s — rich subtrees anchored to a settings category or to the extension's own settings screen |
| Host | bind | Called once after every extension is registered, with the `Host` (below) |
| Function-panel content | provides_function_content, accept_content, view | The content provider (the lowest-priority-rank extension with `provides_function_content` — [UI.md](UI.md), Extension priority) is the initial content owner. `accept_content(source, &Content)` receives every result addressed to the extension — `Bytes` / `Dir` from a Read or an owner-addressed Search, `Value` from an Eval (`source` is the path, the search root, or empty for an evaluation). `view(ctx) -> Option<Arc<PanelView>>` is asked of the content owner only |
| Function-panel form | render_form, form_scroll | While `form_owner` names the extension it draws directly into the panel's inner rect (a real `&mut Frame`) and returns the title — a form takes precedence over the content owner's view; `form_scroll` reports `(total rows, position)` so the form gets the ordinary scrollbar and clamp |
| Behavior | handle_intent, on_cursor_changed, on_tick | `handle_intent` receives intents addressed to this extension; `on_cursor_changed` the generic broadcast; `on_tick` runs after every event, for out-of-band state. All return follow-up intents |
| File list | entry_style | Per-entry style override, given the entry and the frame's `Theme`; the lowest-priority-rank `Some` wins. Called per visible entry per frame — must be O(1) (see [ARCHITECTURE.md](ARCHITECTURE.md), Render Contract) |

Hook rules:

- Hooks are synchronous and never block. Work that needs I/O is expressed as
  intents that become plans (LoadContent, LoadSearch, RunResolver, …). To
  change a configurable value — its own options included — an extension
  returns a `Configure` intent; the store validates, persists, and updates
  the projections. A file the extension owns goes to `Host.files`. The only exceptions are the pty extension's, listed in
  [ARCHITECTURE.md](ARCHITECTURE.md) (Purity Contract).
- `view()` returns the **same** `Arc` until the content changes; the Arc's
  identity is the revision core caches layout against. Build a new Arc in
  `accept_content` (or wherever the content changes), not in `view()`. The
  view never depends on the viewport: wrapping, alignment, and match → row
  mapping are core's.
- `on_cursor_changed` fires only when the cursor signature changes;
  `on_tick` fires on every pass and must return nothing when there is nothing
  to say.

### Host

`Host` is the kernel's handle given to bundled extensions in `bind`:

| Field | Use |
|-------|-----|
| options | `Arc<OptionStore>` — the projection of the settings store: read options (own or, per the coupling rule, another's); observe the generation. Read-only: writes are `Configure` intents |
| events | `Sender<AppEvent>` — wake the loop with `AppEvent::Tick` when out-of-band state changes (a reader thread) |
| resolver | `Arc<ResolverChain>` — resolve command templates and options for a command builder |
| theme | ThemeCell — publish the active `Theme` |
| files | FileWriter — write-behind for a file the extension owns under the config directory |
| extensions | the registered extensions' ids and About cards |
| settings_nodes | the settings nodes every extension contributed |

A dynamic extension has no Host; it receives its option snapshot as the
`options` argument of every `wasdf_handle_intent` call.

### Input capture

An extension that needs every key (a terminal) installs an `InputCapture` via
`SetInputCapture`:

| Field | Meaning |
|-------|---------|
| owner | the extension receiving captured keys as `"key"` intents (data: the lossless key encoding `codec::encode_key`) |
| release | the hard release key. Core clears the capture on it unconditionally, then sends `"prefix"` to the owner |
| escape | intents that also end the capture: a key the keymap resolves to one of them clears the capture and the intent is dispatched normally |

The escapes are data held by core, not a protocol the extension implements, so
they work even when the extension is wedged; declaring them as intents means
they follow the user's keymap. The `input-captured` predicate lets keymaps gate
on a capture, and the help row renders the capture's own keys ([HELP.md](HELP.md)).

## Extension Intents and ExtensionValue

Extension intents travel as data: the extension id, an intent id, and a
structured ExtensionValue payload (Nil, Bool, Int, String, Path, List, or
string-keyed Map, with structural accessors; no downcasting).

Two data keys are reserved:

| Key | Meaning |
|-----|---------|
| item | Filled by `Mode::InputForm`'s confirm with the entered value ([UI.md](UI.md)) |
| resolver | Marks the intent as a plan. It is not dispatched to the extension; the kernel executes it through the resolver chain, with Policy confirmation when required ([ARCHITECTURE.md](ARCHITECTURE.md)) |

An intent without the resolver key is dispatched to its owner and can only
return more intents; dispatch terminates structurally, with a depth cap as a
backstop.

## Prefer Select or Input Form over Custom Modes

Most extension UIs are pickers or one-value prompts. Push Select with a Static
source, or `Mode::InputForm` for a single value (Text / Bool / Choice / Int /
List), whose confirm re-injects the next intent with the value under `item`.
Register a custom mode only for UIs that are genuinely neither. A UI that must
stay live beside the file list is a function-panel form (`SetFunctionForm` /
`HostFunctionForm` + `render_form`).

## Rust / Scheme Split

Rust is the how: trait implementation, decode, intent handling, plan
construction, producing a `PanelView`. Scheme is the what and when: keymaps,
conditions, commands, options, presets — declared as data and decoded at
registration into native tables, never consulted on the input or render path.

- Strings are read into `Datum`s only by `script::sexpr`, and intents / keys /
  modes are encoded and decoded only by `script::codec`.
- An extension's **own** data (presets) is interpreted from `Datum` by the
  extension itself, inside its directory. `script` holds no
  extension-specific decoder.
- An extension's configurable state is its options, held in the settings
  store like every other setting. Users change them in `config.scm` (possibly
  per environment), at the REPL, or through the UI; the extension changes them
  with `Configure`. Nothing but the Persister writes `custom.scm`, and
  nothing in wasdf writes `config.scm` after the template.
- A prelude may wrap the store procedures in friendlier ones for users
  (theme's `define-theme` / `set-theme`); it is sugar over options, never a
  separate configuration form.

## Function-Panel Content Model

```
key ──(Extension-layer binding)──▶ Intent::Extension{owner, intent, data}
   ▼ kernel ──▶ owner.handle_intent(...)                         (sync)
        returns: LoadContent | ShowFunctionContent{owner} | UpdateFunctionView
                 | push-overlay / pop-mode | any core intent
   ▼ reducer stores view state (content_owner, ext_view, scroll, search)
   ▼ render: core pulls content_owner's view() and lays it out in the chrome
```

For file-backed content the provider returns `LoadContent{owner, path, offset}`
from `on_cursor_changed`; the worker reads one bounded chunk; the kernel hands
the `Content` to `owner.accept_content`, which decodes and stashes it (main
thread) and pages further chunks by returning LoadContent with the next offset.
A provider declines to follow the cursor into a panel it does not own
(`form_owner` set).

**The content owner.** The panel shows exactly one extension's content:
`FunctionPanelState.content_owner`. It starts as the content provider. It
becomes another extension when that extension claims the panel with
`ShowFunctionContent{owner}` or follows the cursor with its own `LoadContent`;
it returns to the provider when that extension's overlay mode pops, or
whenever the function panel is hidden (in any mode). A form's own reads (CmdOp's browser) do not change it —
the form is `form_owner`, drawn through `render_form`. Content search is routed
to the content owner, so what is searched is always what is shown.

`PanelView` is the logical content core lays out: `Document(Vec<Block>)` — a
`Block` is `Field { label, value }`, `Line(spans)` (one row, h-scrolled),
`Para(spans)` (wrapped by core at the panel width), `Heading(spans)`, `Rule`,
or `Image` — or a whole-panel `Image { width, height, rgb }`. A `TextSpan`
carries `SpanStyle::{Default, Role(name), Rgb{fg,bg}}` and an emphasis. The
extension tags spans; it never wraps, scrolls, numbers, aligns, or highlights.

Search matches are **logical**: `(block index, byte start, byte end)` within
the concatenated text of that block's spans (a `Field`'s value). The
extension's matcher runs over its own document and never needs the panel
geometry; core maps each match to the rows its block was laid out into.

| Concern | Home |
|---------|------|
| Panel chrome, the Exec frame | core |
| The `PanelView` layout engine (alignment, wrap / h-scroll, line numbers, theming, search highlight, image → cells) and its layout cache | core |
| `content_owner`, `ext_view`, scroll, the search match list | core (`AppState`) |
| A dynamic extension's replied content | the loader's wrapper for that extension (served as its `view()`) |
| Raw bytes and the decoded representation | extension (interior mutability) |
| Decoding, the search matcher, the cursor-follow and paging policy | extension |
| Keys, conditions, options | extension, declared in Scheme |

Two constraints shape the seam:

1. **`:when` predicates read only `AppState` and the OptionStore.** State that
   gates a key lives in AppState or is an option — hence the search match list
   is core's (`PanelSearch`), while the matcher is the extension's. The extension computes in `handle_intent`; the
   reducer stores via `SetSearchMatches` / `SetScroll`.
2. **The reducer cannot call extensions.** Extension-owned behavior routes
   through the kernel (`handle_intent`), which returns generic intents.

Because the closed `Payload` carries only core types, only raw bytes cross the
worker → main boundary; decoding runs in `accept_content` on the main thread,
bounded by the chunk size.

## Bundled Extensions

| Id | What it owns | Canon |
|----|--------------|-------|
| preview | The reference content provider: MIME dispatch via `wasdf-sdk` (text / hex / markdown / image / directory), syntax highlight (syntect), chunk paging, the less-style search matcher, and its keys — `/` `n` `p` in both the function scope and the file scope, so content can be searched without focusing it | this section |
| archive | Pack (an InputForm for the archive name) and unpack (a resolver plan built from the cursor entry), as resolver entries + bindings gated by native predicates. No mode, no panel | this section |
| pty | Pseudo-terminal sessions in the function panel; implements the command-runner contract (so `cmdop.runner = "pty"` runs builder commands on a PTY) and optionally hosts the editor | [PTY.md](PTY.md) |
| theme | Theme presets and custom themes under `@/settings/Display/Themes`, the cursor-follow role color editor, publishing the active theme into ThemeCell, and a prelude (`define-theme`, `set-theme`) | [THEME.md](THEME.md) |
| lscolors | File-list coloring (`entry_style`, which receives the frame's theme) from `LS_COLORS`, a named palette (browsed under `@/settings/Extensions/lscolors/Palette`), or `Themes` — directories in the active theme's `title-focused` color, bold, the rest from the palette | this section |
| bookmarks | `@/bookmarks`, `B` to add, removal on a bookmark leaf, the `bookmarks` / `add-bookmark` palette commands; the list is its own file, written through `Host.files` | this section |
| repl | `@/repl`: a Scheme REPL on the resident session, as an active function-panel form — an input line (`BeginInput`) over the evaluation history; each submit emits `Eval{owner: "repl", expr}` and the result arrives in `accept_content` as `Content::Value`. Past evaluations are listed as leaves under `@/repl`; activating one opens the REPL with that expression in the input line, ready to edit and re-run. The `repl` palette command opens it | [SCHEME.md](SCHEME.md), The REPL |
| settings | `@/settings`: every declared option as an editable leaf (showing which layer its value comes from, with reset), extension screens with About cards, hosting the contributed `SettingsNode`s (handing each its base path), key rebinding; every edit is a `Configure` intent; the `settings` palette command | this section |

## Registration

At boot every extension — bundled and optional alike — goes through one
registration path: its native `keymaps()` and `scheme_source` groups merge into
the Extension layer (collision detection + user override); its commands,
resolver entries, VFS providers, conditions, and settings nodes are collected;
then it is stored in the ExtensionRegistry. After the user layer is applied and
the resolver chain is finalized, every extension receives `bind(&Host)`.
Extension dispatch routes extension intents to their owner by id.
