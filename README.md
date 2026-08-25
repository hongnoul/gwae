# strimux

> Your CLI agents, on a niri strip. No compositor required.

**strimux** is a terminal-native, daemon-free multiplexer for CLI agents. Claude
Code, Jcode, and any other TUI each own a terminal; strimux gives them room: an
**infinite 2D grid of strips** where every pane keeps its full, natural size.
New panes slot in to the right of a row, you scroll the viewport across, and
unlimited named rows stack below. It runs in any terminal on **macOS, Windows,
and Linux**.

- Panes never shrink. No cramming; agents stay readable at full size.
- Keyboard-first: `Alt+hjkl` moves focus, `Alt+Shift+hjkl` moves panes.
- Agent-aware via the standard **OSC 133** protocol only - panes stay ordinary
  PTY TUIs. The minimap colors agents and `Alt+g` jumps to the one that needs you.
- Single process, **no daemon**, no attach/detach. Persistence is each
  harness's own `--resume`.

## Status

Scaffold / M0. The pure layout core (`strimux-layout`) is implemented and
property-tested; the PTY render loop is the next milestone. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/ROADMAP.md`]().

## Why

tmux divides a fixed screen; strimux scrolls an infinite one. niri's no-shrink
scrollable tiling, as a home for your CLI agents, in a plain terminal - no
compositor, no GUI, on any OS. See [`docs/COMPARISON.md`](docs/COMPARISON.md).

## Non-goals (read before filing a feature request)

- **No daemon / detach / attach.** Your agents already `--resume` themselves.
- No free 2D canvas (we are a structured grid of strips).
- No floating panes, no mouse-driven chrome, no plugin system.

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
