---
name: new-extension
description: Implement a new wasdf feature as an extension (bundled or user). Use when adding any feature to wasdf, creating an extension, binding new keys to new behavior, adding a settings screen, or asking where feature code should live. Covers the bundled-vs-user split, the scm/Rust contract, and the zero-edit rules.
---

SKILL: Implementing a new feature as a wasdf extension
=======================================================

Audience: an LLM (or human) adding a feature to wasdf. In this codebase,
features ARE extensions ("thin core, fat extension" — CLAUDE.md pillar 2).
Before writing code, read doc/EXTENSION.md (the canon this skill condenses),
doc/SCHEME.md (the scm roles), and the Top-Level Invariants in CLAUDE.md.

Prime directive — the Zero-Edit Principle
-----------------------------------------
Adding an extension edits nothing in core/, app/, script/, or exec/.
Acceptance test: grep the tree for your extension's id — outside your own
directory it may appear ONLY in the bundled-extension list
(`extension::bundled()` in crates/wasdf/src/extension.rs). If you find
yourself editing app/kernel.rs, core/intent.rs, or the reducer to make your
feature work, stop: you are on the wrong path. (Precedent: lscolors was once
wired through kernel.rs and was refactored back out — only `theme` earns a
kernel special case, because App itself must hold its registry Arc for the
per-frame `resolve_active()` read.)


1. Two kinds, one trait
-----------------------
| Kind    | Lives at                                        | Linked        |
|---------|--------------------------------------------------|---------------|
| Bundled | crates/wasdf/src/extension/<id>/  (ALL of it)    | statically    |
| User    | ~/.config/wasdf/extensions/<id>/init.scm + lib   | dlopen (C ABI)|

Both are the same concept: an object implementing `extension::Extension`
(crates/wasdf/src/extension.rs), registered once at boot, addressed by id.
A user extension is wrapped in `loader::DynamicExtension`, which implements
that same trait by bridging calls over a C ABI. Decide the kind first:

- Building a feature INTO wasdf (this repo)  -> bundled.
- Building a feature FOR wasdf from outside (separate crate, deployable
  without rebuilding the app) -> user extension against `wasdf-sdk`.


2. What is COMMON to both kinds (design you always follow)
----------------------------------------------------------
These mechanisms are identical regardless of kind; design against them and
the kind becomes almost an implementation detail.

a) The pipeline. One flow, no exceptions:
     key -> Intent -> (kernel dispatch) -> your handler -> follow-up Intents
          -> reducer -> render
   Your extension participates by RETURNING intents, synchronously. It never
   does I/O inline (I/O = kernel async plans), never touches the event loop,
   never adds a variant to the core `Intent` enum.

b) Extension intents are data. `(ext "<id>" "<intent>" [data])` in scm
   becomes `Intent::Extension(ExtensionIntent { extension, intent, data })`
   in Rust. `data` is an `ExtensionValue` (Nil/Bool/Int/String/Path/List/
   Map) consumed structurally — no downcasting. Two reserved data keys:
     "item"     — filled by Mode::InputForm's confirm with the typed text
     "resolver" — marks the intent as a plan; the kernel executes it through
                  the resolver chain (with Policy confirmation when
                  destructive); it is NOT dispatched to you.

c) Declarations, not registrations. You declare commands, keymaps (Extension
   layer), resolver entries, and options; the kernel collects them once at
   boot. Options use the shared form
     (option "<id>.<key>" "<Category>" <kind> "<default>" "<label>")
   with kind ∈ bool | text | list | (int MIN MAX) | (choice "a" "b" …), and
   automatically appear editable + persisted under
   @/settings/Extensions/<id>, next to an auto-generated About card
   (`Extension::about()`) and a Priority leaf.

d) UI surface. The function panel is the ONE place an extension renders, and
   it renders a LOGICAL `PanelView` (document of blocks: field / line /
   heading / rule, spans styled by theme role or literal rgb). Core owns all
   layout: alignment, wrap, scroll, line numbers, theming, search highlight.
   You describe WHAT, never assemble styled lines. For interaction, prefer
   the existing modes: `Select` (picker) or `Mode::InputForm` (one-line
   prompt / typed widget) before inventing a custom mode; overlay extension
   modes exist for cursor-following panels (keys fall through beneath).

e) Failure policy, everywhere: disable, notify, continue. A failed `:when`
   is false. A faulting extension is disabled, not fatal. Never add retry UI.

f) VFS. An extension may contribute a mount, conventionally `@/<id>`,
   serving `Entry` lists whose leaves carry `(ext …)` activations — an
   ordinary source of intents, not a separate mechanism.


