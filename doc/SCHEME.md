# Scheme Configuration

wasdf uses a Scheme-like language, evaluated by the steel engine
(`steel-core`), in three roles: the parser of all configuration, the glue
declaring each extension's keys, commands, and conditions, and the layouter.
steel runs as a resident session — a dedicated OS thread owns the `Engine`
(which is `!Send`/`!Sync`), and the kernel communicates with it via an mpsc
channel. Evaluation is synchronous from the caller's perspective (the call
blocks on the reply channel, with a timeout).

The engine stdlib loads at session startup on the dedicated thread. Boot is
non-blocking: the thread signals readiness via a Condvar after stdlib load
completes, and the kernel waits briefly before falling back to native embedded
defaults. There is no bytecode cache and no respawn — if the engine thread
exits, the session is silently disabled for the remainder of the process.

The embedded `.scm` configuration files are pure `(quote (...))` data
literals; they use no R7RS-specific features and evaluate on steel without
modification. Dynamic evaluation (`Plan::EvalScheme`) uses steel's built-in
procedures.

All encoding and decoding of intents, keys, and modes goes through the single
codec module in script (including a small s-expression reader for the session's
output); no other module parses or formats these strings. Any evaluation or
parse failure — and a not-yet-ready session — falls back to the
native embedded defaults instead of dropping registrations.

## Role 1: Configuration Parser

- The only source of truth for defaults is the embedded Scheme configuration:
  the default keymaps, default commands, and the default resolver schema.
  There are no parallel Rust tables.
- User configuration lives in the user configuration directory. The
  keybindings file (`keybindings.scm`) is written out as a fully commented-out
  template; the user uncomments only the lines they change, so upgraded
  defaults take effect and collision warnings stay quiet. Each line is a
  `(bind <mode> <panel|#f> "<key>" <intent> [<when>])` form; the uncommented
  forms load at the User layer ([UI.md](UI.md)). The config directory is
  `$WASDF_CONFIG_DIR`, else `$XDG_CONFIG_HOME/wasdf`, else `~/.config/wasdf`.

Configuration forms (all plain Scheme s-expressions):

| Form | Declares | Elements |
|------|----------|----------|
| keymap | key bindings | a mode scope, an optional panel scope, and bindings of key, intent, and an optional when condition |
| defcommand | a palette command | name, description, intent, optional when condition |
| defresolvers | resolver entries | per entry: a key, an optional destructive flag, ordered candidates of a kind plus argv elements (literal strings or the placeholder symbols src, dst, path, paths, opts), and an optional trailing list of selectable options `((token label) …)` offered by the TUI flag-picker — see [ARCHITECTURE.md](ARCHITECTURE.md) |
| deflayout, defsublayout | layouts | an id (and a parent panel for sublayouts) and a node tree of rows, columns, and panel leaves, each with optional size, minimum height, when condition, and sublayout attributes |
| deffocus-order | the focus cycle | a mode and an ordered list of panel ids |
| set-option | options | a flat key-to-value store, for example the preview auto-update flag or the per-extension enable flag; unknown keys warn |

Key strings name a single character, a special key (Enter, Escape, Tab,
Space, the arrows), or a modifier chain with Ctrl, Shift, and Alt. Mode
scopes name File, Policy, Select as a whole, a single Select instance, or an
extension mode; Select scopes match by longest id prefix.

## Role 2: Extension Glue

Each extension's Scheme source — evaluated in the session at registration —
declares its entry bindings into core modes, its palette commands, its when
conditions, and its extension intent expressions. Rust stays the how
(rendering, intent handling, plan construction); Scheme is the what and when.

When conditions fall into three evaluation classes. The input path never
blocks on the Scheme session.

| Class | Examples | Evaluated |
|-------|----------|-----------|
| Static, environment-only | executable presence, environment variables | once at load time in the session; the result is baked into the binding |
| Dynamic predicate | selection presence, archive test, select phase | a closed grammar — a registered predicate with literal arguments, combined with and, or, not — parsed once into an AST and evaluated natively in Rust per keypress; predicates read AppState implicitly |
| Dynamic arbitrary expression | anything else | asynchronously against the live session as an EvalScheme plan, with the result cached; a keypress sees the cache |

Defaults are false: a condition whose evaluation fails, and an arbitrary
expression whose cache is not yet populated, both count as false (the binding
is inert). The cache is refreshed only by explicit re-evaluation on
state-change events; there is no time-based expiry.

Intent arguments that are expressions (for example, composing an editor
command from the environment and the cursor path) are likewise evaluated
asynchronously at fire time; literal arguments make no round trip.

Builtins available to configuration: environment lookup, executable test,
string concatenation, path joining, the cursor path, the current directory,
the selected paths, and the dynamic predicates for selection, mode, select
phase, sublayout, and function panel visibility.

## Role 3: Layouter

Layouts are declared with the layout forms above. Sizes are fractional
strings, automatic, or the ratio-left and ratio-right pair that reads the
shared function panel ratio from AppState. Layout when conditions use the
same native condition evaluator as keymaps; rendering makes no Scheme round
trip. See [UI.md](UI.md) for the two core built-in layouts and the panel
catalog.

## Session Lifetime

The engine thread runs for the life of the process. If it exits (engine
panic), the session becomes permanently unavailable — all subsequent `eval`
calls return a "thread gone" error and the fallback native defaults remain
active. There is no respawn.

## Error Handling

| Error | Behavior |
|-------|----------|
| Parse error | surfaced as a notification; the script is skipped |
| Evaluation error | surfaced as a notification; the expression is skipped |
| Protocol mismatch | warning; fall back to embedded defaults |
| Session death | session permanently disabled; native defaults remain active |

MVP limits: no macro definition, limited function definition, conditionals
only via when clauses, and a flat option store without a nested configuration
tree.
