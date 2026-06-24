# wasdf

**WASD/hjkl file manager for mini-keyboards.** A keyboard-only TUI file manager
(Rust · [ratatui](https://ratatui.rs) · [crossterm](https://github.com/crossterm-rs/crossterm))
that keeps your hands on the home row: **WASD** moves the cursor, **hjkl** drives
the preview — no arrow keys, no mouse. Made for thumb / tiny / mini keyboards.

> **Early-stage concept.** The design is still settling and parts are
> unimplemented. The spec under [`doc/`](doc/) is authoritative and runs ahead of
> the code.

## Keys

| Keys | Action |
|------|--------|
| `w` `a` `s` `d` | move the cursor up / left / down / right |
| `h` `j` `k` `l` | scroll the preview ◂ ▾ ▴ ▸ |
| `Enter` | enter directory / open file · `Space` toggle select |
| `f` · `x` | file search · command palette |
| `c` `m` `R` `e` | copy · move · rename · edit |
| `v` · `,` | cycle list layout (rows/columns/grid) · cycle preview pane |
| `q` | quit |

`Enter`/`y` mean OK, `Esc`/`n` mean cancel. Full keymap: [doc/UI.md](doc/UI.md).

## Design

Three pillars: **spec-driven layout & keys** ([doc/UI.md](doc/UI.md)); **everything
is an extension**, yet it boots usable with zero ([doc/EXTENSION.md](doc/EXTENSION.md));
a **resident R7RS Scheme REPL** ([stak](https://github.com/raviqqe/stak)) as the
config parser, extension glue, and layouter ([doc/SCHEME.md](doc/SCHEME.md)).

One pipeline drives it all — `key → Intent → plan → kernel async → AsyncResult →
reducer → render` — with a pure reducer, all I/O in async tasks, and a uniform
failure rule: **disable, notify, continue** ([doc/ARCHITECTURE.md](doc/ARCHITECTURE.md)).

## Build

```sh
cargo run -p wasdf
```

Rust edition 2024. The Scheme REPL compiles its bootstrap at first launch and
caches the bytecode; the first cold start is slower.

## Layout

| Path | Contents |
|------|----------|
| [`doc/`](doc/) | The authoritative spec (start here) |
| `crates/wasdf/` | The application |
| `crates/wasdf-sdk/` | ABI-stable types for dynamic extensions |
| `crates/wasdf-example-ext/` | An example dynamic extension |

## License

Apache 2.0
