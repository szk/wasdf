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
3. **The resident scm (stak) REPL** is used as ① the configuration parser,
   ② the glue for extensions, and ③ the layouter (doc/SCHEME.md).

## Documentation Index

| Document | Contents |
|----------|----------|
| [doc/ARCHITECTURE.md](doc/ARCHITECTURE.md) | Engine canon: component model (the two layers — kernel services + Extension), the intent pipeline, the intent catalog, the **async round-trip contract** (the closed Plan set ReadDir / Search / Read / ResolveAndRun / Execute / Suspend / EvalScheme, the shared AsyncResult schema, the fixed purpose namespace, stale / cancel, Read→accept_content), the **resolver contract** (target counts, placeholder expansion, Policy confirmation for destructive operations), state ownership, module layout (**no mod.rs — `<dirname>.rs` facade**), startup order |
| [doc/UI.md](doc/UI.md) | UI / mode / keymap canon: the core built-in layouts (file / select, plus the Policy overlay), screen chrome (connected frame, scrollbars), the panel catalog, function-panel visibility and sublayout (Preview / Exec), vertical responsiveness; the mode stack (File / Select / Policy / Extension), FunctionPanelState, SelectSpec and its two phases, the **Confirm contract (Single / Many / InputOnly)**, selection and cursor rules; the **canonical key assignments**, the meaning of Enter (per focused panel), the three layers and collision detection |
| [doc/EXTENSION.md](doc/EXTENSION.md) | Extension canon: the thin-core / fat-extension principle, the Extension trait (provides_function_content / accept_content / render_function / scheme_source), the optional C ABI and loading rules (exact api_version match, disable on fault), ExtensionValue and the reserved keys (item / resolver), Select reuse, the zero-edit principle, the function-panel content model (PanelContent / render hook / the `:when` gating constraint), bundled examples |
| [doc/SCHEME.md](doc/SCHEME.md) | The resident REPL, the configuration forms, the three `:when` classes (default false), respawn rules, error handling |

## Top-Level Invariants

- Single pipeline: key → Intent → plan → kernel async → AsyncResult → reducer →
  render. Never write intent-specific branches in the event loop.
- The reducer is pure and synchronous. All I/O lives in kernel async tasks;
  extensions only return plans, synchronously. Completions are handled by
  (purpose, payload) alone, never branching per intent.
- Modal UI is unified under Select. External commands go through the resolver.
  Raw Execute is reachable only from user configuration and the palette.
- The default behavior on failure is the same everywhere: **disable, notify,
  continue** (no retry UI; a failed `:when` is false; an extension fault is
  disabled).
- Prefer "the MVP closing short and correct" over "room to extend." Choose the
  change that removes a branch over the change that adds an abstraction.
