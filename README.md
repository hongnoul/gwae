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

everything lives in [docs/](docs/): [why](docs/WHY.md) · [agents](docs/agents.md) · [configuration](docs/CONFIG.md) · [keybinds](docs/KEYBINDS.md) · [architecture](docs/ARCHITECTURE.md) · [layout](docs/LAYOUT-SPEC.md) · [latency](docs/LATENCY.md) · [comparison](docs/COMPARISON.md) · [copy-paste](docs/COPY-PASTE.md) · [spawn dir](docs/SPAWN-DIR.md) · [terminal compatibility](docs/TERMINAL-COMPATIBILITY.md) · [updates](docs/UPDATES.md)

teach yourself gwae end to end: paste this into an empty pane and follow it.

```text
gwae e2e: panes never shrink.

1. press ⌥+Enter three times. three shell panes, same width.
2. press ⌥+Enter three more times. the viewport scrolls right;
   the first panes keep their width instead of squeezing.
3. press ⌥+h twice. focus moves left, the viewport follows.
4. press ⌥+r. the focused column cycles its width.
5. press ⌥+b. a pane splits below the focused one.
6. hold ⌥. the dashboard shows every pane with its status.
7. press ⌥+/ for the cheat sheet, Esc to close it.
8. type exit in each shell, or press ⌥+q to close the focused pane.
   the last pane exiting quits gwae.

next: gwae init sets up your agent, gwae doctor checks config.
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
