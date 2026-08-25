# strimux

> Infinite spatial canvas terminal multiplexer

**strimux** is a terminal-native, daemon-free multiplexer for CLI agents. Claude
Code, Jcode, and any other TUI each own a terminal; strimux gives them room: an
**infinite 2D grid of strips** where every pane keeps its full, natural size.
New panes slot in to the right of a row, you scroll the viewport across, and
unlimited named rows stack below. It runs in any terminal on **macOS, Windows,
and Linux**.

- Panes never shrink. No cramming; agents stay readable at full size.
- Keyboard-first: `⌥+hjkl` (the **Option** key on macOS, Alt elsewhere) moves
  focus, `⌥+Shift+hjkl` moves panes.
- Agent-aware via the standard **OSC 133** protocol only - panes stay ordinary
  PTY TUIs. The minimap colors agents and `⌥+g` jumps to the one that needs you.
- Single process, **no daemon**, no attach/detach. Persistence is each
  harness's own `--resume`.

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
terminal with **zero configuration** (including macOS, where the Option key `⌥`
does not become Alt by default). Press `Ctrl-b`, then a command key; the status
line shows the pending prefix.

| Keys (`Ctrl-b` then...) | Action |
| --- | --- |
| `Ctrl-b` `h` / `l` / `k` / `j` | move focus across panes (`j`/`k` cross strips; past the last strip creates a new empty one, niri-style) |
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

The equivalent `⌥` (Option) chords remain available for terminals where
the modifier arrives as Meta/Alt (`⌥+hjkl` to focus, `⌥+Shift+hjkl` to move,
`⌥+a` new column, `⌥+;` spawn an agent pane, `⌥+s` split, `⌥+z` width,
`⌥+x` / `⌥+q` kill the focused pane).

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

## License

MIT. See [LICENSE](LICENSE).
