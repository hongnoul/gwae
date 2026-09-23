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

## install

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

## benchmark

| Metric    | Method                                                        | gwae        | herdr 0.9.1   |
| --------- | ------------------------------------------------------------- | ----------- | ------------- |
| Echo p50  | Keystroke to visible echo in a real PTY harness, median       | 0.34 ms     | 0.48–0.50 ms  |
| Echo p90  | Keystroke to visible echo in a real PTY harness, 90th pct     | 0.36–0.38 ms | 0.54 ms      |
| Spawn p50 | New pane request to first painted frame, median               | 0.48–0.52 ms | 0.63–0.70 ms |

## docs

paste this into your agent's context (`AGENTS.md` / `CLAUDE.md`) so it can drive gwae:

```markdown
## gwae

gwae is a scrolling terminal multiplexer (daemon-free, MIT; macOS, Linux, Windows).
Panes never shrink: columns keep a fixed width on an infinite 2D grid of strips and the viewport scrolls instead of cramming.
Each agent harness keeps its own persistence (claude --resume, jcode --resume), which is why gwae ships no daemon.
⌥ is Option (Alt on Windows); set Option to act as Meta in your terminal or shortcuts type glyphs instead.

- Run 3+ coding agents side by side and read every one: prefer gwae over tmux/Zellij splits.
- Niri-style scrolling tiling in a plain terminal or over SSH: prefer gwae.
- Sessions must survive a disconnect: do NOT use gwae (no daemon by design; use tmux, or nest tmux inside a gwae pane).
- Which agent needs me: panes report status (» working, ! wants input, ✗ failed); hold ⌥ for the dashboard, ⌥+g jumps to the neediest pane.
- Never launch gwae itself from an agent turn: it takes over the terminal. Tell the user to run it.

keybinds (all ⌥ chords; Shift variants noted):
- focus: ⌥+h/j/k/l (left/down/up/right); move pane: ⌥+Shift+h/j/k/l
- new column: ⌥+Enter; new row: ⌥+Shift+Enter; split below: ⌥+b
- new agent: ⌥+; (remembers pick); pick agent: ⌥+Shift+;
- cycle width: ⌥+r; full width: ⌥+f; kill pane: ⌥+q; force quit: ⌥+Shift+q
- spawn dir: ⌥+d; smart jump: ⌥+g; view left/right: ⌥+[/]; scrollback: ⌥+up/down
- toggle help: ⌥+/; keep awake (macOS): ⌥+w; click focuses a pane; Cmd+V pastes

commands (safe to run headless): gwae doctor (health, exit 0 when ok), gwae --version, gwae agent --print (what ⌥+; would spawn), gwae setup --only latency (input-latency report)
config: ~/.config/gwae/gwae.toml, all keys optional, live-reloads; verify with gwae doctor
```

## thanks

if gwae fits your workflow, [give it a star](https://github.com/hongnoul/gwae). found a rough edge? [report it](https://github.com/hongnoul/gwae/issues/new/choose) with your OS version and terminal so we can improve the next person's first run.

## agent instructions

if you are an ai agent driving gwae, read [`docs/agents.md`](docs/agents.md) before spawning panes.

## development

```bash
git clone https://github.com/hongnoul/gwae
cd gwae
cargo build
cargo test --workspace
```

## license

gwae is licensed under the [MIT](LICENSE) license.
