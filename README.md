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

```text
SETUP
  run: gwae init (guided setup), gwae doctor (health check, exit 0 when ok).
  config: ~/.config/gwae/gwae.toml, all keys optional, live-reloads on save.

1. SCROLL, DON'T SHRINK
  press ⌥+Enter 6 times. the viewport scrolls right; every pane keeps
  its width.

2. MOVE LIKE VIM
  ⌥+h/l: focus left/right. ⌥+k/j: focus up/down, crossing strips.
  click any pane to focus it. drag to copy, wheel scrolls.

3. RESHAPE
  ⌥+r: cycle the focused column's width. ⌥+f: full-width toggle.
  ⌥+b: split below. ⌥+Shift+Enter: new row below.
  ⌥+Shift+h/l: move a pane between columns.

4. AGENTS
  ⌥+;: spawn your agent (first press offers what is installed and
  remembers it). ⌥+Shift+;: picker every time.
  gwae run "claude": start from the CLI. ⌥+d: pick the spawn directory.

5. NEVER HUNT THE STUCK ONE
  hold ⌥: dashboard with every pane's status (» working, ! needs input,
  ✗ failed). ⌥+g: jump to the neediest pane. click a tile to focus it.

6. KNOW EVERYTHING
  ⌥+/: cheat sheet. ⌥+w: keep-awake toggle (macOS). ⌥+q: close pane,
  ⌥+Shift+q: quit. last pane exiting quits gwae.
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
