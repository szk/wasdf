# wasdf

A TUI file manager built around WASD-key navigation (Rust / ratatui / crossterm).

The authoritative specification is the English documentation under `doc/`. This
file holds only the index and the top-level invariants. Specification detail is
always written and read on the `doc/` side.

## The Three Design Pillars

1. **Screen layout and key assignments inherit the current spec** (canonical:
   doc/UI.md).
2. **Features are implemented as extensions** (bundled = statically linked /
   optional = dynamically loaded). Yet with zero extensions it still boots as a
   minimal, usable file manager (doc/EXTENSION.md).
3. **Scheme** is ① the format of all declarative data, and — in the resident
   steel-core session — the **settings store** (the single source of truth for
   everything configurable) and the language that changes it: the user's
   `config.scm` (environment-aware, like an Emacs init file), `custom.scm`,
   and the REPL; ② the glue for extensions (data + optional preludes); and ③
   the layouter (doc/SCHEME.md).

## Documentation Index

| Document | Contents |
|----------|----------|
| [doc/ARCHITECTURE.md](doc/ARCHITECTURE.md) | Engine canon: the two component kinds and the kernel services, the intent pipeline (key routing incl. input capture, dispatch stages, the cursor-changed broadcast, tick), the intent catalog, the **purity contract** (no fs / env / process in the reducer), the **render contract** (no I/O, draw-when-dirty, generation caches, viewport-bounded function-panel layout), the **async contract** (the closed Plan set, ReadDir / Search / Read semantics, the AsyncResult schema, one live task per purpose+owner, staleness), the **resolver contract** (expansion, option extraction memo, confirmation), state ownership (ModeStack), module layout (**no mod.rs — `<dirname>.rs` facade**), startup order |
| [doc/UI.md](doc/UI.md) | UI / mode / keymap canon: the layouts, screen chrome, the panel catalog, the function panel (visibility, sublayouts, renderers, FunctionPanelState), vertical responsiveness, UiManager caches; the mode stack, Select (spec, phases, deep search vs. shallow filter, filter stack), the Confirm contract, Policy, InputForm; selection rules; the **canonical key assignments**, Enter semantics, line editing, layers and collisions, the key model |
| [doc/EXTENSION.md](doc/EXTENSION.md) | Extension canon: thin core / fat extension, the Zero-Edit Principle and **coupling between extensions (option keys and intent names only)**, the optional manifest + C ABI (null reply = no-op, logged), the Extension trait and hook rules, `Host`, input capture, ExtensionValue and the reserved keys, the function-panel content model, the bundled-extension table, registration |
| [doc/SCHEME.md](doc/SCHEME.md) | Scheme's three roles: data format (reader); the **settings store** in the steel session (options, bindings, layouts, commands, display orders; layers default / config / custom / session; store procedures; projections via change sets) with `config.scm` (the user's program, environment-aware for macOS / Linux / WSL), `custom.scm` (the persisted custom layer), extension preludes, and the REPL at `@/repl`; and the **layouter (`layout!`)**; `:when` grammar (AppState + options), fallback, error handling |
| [doc/HELP.md](doc/HELP.md) | The help row, derived from the labelled bindings in force (`Binding.help`, `resolve_binding` / `help_hints`, merging and order, keyless input hints, the captured-keyboard row) |
| [doc/CMDOP.md](doc/CMDOP.md) | The **built-in** CmdOp command builder (copy / move / rename / create; part of the kernel): its seams, keys, layout, flows, the runner contract (`cmdop.runner`), and two-stage destination search |
| [doc/THEME.md](doc/THEME.md) | The bundled `theme` extension: the theme model and base fallback chain, `@/settings/Display/Themes`, the cursor-follow role editor, persistence forms |
| [doc/PTY.md](doc/PTY.md) | The bundled `pty` extension: PTY sessions in PANEL_FUNCTION, the core facilities it relies on, the session model, input and key encoding, rendering / resize, `@/pty`, options, the command-runner contract / Edit integration, failure policy |
| [doc/SKILL.md](doc/SKILL.md) | The `new-extension` skill (symlinked from `.claude/skills/`): how to implement a feature as a bundled or user extension |

## Top-Level Invariants

- Single pipeline: key → Intent → plan → kernel async → AsyncResult → reducer →
  render. Never write intent-specific branches in the event loop.
- The reducer is pure and synchronous. All I/O lives in kernel async tasks;
  extensions only return plans, synchronously. Completions are handled by
  (purpose, payload) alone, never branching per intent.
- Render does no I/O and never blocks: it draws from AppState, render caches,
  and shared snapshots, and only when something changed. (The pty extension's
  spawn / resize / write are the only sanctioned I/O outside kernel tasks.)
- Core never names an extension. Extensions reach each other only through
  option keys and intent names, and degrade when the other is absent.
- Modal UI is unified under Select (pickers) and InputForm (one value).
  External commands go through the resolver. Raw Execute is reachable only
  from user configuration (a `bind!` or user palette `command!` in config.scm)
  and from a command line the user edited in an extension's builder.
- The default behavior on failure is the same everywhere: **disable, notify,
  continue** (no retry UI; a failed `:when` is false; a failing `config.scm`
  contributes nothing; a dynamic extension's null reply is a logged no-op).
- Prefer "the MVP closing short and correct" over "room to extend." Choose the
  change that removes a branch over the change that adds an abstraction.