3. What DIFFERS between bundled and user extensions
---------------------------------------------------
| Aspect            | Bundled                              | User (optional)                         |
|-------------------|--------------------------------------|-----------------------------------------|
| Location          | crates/wasdf/src/extension/<id>/     | extensions dir: <id>/init.scm + lib     |
| Trait impl        | direct Rust `impl Extension`         | C ABI fns bridged by DynamicExtension   |
| Glue delivery     | `include_str!` .scm, returned from   | init.scm manifest, discovered & parsed  |
|                   | scheme_source()/option_scheme()      | as data by loader::parse_glue           |
| Panel content     | PULL: `view(ctx) -> PanelView`, plus | PUSH: reply `(show-function-content     |
|                   | `accept_content` for kernel-read     | "<id>" (document …))` from a handler;   |
|                   | bytes (LoadContent -> Plan::Read)    | no accept_content (deferred)            |
| Images            | yes (PanelView::Image, raw RGB)      | no (pixels don't cross the ABI)         |
| Interactive form  | `render_form(&mut Frame, …)` +       | not available (a Frame can't cross      |
|   in the panel    | SetFunctionForm(Some(id)) (CmdOp,    | the ABI)                                |
|                   | theme's color editor)                |                                          |
| `:when` predicates| may register NATIVE predicates via   | limited to core predicates (cannot      |
|                   | register_conditions (read &AppState) | register natives across the ABI)        |
| Options readback  | shared `Arc<OptionStore>` stashed in | `wasdf_sdk::Settings::from_scheme_alist`|
|                   | bind_store (OnceLock)                | snapshot handed as the intent payload   |
| Settings subtree  | may graft a SettingsNode (rich       | not available                           |
|                   | screens under @/settings/…, e.g.     |                                          |
|                   | lscolors' Themes)                    |                                          |
| Registration      | one line in `extension::bundled()`   | zero edits: drop files in the dir       |
| Versioning        | compiled together                    | `wasdf_abi_version()` must equal        |
|                   |                                      | wasdf_sdk::API_VERSION exactly, else    |
|                   |                                      | skipped with a warning                  |
| Faults            | ordinary Rust                        | catch_unwind around every ABI call; a   |
|                   |                                      | panic disables the call, not the app    |
| Id collision      | you control the list                 | later manifest is disabled with warning |

Everything in section 2 (intents-as-data, options form, keymap shape,
resolver entries, failure policy, VFS, the PanelView vocabulary) is shared
verbatim — a user extension's manifest sections are parsed by the SAME
script::config parsers the bundled .scm sources go through.


4. The scm / Rust split (applies to both kinds)
-----------------------------------------------
Scheme is the WHAT and WHEN. Rust is the HOW. Concretely:

  scm declares (data, evaluated/parsed once at boot, never on the input or
  render path):
    - keymaps: which key, in which mode/panel scope, fires which intent,
      gated by which `:when` condition
    - palette commands, resolver entries, options (kind/default/label)
    - for data-heavy extensions: presets as pure `(quote (…))` literals
      (see theme/presets.scm, lscolors/presets.scm)

  Rust implements (runtime behavior):
    - `handle_intent`: the match over your intent names — the OTHER END of
      every `(ext "<id>" "<name>")` your scm declares. The intent-name
      string is the contract between the two files; keep them in sync.
    - state (interior mutability: RefCell/OnceLock — `handle_intent` takes
      &self), decode, matching, view construction, persistence
    - native `:when` predicates (bundled only), VfsProvider impls,
      SettingsNode impls, parsing of your own preset scm via
      script::config (add a parser there if you introduce a new form —
      script/ is generic infrastructure, not core)

  The runtime relationship:
    - Bundled: kernel evaluates your `scheme_source()` in the resident
      steel session at registration; the resulting datum is decoded by
      script::codec/config into NATIVE tables (bindings, commands,
      OptionDefs). After boot, scm is out of the loop — a keypress hits
      compiled tables and dispatches straight to your Rust.
    - User: init.scm is never evaluated at all — it is READ AS DATA by the
      s-expression reader. The link from a declared `(ext id intent)` to
      your Rust happens at dispatch time over the C ABI, by id. Replies
      cross back as Scheme datums (strings) and are decoded into intents
      by the same codec.
    - Never parse or format intents/keys/modes yourself; script::codec is
      the single codec (CLAUDE.md invariant).

  Rule of thumb: if you're writing a table (keys, commands, colors,
  defaults) — it's scm. If it computes, reads the filesystem, or holds
  state — it's Rust. Glue is data and cannot compute (this is exactly why
  exampleext needs a cdylib and can't be scm-only).


5. Recipe: bundled extension (all inside crates/wasdf/src/extension/)
---------------------------------------------------------------------
1. Layout (no mod.rs — `<dirname>.rs` facade, per CLAUDE.md):
     crates/wasdf/src/extension/<id>.rs        # facade: mod decls + pub use
     crates/wasdf/src/extension/<id>/extension.rs
     crates/wasdf/src/extension/<id>/keymap.scm    # if you bind keys
     crates/wasdf/src/extension/<id>/options.scm   # if you have options
     …plus whatever else the feature needs (parse.rs, registry.rs, writer.rs)
2. In extension.rs: `struct <Id>Extension { … }` with interior-mutable
   state; `impl Extension` overriding only what you provide. Take
   `config_dir: Option<PathBuf>` in `new()` if you persist anything
   (convention: bookmarks, lscolors) — the file name inside it is yours.
3. Embed scm with `include_str!("<id>/keymap.scm")` etc.; return from
   `scheme_source()` / `option_scheme()`.
4. Register: add ONE line to `extension::bundled()` in
   crates/wasdf/src/extension.rs. This is the only edit outside your dir.
   Do NOT touch app/kernel.rs unless App itself must own a shared handle
   (the theme registry is the only current legitimate case).
5. Persist user edits (if any) into the managed block of config.scm using
   your own writer (copy the theme/writer.rs pattern: BLOCK_BEGIN/END,
   atomic tmp+rename); reload them in `new()` by scanning the same block.
6. Rich settings screen (if flat option leaves aren't enough): implement
   `SettingsNode` and return it from `settings_nodes()`; anchor to a core
   category `("Display", "Name")` or to your own screen `("<id>", "Name")`
   (-> @/settings/Extensions/<id>/Name). The kernel collects nodes from
   all bundled extensions generically.
7. Tests: unit tests inline in each module (see lscolors/registry.rs,
   script/config.rs test style). `cargo test -p wasdf` must stay green.
8. Verify live: run the TUI and drive your feature (keys, settings screen).

6. Recipe: user extension (outside this repo, against wasdf-sdk)
----------------------------------------------------------------
1. Write init.scm — the manifest, pure data:
     (extension "<id>"
       (lib "<crate_lib_name>")                         ; omit if scm-only
       (keymaps ((file file (("o" (ext "<id>" "<intent>"))))))
       (commands (…)) (resolvers (…))
       (options ((option "<id>.k" "<id>" text "" "k"))))
   A manifest with no (lib …) is purely declarative: it can only wire
   EXISTING core intents (still useful for custom keymaps/commands).
2. If behavior is needed, write a `cdylib` crate depending on `wasdf-sdk`
   (model: crates/wasdf-example-ext). Export:
     wasdf_abi_version() -> u32                        // = API_VERSION, required
     wasdf_handle_intent(intent, data) -> *c_char      // optional
     wasdf_on_cursor_changed(path) -> *c_char          // optional
     wasdf_vfs_list(path) -> *c_char                   // optional, mounts @/<id>
   All replies are Scheme-datum strings: a list of follow-up intents, e.g.
     ((notify "hi") (push-overlay "<id>" "info")
      (show-function-content "<id>" (document (field "Name" ((span "x"))))))
   Keep the reply CString alive until the next call (thread_local slot).
3. Read your options from the `data` payload:
   `wasdf_sdk::Settings::from_scheme_alist(data)` — the kernel passes your
   live option snapshot whenever the intent form carried no data of its own.
4. Deploy: `~/.config/wasdf/extensions/<id>/init.scm` + `lib<id>.{so,dylib}`
   (the loader adds the platform affixes to the bare (lib …) name).
   Your options + About appear under @/settings/Extensions/<id> like a
   bundled extension's.

7. Pitfalls (each has bitten before)
------------------------------------
- The reducer cannot call extensions. Anything the reducer must apply goes
  through generic setter intents you return (SetSearchMatches, SetScroll,
  SetFunctionForm, Refresh, NavigateTo, …).
- `:when` predicates read ONLY &AppState and must be cheap. State that
  gates a key must therefore live in AppState, even when the algorithm that
  computes it is yours (preview's search matches are the precedent).
- Don't grow the core Intent enum, don't branch per-intent in the event
  loop, don't handle completions per-intent — (purpose, payload) only.
- Prefer removing a branch over adding an abstraction; MVP short and
  correct beats extensible (CLAUDE.md).
- presets/config scm files are `(quote (…))` data literals — parse them via
  script::sexpr + a script::config parser; never hand-roll a parser and
  never build scm source at compile time in Rust.
- If your feature is "a picker" or "one line of input", you do not need a
  mode: Select / Mode::InputForm already exist and round-trip through the
  reserved "item" key.
