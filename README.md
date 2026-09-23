# gwae

  [website](https://hongnoul.github.io/gwae/) · [install](#install) · [quick start](#quick-start) · [docs](#docs)

---

![gwae demo: agents on an infinite no-shrink strip grid](docs/assets/gwae-demo.gif)

**the infinite-scroll terminal multiplexer for macOS.**

- **panes never shrink** — open more columns and the viewport scrolls instead of squeezing them, like niri. widths stay fixed and scroll snaps to column boundaries, no slivers. [layout spec →](docs/LAYOUT-SPEC.md)
- **never hunt for the stuck one** — every pane is marked `»` working, `!` needs input, `✗` failed. hold `⌥` for the dashboard, `⌥+g` jumps to the pane that needs you. [agents →](docs/agents.md)

  ![gwae attention demo: hold Option for the dashboard, tap Option-G to jump to the pane that needs you](docs/assets/gwae-attention.gif)

  [watch the attention demo as MP4](docs/assets/gwae-attention.mp4)

- **runs what you already run** — claude code, codex, shells, TUIs. `⌥+;` spawns your agent and remembers your pick; `gwae run` starts it from the CLI. [why gwae →](docs/WHY.md)
- **keyboard and mouse, both first-class** — option-key chords *and* click, drag-to-copy, wheel scroll. [keybinds →](docs/KEYBINDS.md)
- **macOS native** — `pbcopy` clipboard, `caffeinate` keep-awake, CoreGraphics option-key poll, focus fixes. [comparison →](docs/COMPARISON.md)
- **one rust binary** — runs in whatever terminal you already use. [architecture →](docs/ARCHITECTURE.md)

---

## install

macOS only. Homebrew is the primary install:

```bash
brew install hongnoul/tap/gwae
```

or the curl installer, which lands in `~/.local/bin` and wires up your shell PATH:

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

then start it where the work lives:

```bash
gwae
```

run `gwae init` for guided setup and `gwae doctor` to check config. upgrade the same way you installed (`brew upgrade gwae`, or re-run the installer). if `command -v gwae` points somewhere other than Homebrew after installing, an older copy earlier on PATH is shadowing it. [quick start →](#quick-start)

## quick start

on first launch, dismiss the help overlay with Escape. no agent account needed: skip the agent choice for a shell-only test.

press `⌥+Enter` a few times to add shell panes. keep going past the screen edge: the viewport scrolls, the panes keep their width. use `⌥+h` / `⌥+l` to move left / right, `⌥+r` to cycle the focused column's width, `⌥+/` for help. type `exit` in each shell (or `⌥+q` to close the focused pane) when you're done.

if `⌥` types special characters instead of triggering shortcuts, set Option to act as Meta in your terminal (Terminal.app: Preferences → Profiles → Keyboard → Use Option as Meta Key; iTerm2 / Ghostty / WezTerm have the equivalent).

bring your agents:

```bash
gwae init             # guided setup, safe to re-run
gwae run "claude"     # start with Claude Code in the first pane
gwae run "codex"      # or Codex CLI
gwae doctor           # check config and setup
```

config lives at `~/.config/gwae/gwae.toml` (`$XDG_CONFIG_HOME/gwae/gwae.toml`), all keys optional. full key reference: press `⌥+/` in gwae. [configuration →](docs/CONFIG.md)

## docs

everything lives in [docs/](docs/): [why](docs/WHY.md) · [agents](docs/agents.md) · [configuration](docs/CONFIG.md) · [keybinds](docs/KEYBINDS.md) · [architecture](docs/ARCHITECTURE.md) · [layout](docs/LAYOUT-SPEC.md) · [latency](docs/LATENCY.md) · [comparison](docs/COMPARISON.md) · [copy-paste](docs/COPY-PASTE.md) · [spawn dir](docs/SPAWN-DIR.md) · [terminal compatibility](docs/TERMINAL-COMPATIBILITY.md) · [updates](docs/UPDATES.md)

## thanks

if gwae fits your workflow, [give it a star](https://github.com/hongnoul/gwae). found a rough edge? [report it](https://github.com/hongnoul/gwae/issues/new/choose) with your macOS version and terminal so we can improve the next person's first run.

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
