# Scheme Configuration

wasdf uses Scheme in three roles: the **format** of all declarative data, the
**settings store and the language that changes it** (the user's `config.scm`,
the persisted `custom.scm`, and the REPL), and the **layouter**. Data is read
by the s-expression reader. The store lives in the resident steel session
(`steel-core`): a dedicated OS thread owns the `Engine` (which is
`!Send`/`!Sync`) and the kernel talks to it over an mpsc channel. Scheme is
never consulted on the input or render path: the kernel keeps native
**projections** of the store, and those are what input and rendering read.

## Files and Their Readers

| File | Kind | Reader | Writer |
|------|------|--------|--------|
| Embedded defaults (`app/keymap.scm`, `command.scm`, `resolver.scm`, `options.scm`, `layout.scm`) | data: one `(quote (...))` literal | s-expression reader → the store's `default` layer | — (compiled in) |
| A bundled extension's data sources (keymap, options, presets) | data | s-expression reader | — (compiled in) |
| An extension's **prelude** (optional) | program | the session, at registration | — (shipped with the extension) |
| An optional extension's manifest (`<id>/init.scm`) | data | s-expression reader | — |
| `config.scm` | program | the session → the `config` layer | the user (wasdf only creates it, as a template, when absent) |
| `custom.scm` | program restricted to literal store calls | the session → the `custom` layer (and the reader, early at boot and as the fallback) | the Persister, generated from the store |

The config directory is `$WASDF_CONFIG_DIR`, else `$XDG_CONFIG_HOME/wasdf`, else
`~/.config/wasdf`.

All string → `Datum` reading goes through `script::sexpr`, and all encoding and
decoding of intents, keys, and modes goes through `script::codec`. No other
module parses or formats these strings. There are no parallel Rust tables: the
embedded sources are the only definition of the defaults.

## The Settings Store

The store is the single source of truth for everything configurable. It holds
Steel values in five sections:

| Section | Entries |
|---------|---------|
| options | every declared option, typed (`bool`, `text`, `int`, `choice`, `list`, and the structured kinds `record` and `map`) — core options and every extension's options alike |
| bindings | (mode, panel, key) → (intent, when, label) |
| layouts | layout id → layout tree (Role 3) |
| commands | palette command name → (description, intent) |
| orders | named display orders: `extensions` (extension priority — [UI.md](UI.md), Extension priority), `help` (help-row label order) |

### Layers

Every entry holds one value per layer; the **effective value** is the one in the
highest non-empty layer:

| Layer | Written by | Persisted |
|-------|------------|-----------|
| `default` | embedded defaults and extension declarations | — |
| `config` | `config.scm` | the user's file |
| `custom` | the settings UI, extension editors (via `Configure`), and `customize!` at the REPL | `custom.scm` |
| `session` | plain writes at the REPL | never |

Options mirror all four layers. For bindings, commands, and orders the
`default` layer stays native — the keymap's Core and Extension layers, the
built-in command list, registration order — and the store holds only the
user's layers above it.

`reset!` clears an entry's `custom` value, so whatever `config.scm` computed
(or the default) shows through again. An `unbind!` in a layer is a tombstone:
it hides the binding from the layers below.

### Procedures

The store's procedures are the one configuration vocabulary — `config.scm`,
`custom.scm`, extension preludes, and the REPL all use them:

| Procedure | Effect |
|-----------|--------|
| `(opt 'key)` | the effective value |
| `(opt-set! 'key value)` | set an option in the current layer |
| `(bind! 'mode 'panel-or-#f "key" 'intent ['when] ["label"])` | add a binding in the current layer; after the intent, a string is the help label and anything else is the `:when` |
| `(unbind! 'mode 'panel-or-#f "key")` | tombstone a binding in the current layer |
| `(layout! 'id 'tree)` | set a layout |
| `(command! 'name "description" 'intent)` | set a palette command |
| `(order! 'name '(item …))` | set a display order |
| `(customize! expr)` | evaluate `expr` with `custom` as the current layer |
| `(reset! 'key)` | clear the `custom` value of an option (bindings: `(reset! 'mode 'panel "key")`) |

