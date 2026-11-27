# Theme Extension

`theme` is the bundled extension that owns color themes: the bundled presets,
user-defined themes, the per-role editor, and the active theme. It assumes
[EXTENSION.md](EXTENSION.md) (settings nodes, forms, `Host`) and
[SCHEME.md](SCHEME.md) (the settings store, preludes).

## Model

A `Theme` (core type, `core/theme.rs`) maps every `Role` to a style: a
foreground, a background, and Bold / Italic / Underline. `Color::Reset` is
**nil** — literally no color. Every item that has both a foreground and a
background is **one role carrying both** (`cursor`, `match`, `notice-info`,
`help-key`, …) — there are no separate `-fg` / `-bg` roles. Presets are
`(quote …)` data in `theme/presets.scm`, interpreted by the extension itself.
User themes are presets' copies with per-role overrides.

### One role form everywhere

A role's look is written the same way in `presets.scm`, in `define-theme`, and
(inside the store's map) in `custom.scm` — the role, then its fields:

```scheme
(cursor (fg "#2e3440") (bg "#88c0d0") (bold #t))
```

Every field (`fg`, `bg`, `bold`, `italic`, `underline`) is optional: an omitted
color is nil, an omitted modifier off. A role listed is that role's whole look;
a role not listed keeps the built-in one. In `custom.scm` the same fields sit
under the role's name, in that order:
`(opt-set! 'theme.custom '(("mine" (("cursor" ((fg "#2e3440") (bg "#88c0d0")))))))`.
`file-row-size-graph` is the file-size bar's gradient: `fg` its left end (each
preset uses its highlight color), `bg` its right end (gray).

The extension resolves the active theme and publishes it into ThemeCell
(`Host.theme`) at bind time and after every change; render reads that
`Arc<Theme>` and never touches the theme registry.

### Resolution and the base fallback

`Role::Base` is what the whole frame is painted with before any more specific
role draws; its fg and bg are the base pair every other role falls back onto.
For every other role, `Theme::style` resolves:

- a role's own non-nil fg or bg wins;
- a nil bg falls back to base's bg;
- a nil fg falls back to base's **bg** if the role set its own bg (a
  "knockout": text in base's bg color on the role's background), otherwise to
  base's fg;
- a fallback that is itself nil stays nil — the terminal's color shows through.

`Role::Base` resolves to its own fg / bg with no fallback.

## `@/settings/Display/Themes`

Contributed as a `SettingsNode` anchored to the Display category; the settings
host hands the node its base path, and the editor's cursor-follow gate compares
against that path rather than a hardcoded one:

| Path | Entries |
|------|---------|
| `Themes/` | one directory per theme (the active one badged), `+ New theme…` |
| `Themes/<theme>/` | `Set active` (unless active); one leaf per role showing its current look; for a preset a header (`preset — duplicate to edit`) and `Duplicate…`; for a custom theme also `Rename…` and `Delete` |

New, duplicate, and rename ask for a name through `Mode::InputForm`; delete
confirms through Policy.

## The role editor

The editor is **cursor-follow**: when the file cursor lands on a custom theme's
role leaf, `on_cursor_changed` seeds the editor from that role's resolved style
and hosts the panel with `HostFunctionForm(Some("theme"))` — the passive form
claim that leaves focus on the file panel, so `wasd` keeps browsing roles while
`hjkl` edits whichever role is under the cursor. Leaving a role (a preset's
role, another leaf, another directory) releases the panel the same way.
Activating a role leaf (`edit-role`) runs the same seed-and-host logic.

A role's fg, bg, and modifiers are **one setting**, edited together. The
editor's focusable controls, top to bottom as drawn:

1. FG nil toggle
2. FG R, G, B
3. Bold, Italic, Underline
4. BG nil toggle
5. BG R, G, B

A nil toggle does not discard the sliders beneath it; it decides whether they
apply. The panel draws, in the same order: the FG nil row, an fg R/G/B bar
chart (the focused channel emphasized), the three modifier rows with aligned
`[x]` / `[ ]` boxes, a separator, the BG nil row, a bg bar chart, and a one-line
swatch styled with the role's actual resolved style. Sections that do not fit a
short panel are omitted.

Keys, in the **file** panel scope, gated by `theme-color-active`
(`form_owner == "theme"`) — they override core's `FuncUp/Down/Left/Right` there:

| Key | Action |
|-----|--------|
| j / Down, k / Up | next / previous control |
| l / Right, h / Left | step the focused channel by ±1, or flip the focused toggle |
| L, H | step by ±10, or flip the focused toggle |

There is no confirm or cancel: every keypress applies the role's whole style
by returning a `Configure` intent that updates `theme.custom` in the store's
`custom` layer; the change set republishes ThemeCell, and the Persister writes
`custom.scm` on its own thread — a held key costs one write, not one per
repeat. An edit that lands exactly on the role's built-in
default removes the override instead of storing a redundant one.

## Settings and Persistence

Theme state is two options, held in the settings store like every other
setting ([SCHEME.md](SCHEME.md)):

| Option | Kind | Meaning |
|--------|------|---------|
| `theme.active` | `text` | the active theme's name |
| `theme.custom` | `(map (map (record (fg text) (bg text) (bold bool) (italic bool) (underline bool))))` — theme name → role → style | user themes: per-role overrides of the preset they were copied from |

A color value is a `"#rrggbb"` string or an ANSI name; an omitted field is the
default (no color, modifier off), so only what differs is written.

The theme prelude gives users friendlier procedures for `config.scm` and the
REPL; both write through the store, so they are validated and land in the
caller's layer:

| Procedure | Effect |
|-----------|--------|
| `(set-theme "name")` | `(opt-set! 'theme.active "name")` |
| `(define-theme "name" (role (fg C) (bg C) (bold B) …) …)` | set `name`'s overrides in `theme.custom` — a macro, so the role forms are written unquoted, exactly as in `presets.scm`, e.g. `(define-theme "mine" (entry-dir (fg "#83a598") (bold #t)))` |

So a user can pick a different theme per platform in `config.scm`, while the
editor's edits land in the `custom` layer and persist in `custom.scm`.
