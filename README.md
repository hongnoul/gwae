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

**the infinite-scroll terminal multiplexer.**

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

Prebuilt [binaries](https://github.com/hongnoul/gwae/releases) for all
five targets (`aarch64`/`x86_64` macOS, `x86_64`/`aarch64` Linux,
`x86_64` Windows) ship with every release. On ARM64 Windows the x64 binary
runs under emulation; there is no native ARM64 build yet.

then start it where the work lives:

```bash
gwae
```

run `gwae init` for guided setup, `gwae doctor` to check config. upgrade the same way you installed.

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
