# strimux

> Infinite spatial canvas terminal multiplexer

**strimux** is a terminal-native, daemon-free terminal multiplexer.
It aims to provide niri's developer experience in Windows and MacOS.

## Status

Scaffold / M0. The pure layout core (`strimux-layout`) is implemented and
property-tested; the PTY render loop is the next milestone. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/ROADMAP.md`]().

## Why

strimux is an alternative to tmux if you prefer scroll tiling over fixed tiling.
scrollable tiling is superior for smaller screens such as those in laptops.
no compositor, no GUI, on any OS. See [`docs/COMPARISON.md`](docs/COMPARISON.md).

## Non-goals (read before filing a feature request)

- **No daemon / detach / attach.** no need for persistence layer, keeping it lightweight
- No free 2D canvas (we are a structured grid of strips). cmux canvas view is an overkill
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
