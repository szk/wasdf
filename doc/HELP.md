# The Help Row

The bottom row of the screen is a single unboxed line of key hints for the
current mode and focus ([UI.md](UI.md)'s `help` panel). It is **derived from
the keymap actually in force**: every chip is a key that would really fire,
found by the same resolution `handle_key` uses. There is no hand-maintained
list of keys anywhere.

## The label lives on the binding

```rust
pub struct Binding {
    pub mode: String,
    pub panel: Option<String>,
    pub key: Key,
    pub intent: Intent,
    pub when: Cond,
    pub layer: Layer,
    /// The help-row label, e.g. "copy". `None` keeps the binding off the row.
    pub help: Option<String>,
}
```

The key is never written twice, so it cannot go stale. The label is the only
authored part, and authoring it is how a binding opts into the row: an
extension labels its headline keys and leaves the rest bare, which keeps the row
short by construction.

In Scheme a label is a **string** slot after the intent; `:when` is a symbol or
list, so the two are told apart by type, not position:

```scheme
("c" (ext "cmdop" "start-copy") "copy")
("f" file-search (and (not filter-maxed) (not cwd-virtual)) "find")
("n" function-search-next function-has-matches "match")
```

The same rule applies to the store procedure `(bind! …)` in `config.scm`, at
the REPL, and in `custom.scm`, so a user can relabel any binding; the
generated template carries the labels.

## Enumeration

```rust
impl KeymapRegistry {
    /// The binding that would fire for `key`, or None. `resolve` wraps it.
    fn resolve_binding(&self, conds: &Conditions, state: &AppState, panel: &str, key: Key)
        -> Option<&Binding>;

    /// Every labelled binding in force, as (keys, label), merged and ordered.
    pub fn help_hints(&self, conds: &Conditions, state: &AppState, panel: &str)
        -> Vec<(String, String)>;
}
```

`help_hints` walks the distinct keys of the bindings whose mode scope is on the
stack, calls `resolve_binding` for each, and keeps the winners that carry a
label. A key bound twice is advertised once, under whichever binding wins, and
a binding whose `:when` is false does not appear. The walk goes down the mode
stack and stops at the first **modal** mode: Select, Policy, InputForm, and an
extension mode pushed as a regular mode. An **overlay** extension mode is not
modal — unbound keys fall through it to the mode beneath — so the walk passes
through it and the row shows both the overlay's keys and those beneath.

## Merging and order

- **Merge** bindings that share a label. When every merged key is a single
  character the chip concatenates them (`wasd`, `jk`, `hl`, `np`, `gG`);
  otherwise they join with `/` (`Enter/Esc`).
- Because merging is by label, a label must name one meaning: two unrelated
  actions never share a word (the file cursor is `cursor`, the operation is
  `move`; core's suspending editor is `Edit full`, the PTY-hosted one `edit`).
- **Order** is the store's `help` display order when it lists the label;
  labels it does not list follow **alphabetically** (case-insensitive). With
  no `help` order set, the lead is `cursor`, `select`, `edit`, `find`, `move`,
  `delete`, `shell`, `quit`, `help` — so the File row reads
  `wasd cursor  Space select  edit  find  move  Delete  x shell  quit  ? help …`.
  A user `(order! 'help …)` replaces that lead. Bindings are written in the
  order they should read (`w a s d`).
- **Chips.** When the key spells the start of its label (case-insensitively)
  the chip *is* that part of the label, highlighted (`f`+`ind`, `E`+`dit full`,
  `Del`+`ete`); otherwise the chip precedes the label (`x shell`). The row
  spells Delete as `Del`.
- **Overflow** clips at the row width.

## Keyless entries

An entry that describes an open input rather than a key (what the Select query
or an InputForm is for) comes from the mode that opened the input:
`SelectSpec.input_hint` and `InputFormSpec.input_hint`. `render_bottom`
prepends the active mode's hint as the first, keyless entry. The function-panel
search input keeps its hint in `PanelSearch`.

## A captured keyboard

While an `InputCapture` is installed the keymap is bypassed, so the row is
rendered from the capture itself — the one state where the user most needs the
way out:

```
Ctrl-b release   ·   wasd cursor
```

- The release chip is `codec::key_name(cap.release)`, so it follows whatever the
  capturing extension configured.
- Each intent in `cap.escape` is looked up among the labelled bindings, so the
  escape chips are real keys with their real labels. An empty `escape` shows
  only the release chip.

This names no extension and reads no extension state; everything else on the
row is suppressed, because nothing else is reachable.

## Wiring

```rust
pub fn render_bottom(
    frame: &mut Frame, area: Rect, state: &AppState,
    keymap: &KeymapRegistry, conds: &Conditions, theme: &Theme,
);
```
