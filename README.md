# wasdf

**wasd/hjkl file manager for mini-keyboards.** A keyboard-only TUI file manager
(Rust · [ratatui](https://ratatui.rs) · [crossterm](https://github.com/crossterm-rs/crossterm))
that keeps your hands on the home row: **wasd** controls the file pane, **hjkl**
controls the function pane.

The split is by design: **wasd** is meant to be worked by the **left** thumb and
**hjkl** by the **right** thumb, so both clusters fall under the thumbs on a
mini/thumb keyboard and the two hands share the load symmetrically.

<p align="center">
<img src="https://github.com/user-attachments/assets/c1e27c90-5294-4335-9034-42da5421a43f" width="240">
<img src="https://github.com/user-attachments/assets/cacd4ad1-6657-4609-b998-e49e59381f1a" width="240">
</p>

Runs on **macOS**, **Linux**, and **Windows via WSL2**.

> **Early-stage concept.** The design is still settling and parts are
> unimplemented. The spec under [`doc/`](doc/) is authoritative and runs ahead of
> the code.

## Keys

| Keys | Action |
|------|--------|
| `w` `a` `s` `d` | move the cursor up / left / down / right |
| `h` `j` `k` `l` | control the preview |
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
rustup update
cargo run -p wasdf
```

Rust edition 2024. The Scheme REPL compiles its bootstrap at first launch and
caches the bytecode; the first cold start is slower.

**Platforms:** macOS, Linux, and Windows under **WSL2** (run it inside the WSL2
Linux environment, not native Windows). Optional dynamic extensions load as
`.dylib` on macOS and `.so` on Linux/WSL2.

## Layout

| Path | Contents |
|------|----------|
| [`doc/`](doc/) | The authoritative spec (start here) |
| [`crates/wasdf/`](crates/wasdf/) | The application |
| [`crates/wasdf-sdk/`](crates/wasdf-sdk/) | ABI-stable types for dynamic extensions |
| [`crates/wasdf-example-ext/`](crates/wasdf-example-ext/) | An example dynamic extension |

## License

Apache 2.0
