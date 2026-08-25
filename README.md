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

```sh
cargo build --release          # -> target/release/strimux
make install                   # copies it to ~/.cargo/bin/strimux (optional)
```

## Usage

```sh
strimux                     # start a session with your $SHELL in the first pane
strimux run "htop"          # start a session running a specific command
```

Inside a session:

| Keys | Action |
| --- | --- |
| `Alt+h` / `Alt+l` / `Alt+k` / `Alt+j` | move focus across panes |
| `Alt+Shift+h/j/k/l` | move the focused pane |
| `Alt+a` / `Alt+Enter` | new column to the right |
| `Alt+s` | split the focused column below |
| `Alt+r` / `Alt+z` | cycle column width |
| `Alt+x` | kill the focused pane |
| `Alt+Left` / `Alt+Right` | scroll the pane horizontally across overflow content |
| `Alt+[` / `Alt+]` | scroll the row viewport left / right |
| `Alt+1..9` | jump focus to a column |
| `Alt+q` | quit (kills all panes) |

All other keys pass through to the focused pane. `Alt` is the universal
modifier; no per-terminal config is needed.

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

## License

MIT. See [LICENSE](LICENSE).
