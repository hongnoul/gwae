# gwae


<p align="center">
  <img src="docs/assets/gwae.svg" alt="gwae" width="100" />
</p>

<p align="center">
  <a href="#macos-and-linux">macOS / Linux</a> · <a href="#windows">Windows</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-white?labelColor=black" alt="MIT license" /></a>
  <a href="https://github.com/hongnoul/gwae/releases"><img src="https://img.shields.io/github/downloads/hongnoul/gwae/total?labelColor=black&color=white" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/hongnoul/gwae/stargazers"><img src="https://img.shields.io/github/stars/hongnoul/gwae?labelColor=black&color=white&logo=github&logoColor=black" alt="GitHub stars" /></a>
  <a href="https://github.com/hongnoul/gwae/releases/latest"><img src="https://img.shields.io/github/v/release/hongnoul/gwae?label=release&labelColor=black&color=white" alt="latest stable release" /></a>
</p>

![gwae demo: agents on an infinite no-shrink strip grid](docs/assets/gwae-demo.gif)

## Install

### macOS and Linux

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

or with Homebrew (macOS):

```bash
brew install hongnoul/tap/gwae
```

### Windows

```powershell
irm https://hongnoul.github.io/gwae/install.ps1 | iex
```

## Tutorial

Copy and paste following into your preferred agent harness pane in gwae:

```text
You are teaching me gwae, a scrolling terminal multiplexer. I am inside a
gwae pane talking to you. Walk me through each step below one at a time:
explain what to press, wait for me to confirm it worked, then move on.
Do not skip ahead and do not dump all steps at once.

Setup: gwae init (guided setup), gwae doctor (health check, exit 0 when ok).
Config: ~/.config/gwae/gwae.toml, all keys optional, live-reloads on save.

1. Open panes: have me press ⌥+Enter 6 times. Point out the
viewport scrolling right while every pane keeps its width.
2. Move focus: have me try ⌥+h/l (left/right) and ⌥+k/j (up/down,
crossing strips). Mention click-to-focus, drag-to-copy, wheel scroll.
3. Reshape: have me try ⌥+r (cycle width), ⌥+f (full-width toggle),
⌥+b (split below), ⌥+Shift+Enter (new row), ⌥+Shift+h/l (move pane).
4. Spawn agents: have me press ⌥+; to spawn an agent (first press offers what
is installed and remembers it). Mention ⌥+Shift+; (picker), gwae run
"claude" (CLI start), ⌥+d (spawn directory).
5. Find stuck panes: have me hold ⌥ for the dashboard (»
working, ! needs input, ✗ failed), then ⌥+g to jump to the neediest
pane. Mention clicking a tile focuses it.
6. Help and quit: show ⌥+/ (cheat sheet), ⌥+w (keep-awake, macOS),
⌥+q (close pane), ⌥+Shift+q (quit; last pane exiting quits gwae).
```

## Development

```bash
git clone https://github.com/hongnoul/gwae
cd gwae
cargo build
cargo test --workspace
```

## Benchmark

| Metric    | Method                                                        | gwae        | herdr 0.9.1   |
| --------- | ------------------------------------------------------------- | ----------- | ------------- |
| Echo p50  | Keystroke to visible echo in a real PTY harness, median       | 0.34 ms     | 0.48–0.50 ms  |
| Echo p90  | Keystroke to visible echo in a real PTY harness, 90th pct     | 0.36–0.38 ms | 0.54 ms      |
| Spawn p50 | New pane request to first painted frame, median               | 0.48–0.52 ms | 0.63–0.70 ms |

## Agent Instructions

if you are an ai agent driving gwae, read [`docs/agents.md`](docs/agents.md) before spawning panes.
