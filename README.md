# strimux

> Infinite spatial canvas terminal multiplexer

**strimux** is a terminal-native, daemon-free terminal multiplexer.
It aims to provide niri's developer experience in Windows and MacOS.

## Status

**M0 working.** The single-process, multi-pane PTY renderer is implemented and
interactively usable: it spawns real panes, composes them into one 2D cell
buffer, repaints a full 300x80 frame in ~0.05 ms, streams pane output as it
arrives, and supports a content-width / horizontal-overflow model. The pure
layout core (`strimux-layout`) is property-tested; the emulator facade
(`strimux-term`) is unit-tested; an end-to-end test drives a live PTY through
`render_frame`. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
[`docs/ROADMAP.md`]().

## Build & install

The easiest way is one command from the repo:

```sh
# from inside the repo (installs to wherever `cargo` puts binaries, e.g. ~/.cargo/bin)
cargo install --path crates/strimux
```

If you already built a release binary and just want it on your PATH:

```sh
cargo build --release              # -> target/release/strimux
make install                       # copies to a writable bin dir on your PATH
```

> `make install` picks the first writable `bin` directory on your PATH
> (falling back to `~/.local/bin`), so `strimux` is runnable right away even
> when `~/.cargo/bin` is not on your PATH.

## Usage

```sh
strimux                     # start a session: one strip of 4 quarter-width panes
strimux run "htop"          # same 4 panes; the first runs htop, the rest your shell
```

Launching opens a single **strip of 4 columns**, each `1/4` of the viewport
width, each running your shell (`run` puts its command in the first pane).
All four spawn up front so the screen is usable immediately.

Inside a session, navigation uses a `Ctrl-b` prefix so it works on **every**
terminal with **zero configuration** (including macOS, where the Option key
does not become Alt by default). Press `Ctrl-b`, then a command key; the status
line shows the pending prefix.

| Keys (`Ctrl-b` then...) | Action |
| --- | --- |
| `Ctrl-b` `h` / `l` / `k` / `j` | move focus across panes |
| `Ctrl-b` `H` / `L` / `K` / `J` | move the focused pane |
| `Ctrl-b` `c` / `r` | new column to the right / new row below |
| `Ctrl-b` `;` | spawn a new agent pane to the right and focus it |
| `Ctrl-b` `s` | split the focused column below |
| `Ctrl-b` `z` | cycle column width |
| `Ctrl-b` `x` | kill the focused pane |
| `Ctrl-b` `,` / `.` | scroll the pane horizontally across overflow |
| `Ctrl-b` `[` / `]` | scroll the row viewport left / right |
| `Ctrl-b` `1..9` | jump focus to a column |
| `Ctrl-b` `q` | quit (kills all panes) |
| `Ctrl-b` `Ctrl-b` | send a literal `Ctrl-b` to the pane |
| `Esc` | cancel the pending prefix |

The equivalent `Alt` chords remain available for terminals where
Option-as-Alt is configured (`Alt+hjkl` to focus, `Alt+Shift+hjkl` to move,
`Alt+a` new column, `Alt+;` spawn an agent pane, `Alt+s` split, `Alt+z` width,
`Alt+x` kill, `Alt+q` quit).

All other keys pass through to the focused pane.

## Why

strimux is an alternative to tmux if you prefer scroll tiling over fixed tiling.
scrollable tiling is superior for smaller screens such as those in laptops.
no compositor, no GUI, on any OS. See [`docs/COMPARISON.md`](docs/COMPARISON.md).

## Non-goals (read before filing a feature request)

- **No daemon / detach / attach.** no need for persistence layer, keeping it lightweight
- No free 2D canvas (we are a structured grid of strips). use cmux instead of strimux
- No floating panes, no mouse-driven chrome, no plugin system. just faithful keybind DX

## Development

```sh
cargo build --release
cargo test --workspace      # layout property tests + unit tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Layout invariants live as `proptest` properties in
`crates/strimux-layout/tests/invariants.rs`.

### Hot module reload (develop strimux inside strimux)

strimux ships a small hot-reload scaffold so you can edit its own code from
inside a running session (e.g. with jcode) and see the change live, without the
session ever dying:

```sh
make dev-hmr      # pane 1: watches crates/strimux-core/src, rebuilds the dylib on save
make hmr          # pane 2: the host; hot-swaps the core whenever the dylib changes
```

- The **host** (`strimux-hmr`) owns raw mode, input, the frame buffer, and all
  session state (focus, layout). It `dlopen`s `target/debug/libstrimux_core.dylib`.
- The **core** (`crates/strimux-core`, a `cdylib`) holds the logic you iterate
  on: `handle_key` and `render`. Session state is borrowed in per call, so a
  swap is lossless.
- `strimux-core-api` is the stable boundary both compile against; it never
  recompiles on a reload, keeping each cycle a fast one-crate rebuild.

Edit a core file, save, and the running host shows the new behavior in under a
second. Bump `LABEL` in `crates/strimux-core` to visibly confirm the swap in the
status line. `q` quits the host.

## License

MIT. See [LICENSE](LICENSE).
