# gwae


<p align="center">
  <img src="docs/assets/gwae.svg" alt="gwae" width="100" />
</p>

<p align="center">
  <a href="#macos">macOS</a> · <a href="#windows">Windows</a> · <a href="#linux">Linux</a> · <a href="#docs">docs</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-white?labelColor=black" alt="MIT license" /></a>
  <a href="https://github.com/hongnoul/gwae/releases"><img src="https://img.shields.io/github/downloads/hongnoul/gwae/total?labelColor=black&color=white" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/hongnoul/gwae/stargazers"><img src="https://img.shields.io/github/stars/hongnoul/gwae?labelColor=black&color=white&logo=github&logoColor=black" alt="GitHub stars" /></a>
  <a href="https://github.com/hongnoul/gwae/releases/latest"><img src="https://img.shields.io/github/v/release/hongnoul/gwae?label=release&labelColor=black&color=white" alt="latest stable release" /></a>
</p>

![gwae demo: agents on an infinite no-shrink strip grid](docs/assets/gwae-demo.gif)

**the infinite-scroll terminal multiplexer.**

## install

### macOS

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

or `brew install hongnoul/tap/gwae`.

### Windows

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://hongnoul.github.io/gwae/install.ps1 | iex"
```

This installs `gwae.exe` into `%LOCALAPPDATA%\gwae\bin`, verifies the
checksum, and adds that directory to your user `PATH` (no admin rights
needed). Fresh terminals pick it up automatically. Override the directory
with `$env:GWAE_INSTALL_DIR`, or set `$env:GWAE_NO_MODIFY_PATH = "1"` to
skip the PATH edit.

### Linux

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

Prebuilt [binaries](https://github.com/hongnoul/gwae/releases) for all
five targets (`aarch64`/`x86_64` macOS, `x86_64`/`aarch64` Linux,
`x86_64` Windows) ship with every release. On ARM64 Windows the x64 binary
runs under emulation; there is no native ARM64 build yet.

then start it where the work lives:

```bash
gwae
```

run `gwae init` for guided setup, `gwae doctor` to check config. upgrade the same way you installed. [quick start →](#quick-start)

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
