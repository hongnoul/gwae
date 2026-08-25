<div align="center">

# strimux

[![Latest Release](https://badgen.net/github/release/hongnoul/gwae?icon=github)](https://github.com/hongnoul/gwae/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![CI](https://github.com/hongnoul/gwae/actions/workflows/ci.yml/badge.svg)](https://github.com/hongnoul/gwae/actions/workflows/ci.yml)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-blue?style=flat-square)](https://github.com/hongnoul/gwae/releases)

**niri's scrolling tiling, for your CLI agents, in any terminal**

A terminal-native, daemon-free multiplexer for people who live in concurrent CLI agents. An infinite 2D grid of strips where panes never shrink.

[Website](https://hongnoul.github.io/gwae/) · [Install](#install) · [Quick Start](#quick-start) · [Keyboard Shortcuts](#keyboard-shortcuts) · [FAQ](#faq) · [Docs](docs/)

</div>

---

## Features

**Panes never shrink**
Fixed-width columns (`1/4` by default, `1/3 → 1/2 → 1/4` on `⌥+r`). A row that outgrows the screen scrolls past the viewport edge — it doesn't cram. Strips stack infinitely downward.

**Quantized scroll**
The viewport rests only on column boundaries, so the same grid paints pixel-identically in every scroll state. No slivers, no wobble — verified end-to-end at hostile widths like 342 columns.

**Agent dashboard**
Panes speak standard [OSC 133](https://gitlab.freedesktop.org/terminal-wg/specifications/-/blob/master/docs/OSC-133.md) — zero instrumentation. Hold `⌥` for a minimap tinted by status (working / wants attention / done / failed) and press `⌥+g` to smart-jump to the pane that needs you.

**Single process, no daemon**
No socket, no attach/detach. Crashing one pane's emulator can't take the TUI down. Persistence is each harness's own `--resume`.

**Kitty graphics passthrough**
`kitten icat` and agent screenshots render inside their pane — APC sequences forwarded verbatim and clipped to the pane rect.

**Mouse that helps, and stays out of the way**
Click to focus, drag to copy. Full-screen apps that ask for mouse reporting (vim, agent TUIs) get every event forwarded in their own coordinates, so the wheel behaves natively inside them. strimux claims no wheel of its own.

**Guided onboarding**
`strimux init` walks theme, layout, chrome, and agent setup with a live mockup under every question. Catppuccin Mocha by default, 8 theme presets, `⌥+t` previews them live on the running session.

**Cross-platform**
Any terminal on macOS, Linux, and Windows (ConPTY). 256-color minimum; truecolor, synchronized updates, kitty keyboard protocol, and SGR mouse are auto-detected and gracefully degraded.

## Install

### Shell script (recommended)

```bash
# macOS & Linux
curl -fsSL https://raw.githubusercontent.com/hongnoul/gwae/main/scripts/install.sh | bash
```

Downloads the latest prebuilt binary for your platform to `~/.local/bin` (override with `STRIMUX_INSTALL_DIR`), verifying the checksum.

On Windows, grab `strimux-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/hongnoul/gwae/releases/latest) and put `strimux.exe` on your `PATH`.

### Prebuilt binaries

Every release ships static binaries with SHA-256 checksums for:

| Platform | Asset |
|---|---|
| macOS (Apple Silicon) | `strimux-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `strimux-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64, musl static) | `strimux-x86_64-unknown-linux-musl.tar.gz` |
| Linux (aarch64, musl static) | `strimux-aarch64-unknown-linux-musl.tar.gz` |
| Windows (x86_64) | `strimux-x86_64-pc-windows-msvc.zip` |

### From source

Rust 1.85+:

```bash
cargo install --git https://github.com/hongnoul/gwae strimux

# or from a checkout
git clone https://github.com/hongnoul/gwae && cd gwae
cargo install --path crates/strimux     # -> ~/.cargo/bin/strimux
# or: make install                      # first writable bin dir on PATH
```

Packaging scaffolding for Homebrew, AUR, and Nix lives in [`packaging/`](packaging/).

## Quick start

```sh
strimux                     # one strip, one 1/4-width pane + placeholder boxes
strimux run "claude"        # command runs in column 0, rest are shells ($SHELL)
strimux new -- htop         # (subcommand form) new column in a fresh session
strimux init                # guided setup: theme, layout, chrome (safe to re-run)
strimux setup               # optional per-terminal bindings (e.g. Cmd+hjkl on iTerm2/kitty)
strimux doctor              # diagnostics: config + theme validity, layout smoke
```

The default layout is one strip, one quarter-width column. Skeleton placeholders tile the empty right side so the 4-column container always reads. New columns appear to the right of the focused pane, not at the strip end. `⌥+;` spawns your agent in a new column and focuses it.

## Why strimux?

I run a lot of coding agents in parallel, and every multiplexer I tried divides a fixed screen: more agents means smaller panes, until nothing is readable. niri solved this on the desktop with scrolling tiling — columns keep their size and the viewport moves instead. strimux brings that model into the terminal you already use.

tmux divides a fixed screen; strimux scrolls an infinite one. Séance and tairi have the niri model but need a GUI or compositor; strimux runs over SSH, in any terminal, on all three platforms. See [`docs/COMPARISON.md`](docs/COMPARISON.md).

| Project | Layout | In a terminal? | Detach? | Platforms |
|---|---|---|---|---|
| tmux | plane tiling | Yes | Yes (server) | macOS/Linux/*BSD |
| Zellij | plane tiling + floating | Yes | Yes | macOS/Linux/Windows |
| Séance | niri strip (GUI) | No | socket | Linux (GTK) |
| tairi | niri strip (GUI) | No | workspaces | macOS |
| **strimux** | **2D niri strip grid** | **Yes** | **No (`--resume`)** | **macOS/Windows/Linux** |

**No-shrink** — agents stay readable. **Niri feel** — `⌥+hjkl` / `⌥+Shift+hjkl`, dynamic strips, quantized stops, minimal follow-focus. **No daemon** — if you need SSH persistence that outlives the process, keep tmux; strimux delegates to `claude --resume` / `jcode --resume`.

## Documentation

- [Architecture](docs/ARCHITECTURE.md): single process, no client-server; layout core, pane tasks, composer, renderer
- [Layout spec](docs/LAYOUT-SPEC.md): the normative quantized-scroll / strip-grid spec
- [Configuration](docs/CONFIG.md): every key, generated from code
- [Comparison](docs/COMPARISON.md): tmux, Zellij, Séance, tairi
- [Latency](docs/LATENCY.md): input-latency tuning, macOS + terminal + strimux together
- [Roadmap](docs/ROADMAP.md)

## Keyboard Shortcuts

Every action is an `⌥` chord. `⌥` is the Option key on macOS, Alt elsewhere. It works when the terminal delivers Option as Meta/Alt, plus a fallback that decodes macOS's Unicode glyphs (`…` for `⌥+;`, `œ` for `⌥+q`, `ÓÔÒ` for `⌥+Shift+hjkl`) with no "Option as Alt" toggle.

| `⌥` chord | Action |
|---|---|
| `⌥+h` / `⌥+l` | Focus left / right (adjacent column; scrolls minimally to reveal) |
| `⌥+k` / `⌥+j` | Focus up / down within stack; at edge crosses strips. Past the last strip creates an empty strip (niri workspace semantics); leaving an empty strip discards it |
| `⌥+Shift+h` / `⌥+Shift+l` | Move focused column left / right (swap with neighbor) |
| `⌥+Shift+k` / `⌥+Shift+j` | Move pane up / down within stack; at the stack edge carries the pane to the neighboring strip (creating one past the end), discarding an emptied strip |
| `⌥+Enter` | New column to the right of focused |
| `⌥+Shift+Enter` | New strip (row) below the focused one |
| `⌥+;` (`…` on macOS) | Spawn agent column right of focus and focus it (`default_agent`, or pick one if unset) |
| `⌥+Shift+;` (`Ú` on macOS) | Spawn agent on a new strip below the focused one |
| `⌥+s` | Split focused column — new pane below |
| `⌥+r` | Cycle focused column width `1/3 → 1/2 → 1/4` |
| `⌥+f` (`ƒ` on macOS) | Toggle focused column between full width and `1/4` |
| `⌥+q` (`œ`) | Kill focused pane — columns compact, focus keeps its slot (falls left only at right edge); emptied strip is dropped, last pane quits strimux |
| `⌥+←` / `⌥+→` | Scroll pane's logical content horizontally (when `content_width` > column width) |
| `⌥+[` / `⌥+]` | Scroll the row viewport left / right without moving focus |
| `⌥+↑` / `⌥+↓` | **Scrollback** — read back through the focused pane's history, 3 rows a notch. `⌥+Shift+↑/↓` and `⌥+PageUp/PageDown` move ~a screenful. Typing snaps back to live. A full-screen app (vim, `less`) owns its own scrolling, so it gets the arrow keys instead |
| `⌥+←` / `⌥+→` | Pan wide content sideways when `content_width` exceeds the column (`⌥+Shift` for a bigger step) |
| `⌥+1` … `⌥+9` | Jump to column N in focused strip. Keep `⌥` down and keep typing to address columns past 9 (`⌥` + `1` `2` → column 12); the number commits when `⌥` is released, or after ~500ms on terminals that don't report the release |
| `⌥+g` (`©`) | **Smart-jump** — jump to pane that needs you (see below) |
| `⌥+t` (`†` on macOS) | **Theme picker** — step presets with `←`/`→`, live-previewed on the real UI; `⏎` keeps, `esc` restores |
| `⌥+/` or `⌥+?` (`÷` / `¿` on macOS) | **Toggle the cheat-sheet HUD** — same overlay shown at startup; any other key dismisses it |
| `⌥+Shift+q` | Force-quit strimux — opens a centered confirmation overlay; press `⌥+Shift+q` again (or `⏎`) to kill every pane, any other key cancels |
| click | Left-click focuses the clicked pane |
| drag | Left-drag inside a pane selects text (inverse highlight) and copies it on release. Panes that grab the mouse (vim, agent TUIs) keep it, so hold `Shift` there to select instead |
| wheel | Forwarded as SGR to a pane that asked for mouse reporting (vim, agent TUIs), so it behaves natively there. strimux claims no wheel of its own |

All other keys pass through to the focused pane. Closing a pane by `exit` / process death behaves identically to `kill-pane`.

Bindings live in one place, `crates/strimux/src/binds.rs`, and tests enforce that the dispatcher, the cheat-sheet HUD, the cowsay hints, and this table all agree — a new or re-bound key fails the build until every surface is consistent.

## Agent awareness

### OSC 133 status

If a pane's shell emits OSC 133 (`A` prompt → `C` running → `D;n` done/failed), strimux tracks per-pane status natively:

- `»` **Working** (blue) — command running
- `!` **Wants attention** (amber) — idle with output / prompt waiting
- `✓` **Done** (green) — exited 0
- `✗` **Failed** (red) — non-zero exit

Panes without shell integration fall back to a quiet heuristic: a pane silent for a few seconds flips to `!` so the dashboard still triages it.

### Minimap

No bottom status row. Hold `⌥`/Alt to see status (centered, no pane shrinkage): one row per strip, one tile per pane (width ∝ column share), tinted by status, focused tile accented, digit `⌥+1..9` per tile, summary like `5 »2 !1 ✓1 ✗1`. A HUD line names the pane that needs you: ` » 1.3 needs you — ⌥+g`.

| `minimap.mode` | Behavior |
|---|---|
| `off` *(default)* | No persistent chrome. `⌥` reveals the centered HUD + minimap |
| `overlay` | Legacy bottom-right overlay — `max_width`/`max_rows` apply |
| `edge_ticks` | Single-cell ticks on the outer frame, no box |

### Smart-jump

`⌥+g` jumps to the pane that needs you most — failed beats wants-attention beats done, nearest in layout order first, crossing strips and following with the scroll. Does nothing when every other pane is happily working.

## Configuration

`strimux init` runs a short guided setup on first launch (theme with live swatches, panes at launch, column width, scroll style, labels — each question drawn over a live mockup of the grid). Re-run it any time; defaults become whatever your config currently says.

File: `$XDG_CONFIG_HOME/strimux/strimux.toml` (or `~/.config/strimux/strimux.toml`). TOML, all keys optional. Full reference in [`docs/CONFIG.md`](docs/CONFIG.md).

```toml
default_column_width = "quarter"  # or "half", "two-thirds", "full", or 80 (cells)
content_width = 0                 # >0 gives panes a wider logical grid, panned with ⌥+←/→
default_agent = "claude"          # first pane + ⌥+; spawn this
startup_panes = 1
theme = "catppuccin-mocha"        # or nord, tokyo-night, gruvbox, rose-pine, dracula, ...
cell_labels = false

[minimap]
mode = "off"                      # off | overlay | edge_ticks
```

## FAQ

### How does it compare to tmux?

tmux is a fixed-screen tiler with a client-server daemon. strimux is a scrolling tiler with no daemon: columns keep their width and the row scrolls, so ten agents are as readable as two. If you need detach/attach that outlives the process, keep tmux; strimux delegates persistence to each harness's own `--resume`.

### What coding agents does it work with?

All of them. strimux hosts ordinary PTYs, so any agent that runs in a terminal works out of the box: Claude Code, Jcode, Codex, OpenCode, Gemini CLI, Aider, and anything else you can launch from a command line. Status tracking uses standard OSC 133 with a quiet-heuristic fallback, so no agent needs instrumentation.

### Does it need a specific terminal?

No. Minimum is 256-color plus cursor addressing. Truecolor, synchronized updates (`ESC[?2026h`), kitty keyboard protocol, and SGR mouse are auto-detected and gracefully degraded. It runs over SSH.

### Can I detach and reattach?

No, deliberately. There is no daemon and no socket. Your agents already persist themselves (`claude --resume`, `jcode --resume`); a shell that must survive the terminal belongs in tmux, which you can happily run inside a strimux pane.

### What's not planned?

No free 2D canvas (structured strips only), no floating panes, no plugin system, no overview zoom (post-1.0). See [Non-goals](docs/ROADMAP.md).

## Development

```sh
cargo build --release
cargo test --workspace          # layout proptests + unit tests + live-PTY E2E
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

CI runs fmt, clippy (`-D warnings`), check, and the full test suite on macOS and Linux, plus a Windows build gate, on every push and PR. Nightly PTY integration tests run the real binary through a live PTY. Tags matching `v*` build binaries for all five targets, checksum them, and publish a GitHub Release.

Architecture:

```
strimux (one process)
├── Layout core (rows/strips/columns/panes) — pure, no I/O
├── Pane tasks (one per PTY: bytes → parse → grid + OSC 133)
├── Composer (coalesce damage → single 2D cell buffer)
├── Render (diff buffer → batched ANSI, sync-update markers)
├── Input (raw mode: decode keys, route to pane or ⌥)
└── Minimap / smart-jump (per-pane status → chrome)
```

| Crate | Role |
|---|---|
| `strimux` | bin — raw mode, PTY hosting, composer, render, input, minimap, OSC 133, Kitty APC forwarding, mouse |
| `strimux-layout` | pure 2D grid + verbs + quantized scroll + minimap model. `proptest` invariants |
| `strimux-term` | `TermGrid` emulator facade + damage tracking |
| `strimux-testkit` | fake PTYs + scripted terminals + snapshot harness |

Layout invariants live as `proptest` properties in `crates/strimux-layout/tests/invariants.rs` (quantized-stop, tiling, shape-identity, focus-never-clipped, page-stop, cross-strip move).

## Contributing

- Open an [issue](https://github.com/hongnoul/gwae/issues) or [pull request](https://github.com/hongnoul/gwae/pulls)
- Read the [contributing guide](.github/CONTRIBUTING.md) and [docs/](docs/)
- Changes to keybindings must update `binds.rs` and this README together — the tests will hold you to it

## License

strimux is open source under the [MIT license](LICENSE).
