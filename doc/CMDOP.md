# CmdOp — the Command Builder

CmdOp is the **built-in** command builder for copy, move, rename, and create
(mkdir / touch). It is part of the kernel, not an extension: it is always
present, cannot be disabled, and is what makes the zero-extension wasdf a
usable file manager ([EXTENSION.md](EXTENSION.md)). Its keys and palette
commands live in the core sources (`app/keymap.scm`, `app/command.scm`), and
its id `cmdop` is core vocabulary. Delete is not part of it: `DeleteSelected`
runs the resolver's `delete` entry (`rm`) behind a Policy confirmation.

Internally it is implemented on the same form seam extensions use — the
reference **form**, next to preview's reference **content** role — and is
registered by the kernel before any extension. It assumes
[EXTENSION.md](EXTENSION.md) (forms, `render_form`) and [UI.md](UI.md) (the
function panel, focus).

## Seams

CmdOp uses only the generic form seams:

| Seam | Use |
|------|-----|
| `SetFunctionForm(Some("cmdop") / None)` | claim / release the panel (shows it and moves focus) |
| `render_form` + `form_scroll` | draw the builder into the panel's inner rect; core owns the border and title |
| `LoadContent` → `accept_content` | read the destination browser's directory (`Content::Dir`), gated on the browsed path |
| `LoadSearch` → `accept_content` | the browser's deep search |
| `BeginInput{purpose, seed, multiline}` → `input-submit` / `input-cancel` | every text entry, tagged by purpose |
| the resolver chain | the resolved command template and its options |
| `cmdop-active` predicate (`form_owner == "cmdop"`) | gates its keymap |

Its state (the `Form`) lives in interior mutability inside the extension. The
command's **sources** are always the file panel's live targets
(`AppState::targets` — selection, else cursor entry), so they keep following the
file cursor and marks while the builder is open.

## Keys

Entry keys, file panel: `c` copy, `m` move, `R` rename, `M` create. The palette
commands `copy`, `move`, `rename`, `mkdir`, `touch` start the same flows
(`mkdir` and `touch` both open create).

While the builder is active (`cmdop-active`), in the **file function** scope
(Core layer, gated by `cmdop-active`, so they win over the ungated core
bindings for the same keys by specificity):

| Key | Action |
|-----|--------|
| j / Down, k / Up | move the builder cursor |
| l / Right | descend into the directory under the cursor (on a file: confirm with it) |
| h / Left | ascend to the browsed directory's parent |
| Space | on an option: toggle the checkbox; on a listing entry: pin it as the destination (exclusive — pinning another entry moves the pin; Space on the pinned entry unpins it) |
| ; | jump the cursor between the options section and the listing |
| / | search / filter the destination listing (below) |
| Enter | on the builder row: edit the command line; on the body: run |
| k at the top of the options | move onto the builder row (j leaves it) |
| Esc, q | cancel and release the panel |
| , | cycle the panel ratio |

CmdOp is an **active** form: while it has focus, the keys core binds to
file-targeting intents (`wasd`, `W`/`S`, `Enter` → Activate) come to it as raw
`"key"` intents instead ([UI.md](UI.md), Focus Follows the Action). CmdOp
treats `w`/`s` like `k`/`j` and `a`/`d` like `h`/`l`. To move the file cursor
(and with it the command's sources) while the builder is open, `Tab` to the
file panel; the builder stays open and follows.

## Layout

Top to bottom inside the panel:

1. **CLI builder** — five wrapped rows showing the exact command line about to
   run: the resolver entry's argv template (from the resolver chain) with the
   checked options, the sources, and the destination spliced in.
2. **Options** — the command's options (declared, or extracted from
   `--help`/`man`) as checkboxes.
3. **Destination listing** — a browsable directory listing (copy / move). Each
   section has its own right-edge scrollbar; the title shows the operation and,
   while deep results are shown, the search query.

## Flows

**Copy / move.** The destination is, in order: the pinned entry, else the
entry under the cursor in the listing, else the browsed directory. Enter on the body emits `RunResolver` with the op, the
sources, the checked options, and the destination; the kernel's confirmation
gate asks when it would overwrite ([ARCHITECTURE.md](ARCHITECTURE.md)).

**Rename.** Opens a `prompt` input prefilled with the source's name and focuses
the panel; submit runs `rename`.

**Builder edit.** Enter on the builder row opens a `builder-edit` input seeded
with the live command line; submit releases the form and runs the edited argv.
Where it runs is the core option `cmdop.runner`:

| `cmdop.runner` | The command runs as |
|----------------|---------------------|
| `""` (default) | `Execute` — captured into the Exec frame |
| an extension id | `(ext "<id>" "run" {argv, cwd, label, escape})`, the **runner contract**; if that extension is not registered, a notice and `Execute` |

The runner contract is core's: any extension that implements `"run"` with
those fields can host builder commands (pty does — [PTY.md](PTY.md)). Core
never names the runner; the id is user data.

**Create.** `M` opens a **multi-line** `create-lines` input and no browser or
options; names are created in `cwd`. In a multi-line input Enter inserts a
newline unless the caret's line is empty, in which case it submits — a blank
line ends the batch. Each non-blank line becomes one `RunResolver`: a trailing
`/` makes `mkdir -p NAME`, otherwise `touch NAME`. The builder area previews the
exact invocations using the same line parser, so the preview cannot disagree
with what runs.

## Destination search

`/` in the browser is two-stage, deep then shallow:

- In a freshly browsed directory, `/` opens a submit-only `search-deep` input;
  submit emits `LoadSearch{owner: "cmdop", root: <browsed dir>, query}` — a
  recursive walk of the subtree ranked like file-search. Its landing (through
  `accept_content`) marks the listing as deep results.
- While deep results are shown, `/` opens a live `filter` input: substring
  narrowing over the landed results, applied per keystroke.
- Changing the browsed directory (descend / parent) clears the deep state, so
  `/` searches the new subtree.

The deep query (shown in the title) and the filter query are separate fields:
reusing one would re-narrow fuzzy-ranked results by a literal substring and
hide matches whose names do not contain it verbatim.
