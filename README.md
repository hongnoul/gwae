<div align="center">

<img src="docs/assets/gwae.svg" alt="gwae logo" width="128">

# gwae

[![Latest Release](https://badgen.net/github/release/hongnoul/gwae?icon=github)](https://github.com/hongnoul/gwae/releases)
[![CI](https://github.com/hongnoul/gwae/actions/workflows/ci.yml/badge.svg)](https://github.com/hongnoul/gwae/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

**A scrolling terminal multiplexer. Panes never shrink.**

Run coding agents, shells, and TUIs side by side. Open more panes and the viewport scrolls instead of squeezing them. Inspired by niri's scrolling tiling.

[Install](#install) · [Try it in 30 seconds](#try-it-in-30-seconds) · [Website](https://hongnoul.github.io/gwae/) · [Releases](https://github.com/hongnoul/gwae/releases)

<img src="docs/assets/gwae-demo.gif" alt="gwae demo: agents on an infinite no-shrink strip grid" width="900">

Add columns beyond the screen edge, then move between them without shrinking the panes.

</div>

## Install

### macOS

```bash
brew install hongnoul/tap/gwae
```

### Linux

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

The script also works on macOS. It installs to `~/.local/bin` and adds it
to your shell PATH, so `gwae` works in a fresh terminal. Override with
`GWAE_INSTALL_DIR`; `GWAE_NO_MODIFY_PATH=1` skips the PATH change.

### Windows

Experimental native build via ConPTY.

```powershell
irm https://hongnoul.github.io/gwae/install.ps1 | iex
```

Installs to `~/bin` and adds it to your user PATH.

<details>
<summary>Other install methods</summary>

```bash
cargo install gwae
```

```powershell
scoop bucket add gwae https://github.com/hongnoul/scoop-bucket; scoop install gwae
```

Or download from [Releases](https://github.com/hongnoul/gwae/releases/latest).

</details>

## Try it in 30 seconds

After installing, run `gwae`. On first launch, dismiss the help overlay with Escape and finish the setup in the first pane. No agent account needed: skip the agent choice for a shell-only test.

1. Press `Alt+Enter` a few times to add shell panes. Keep going past the screen edge: the viewport scrolls, the panes keep their width.
2. Use `Alt+h` / `Alt+l` to move left / right. Press `Alt+r` to cycle the focused column's width.
3. Press `Alt+/` for help. Type `exit` in each shell when you're done.

On macOS, use Option (`⌥`) instead of Alt. If it types special characters instead of triggering shortcuts, configure Option as Alt/Meta in your terminal's settings. `gwae doctor` checks gwae's configuration.

### Bring your agents

Use an already-installed CLI agent, or run your usual shell tools:

```bash
gwae init             # theme and layout setup, safe to re-run
gwae run "claude"     # start with Claude Code in the first pane
gwae run "codex"      # or Codex CLI
gwae doctor           # check config and setup
```

New columns appear to the right of focus. `⌥+;` spawns your configured agent, or offers a picker of installed agents when none is configured.

## Why not tmux?

Choose gwae when you want readable panes that scroll beyond the screen, rather than more splits in the same space. Keep tmux when you need detach/attach or processes that survive a disconnected terminal. gwae has no session daemon, and an agent's `--resume` restores its conversation, not its running process.

[Compare layouts and tradeoffs](docs/COMPARISON.md).

## How it works

* Panes keep fixed width (`1/4` default, `⌥+r` to cycle). Rows scroll past the edge, they do not squeeze.
* Scroll snaps to column boundaries. No slivers.
* One process. No daemon, no socket. Agent persistence is `claude --resume` or `jcode --resume`.
* Any terminal on macOS and Linux. Windows builds natively via ConPTY, experimental.
* Kitty graphics forwarded and clipped to pane.

## Agent status

Reads standard [OSC 133](https://gitlab.freedesktop.org/terminal-wg/specifications/-/blob/master/docs/OSC-133.md) markers when available. Otherwise, output activity and idle time provide a heuristic, not a guarantee that an agent needs input.

`»` working · `!` needs input · `✓` done · `✗` failed

Hold `⌥` for dashboard. `⌥+g` jumps to the pane that needs you.

<img src="docs/assets/gwae-attention.gif" alt="gwae Option-G smart-jump: hold Option to reveal the dashboard, tap Option-G to jump to the pane that needs attention" width="900">

[Watch the attention demo as MP4](docs/assets/gwae-attention.mp4).

## Keys

All chords use `⌥` on macOS, `Alt` elsewhere. Other keys go to the focused pane.

```
⌥+Enter        new column to right of focus
⌥+Shift+Enter  new strip below
⌥+;            spawn agent
⌥+h/j/k/l      focus left/down/up/right
⌥+Shift+h/j/k/l move pane
⌥+g            jump to pane that needs attention
⌥+t            theme picker
⌥+w            keep Mac awake (focus ring turns red)
⌥+/            help
⌥+q            kill pane
click          focus pane
drag           select and copy
wheel          scroll this pane's history (child TUIs keep their own)
Ctrl+Shift+J/K scroll this pane's history three lines, like jcode's default (in an agent pane the harness keeps the chord and its own speed)
Ctrl+J/K       always reach the pane (jcode: prompt jump); gwae never claims them
```

Full key reference: press `⌥+/` in gwae. [Keybinding design notes](docs/KEYBINDS.md).

## Config

File: `~/.config/gwae/gwae.toml` (`$XDG_CONFIG_HOME/gwae/gwae.toml`). All keys optional.

```toml
default_column_width = "quarter"
theme = "catppuccin-mocha"
default_agent = "claude"
startup_panes = 1
```

See [docs/CONFIG.md](docs/CONFIG.md).

## Docs

[Why gwae](docs/WHY.md) · [Architecture](docs/ARCHITECTURE.md) · [Layout spec](docs/LAYOUT-SPEC.md) · [Latency](docs/LATENCY.md) · [Comparison](docs/COMPARISON.md)

## Help gwae grow

If gwae fits your workflow, [give it a star](https://github.com/hongnoul/gwae). Found a rough edge? [Report it](https://github.com/hongnoul/gwae/issues/new/choose) with your OS and terminal so we can improve the next person's first run.

## License

[MIT](LICENSE)