The **current layer** is set by what is being evaluated: `config` while
`config.scm` runs, `custom` while `custom.scm` loads, `session` at the REPL.
Every write is validated against the entry's declared kind when it is made; an
invalid write is an error at that call (at the REPL, immediately). The session
also provides environment procedures, evaluated against the real environment:

| Procedure | Returns |
|-----------|---------|
| `(os)` | `"macos"`, `"linux"`, or `"windows"` |
| `(wsl?)` | whether running under WSL (Linux with a Microsoft kernel) |
| `(hostname)` | the host name |
| `(getenv "NAME")` | the variable's value, or `#f` |
| `(executable? "cmd")` | whether `cmd` resolves on `PATH` |

### Projections

Each evaluation in the session returns its value (or error) and the **change
set** — the entries whose effective value it changed. The kernel applies the
change set to its native projections: the OptionStore (typed effective values,
with a generation bumped on every change), the keymap's User layer (rebuilt,
with collision detection), the CommandRegistry, the layouts, and the display
orders. Extensions observe their options through the OptionStore's generation.
Input, `:when`, and rendering read only projections.

Every write from the UI goes through the store too: the core intent
`Configure` carries a structured write (set / reset / bind / unbind / layout /
order, in the `custom` layer), which the codec renders as a store call and the
session evaluates. The store is therefore never bypassed, and the projections
are never written directly.

## Role 1: The Configuration Program — `config.scm`

`config.scm` is the user's program, in the spirit of an Emacs init file. It is
evaluated once at boot, writing into the `config` layer, and it may compute:
choose options, bindings, layouts, and orders by platform, host, terminal, or
installed tools.

```scheme
(when (equal? (os) "macos")
  (opt-set! 'border-style "rounded"))
(when (wsl?)
  (bind! 'file 'file "o" '(navigate-to "/mnt/c/Users")))
(unless (executable? "bat")
  (opt-set! 'preview.tab-width 2))
(set-theme "gruvbox")                    ; a procedure from the theme prelude
```

It is evaluated **one top-level form at a time**, in order. A form that fails —
an error, an unbound procedure, an invalid value — is reported to `@/messages`
with its position and skipped; the forms before and after it still apply.
Definitions made by earlier forms are visible to later ones and at the REPL.
Because each form is compiled on its own, a definition may not refer to a
procedure defined by a *later* form (steel rejects the forward reference):
define helpers first. The same holds for preludes.

When `config.scm` is absent, it is written as a template: every option and
every default binding as commented-out store calls, plus commented examples of
environment-conditional configuration. wasdf never writes `config.scm`
otherwise.

Boot waits for the session to be ready, bounded by the option
`scheme.boot-timeout-ms`. Its value is known before the wait because
`custom.scm` is read first by the reader (below); `config.scm` cannot set it.
If the session is not ready in time, `config.scm` contributes nothing, the
failure is reported to `@/messages`, and boot continues on the fallback.

## Role 1': Persisted Edits — `custom.scm`

`custom.scm` is the `custom` layer, written out. It is generated from the
store — never edited in place — as a flat sequence of store calls with
**literal arguments only** (`opt-set!`, `bind!`, `unbind!`, `layout!`,
`command!`, `order!`), one per customized entry. After a change to the `custom`
layer, the Persister asks the store for the dump — the store itself renders
the `custom` layer as literal store calls, sorted (`store-dump-custom`) — and
writes the file on its own thread — coalesced, so a burst of edits (a held key in the theme editor)
costs one write — atomically (temp file + rename).

Because it is literal-only, `custom.scm` is also valid data: the reader
parses it early at boot (the extension enable toggles and the session boot
timeout are needed before anything else) and whenever the session is
unavailable.

## Data Forms

The embedded sources and extension data use these shapes:

| Form | Declares | Elements |
|------|----------|----------|
| keymap group | key bindings | `(mode panel-or-#f (binding…))`; a binding is `("key" intent [when] ["label"])` |
| command | a palette command | `(name "description" intent)` |
| resolver | a resolver entry | `(key confirm ((kind argv-el…) …) [((token label) …)])`; `confirm` is `never`, `always`, or `overwrite`; argv elements are strings or the placeholder symbols src, dst, path, paths, opts ([ARCHITECTURE.md](ARCHITECTURE.md)) |
| option | a configurable option | `(option "key" "Category" kind default "label")`; kind ∈ `bool`, `text`, `list`, `(int MIN MAX)`, `(choice "a" "b" …)`, `(record (field kind) …)`, `(map kind)`; the default is a literal of that kind. A record holds any subset of its fields (an omitted one is its reader's default), kept in declaration order under symbol names; a map's keys are strings |
| layout | a layout | `(id tree)` — see Role 3 |

Key strings name a character, a special key (Enter, Esc, Tab, `Shift-Tab` /
`BackTab` — two spellings of one key — Space, the arrows, Home/End,
PageUp/PageDown, Insert, Delete, Backspace, F1–F35), or a modifier chain
(`Ctrl-`, `Alt-`, `Shift-`, `Super-`, `Hyper-`, `Meta-`). Mode scopes name File,
Policy, Select as a whole, a single Select instance, InputForm, or an extension
mode; they match by longest id prefix.

## Role 2: Extension Glue

Each extension's data sources declare its bindings into core modes, its
commands, its options, and its presets; they are read into the store's
`default` layer at registration. An extension's settings are ordinary options
(structured kinds where needed — theme's user themes are a `map` of `record`s),
so there is no per-extension configuration form.

### Preludes

An extension may ship a **prelude**: Scheme code evaluated in the session at
registration, after the store holds its declarations and before `config.scm`
runs. A prelude defines convenience procedures on top of the store procedures —
theme's `(define-theme "name" …)` and `(set-theme "name")` are prelude
procedures that write `theme.custom` and `theme.active`. The rules:

- A prelude only defines procedures; it writes nothing at load time.
- Its procedures reach the store only through the store procedures, so every
  write is validated and lands in the caller's current layer.
- A name that another prelude (or the core vocabulary) already defines is
  reported, and the later definition is skipped.
- A prelude that fails to load is reported to `@/messages`; the extension stays
  registered without it.

Bundled extensions supply it through `Extension::prelude()`; an optional
extension's manifest names it with `(prelude "file.scm")`.

### `:when` conditions

A condition is a closed grammar: a registered predicate name, or
`(option "key")`, combined with `and`, `or`, `not`. It is parsed once into an
AST and evaluated natively per keypress. Predicates read `AppState` and the
OptionStore projection — nothing else — and must be cheap. `(option "key")` is
the core predicate for a bool option being on (e.g. pty's `e` binding:
`(option "pty.edit-host")`). A condition that names an unknown predicate, or an
undeclared option, is false. Conditions do not probe the environment;
environment-dependent configuration belongs in `config.scm`.

## Role 3: Layouter

Layouts are store entries, laid out natively; rendering makes no Scheme round
trip. The two core layouts, `file` and `select`, are defined in the embedded
`app/layout.scm`; `(layout! 'id 'tree)` in `config.scm`, at the REPL, or in
`custom.scm` replaces one, so a layout can differ per environment.

The default `file` layout, as `app/layout.scm` declares it (`select` is the
same with the main row holding a column of `select-input` over
`select-candidates` beside `function`):

```scheme
(layout! 'file
  '(column
     (row (size 7 5)                          ; rows: Large class, Compact class
       (panel nav     (weight (option "top.nav-weight"))     (min 22))
       (panel prop    (weight (option "top.prop-weight"))    (min 18) (when (option "top.prop-visible")))
       (panel monitor (weight (option "top.monitor-weight")) (min 16) (when (option "top.monitor-visible"))))
     (row
       (panel file     (ratio left))
       (panel function (ratio right) (when function-visible)))
     (panel help (size 1))))
```

| Node / attribute | Meaning |
|------------------|---------|
| `(column NODE…)` / `(row NODE…)` | stack children vertically / side by side |
| `(panel ID)` | a leaf: one panel from the catalog — `nav`, `prop`, `monitor`, `file`, `function`, `help`, `select-input`, `select-candidates` ([UI.md](UI.md)) |
| `(size N [M])` | fixed rows (in a column) or columns (in a row); `M` applies in the Compact size class (fewer than 25 rows) |
| `(weight W)` | share of the space left after fixed sizes |
| `(ratio left\|right)` | the file : function split from `FunctionPanelState.ratio` |
| `(min N)` | the least space the node needs; when the minimums of a row's (or column's) visible children do not fit, children are hidden from the end until they do — the first is never hidden |
| `(when COND)` | visible only while the `:when` condition holds; a hidden node gives its space to its siblings. The select layout uses the core predicates `select-has-input` and `select-has-function` |

A numeric attribute may be a literal or `(option "key")`. Space is given out
in order: each flexible child takes its weighted share of what is left after
the fixed children that follow it, bounded below by its `min` and above by the
minimums of the flexible children after it; the last child takes the rest.
Adjacent boxed panels share their border line (the connected frame,
[UI.md](UI.md)); `help` is unboxed and shares none. Overlays (Policy,
InputForm) are not part of a layout.

`layout!` checks the tree natively (the kernel registers the check in the
session) and rejects, at the write, a tree that is not one — an unknown node,
attribute, or panel, or a core layout missing a panel it requires (`file`
needs `file`; `select` needs `select-input` and `select-candidates`) — and the
previous layout stays.

## The REPL — `@/repl`

After boot the session serves the REPL, provided by the bundled `repl`
extension at `@/repl` ([EXTENSION.md](EXTENSION.md)): the user types an
expression, it is evaluated (`Eval` → `Plan::EvalScheme`, off the main thread)
in the environment `config.scm` and the preludes built, and the printed value
or error is appended to the history.

Store writes at the REPL land in the `session` layer: they take effect at once
(through the change set) and last for this session only. `(customize! expr)`
writes to the `custom` layer instead, which is persisted to `custom.scm`; so
`(customize! (opt-set! 'border-style "double"))` is exactly what the settings
UI does.

## Session Lifetime

The engine thread runs for the life of the process: it loads the preludes,
`config.scm`, and `custom.scm` at boot, then serves the REPL and `Configure`
writes. Evaluations are serialized; a long-running one delays the next but
never blocks input or rendering.

**Fallback.** If the session is not ready at boot, the kernel builds its
projections from the embedded defaults and the literal store calls of
`custom.scm`, which the reader parses as data; `config.scm` and the preludes
contribute nothing. If the session is unavailable after boot (not ready, or its
thread exited), the projections keep their last values, every store write —
a settings edit, an editor, the REPL — is refused with a notice, and nothing is
persisted. There is no respawn.

## Error Handling

| Error | Behavior |
|-------|----------|
| Embedded / extension data fails to parse | reported to `@/messages`; that source contributes nothing (a build-time bug, caught by tests) |
| A prelude fails to load, or redefines a taken name | reported to `@/messages`; the prelude (or that definition) is skipped |
| A top-level form of `config.scm` fails | reported to `@/messages` with its position; that form is skipped, the rest apply |
| A store write is invalid | an error at that call — reported to `@/messages`, or shown at the REPL |
| `custom.scm` sets an option no one declares this session (its extension is disabled or gone) | not an error: the entry is kept as written — unvalidated, without effect — and written back on the next dump, so it applies again when its extension returns. Only while `custom.scm` loads; an undeclared key in `config.scm`, at the REPL, or in `customize!` is an invalid write |
| `custom.scm` fails to parse or load | reported to `@/messages`; the file is left untouched (not regenerated) until the user fixes it, and edits of this session stay in memory |
| The session is not ready in time, or dies | reported to `@/messages`; the fallback above |
