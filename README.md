# gwae


<p align="center">
  <img src="docs/assets/gwae.svg" alt="gwae" width="100" />
</p>

<p align="center"><strong>Lightweight, Lightspeed</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-white?labelColor=black" alt="MIT license" /></a>
  <a href="https://github.com/hongnoul/gwae/releases"><img src="https://img.shields.io/github/downloads/hongnoul/gwae/total?labelColor=black&color=white" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/hongnoul/gwae/stargazers"><img src="https://img.shields.io/github/stars/hongnoul/gwae?labelColor=black&color=white&logo=github&logoColor=black" alt="GitHub stars" /></a>
  <a href="https://github.com/hongnoul/gwae/releases/latest"><img src="https://img.shields.io/github/v/release/hongnoul/gwae?label=release&labelColor=black&color=white" alt="latest stable release" /></a>

  ![gwae demo: agents on an infinite no-shrink strip grid](docs/assets/gwae-demo.gif)
</p>

<p align="center">Tested with <a href="https://github.com/ghostty-org/ghostty">Ghostty</a> and <a href="https://github.com/1jehuang/jcode">Jcode</a> on MacBook Pro M4</p>



## Install

> Developed and tested on macOS only. Contributions for Windows are welcome.
>
> Linux users: I recommend [niri](https://github.com/YaLTeR/niri), the scrolling window manager that inspired gwae.

```bash
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

or with Homebrew:

```bash
brew install hongnoul/tap/gwae
```

## Cool features

> Official docs are still WIP.

| Feature | Default | What it does |
| ------- | --- | ------------ |
| Heads-up&nbsp;display | hold ⌥ | Live agent status minimap over standard OSC&nbsp;133 with Running, Idle, and Failed states |
| Smart&nbsp;jump | ⌥+g | Cycles through Idle panes with Failed states ranked first |
| Hatchery | ⌥+d | Opens the spawn-directory picker to choose where the next agent process starts |
| Agent&nbsp;selector | ⌥+Shift+; | Opens the harness picker to spawn a specific agent instead of your default choice |
| Drag&nbsp;to&nbsp;copy | drag | Left-drag selects and copies on release via pbcopy with OSC 52 fallback for remote sessions |
| Caffeinate | ⌥+w | Toggles the macOS caffeinate assertion so a long-running job never sleeps mid-task |

## Interactive tutorial

<kbd>⌥</kbd> + <kbd>/</kbd> opens the cheat sheet.

Vim-style movement, like a tiling WM: <kbd>h</kbd> left · <kbd>j</kbd> down · <kbd>k</kbd> up · <kbd>l</kbd> right

Paste this into an agent pane:

```text
You are teaching me gwae, a scrolling terminal multiplexer. I am inside a
gwae pane talking to you. I am a beginner: assume I know nothing beyond
typing in a terminal. Walk me through each step below one at a time. For
each step, tell me exactly what to press, what I should see, and how to
confirm it worked. Wait for my confirmation before moving on. If something
does not work, stop and help me fix it before continuing. Do not skip
ahead and do not dump all steps at once.

0. Fix the modifier key first. Nothing below works without this. Have me
press ⌥+/ . If the cheat sheet opens, it works; press Esc to close it.
If a special character types instead, tell me to set Option to act as
Meta in my terminal (Terminal.app: Preferences, Profiles, Keyboard, Use
Option as Meta Key; iTerm2, Ghostty, WezTerm have the equivalent), then
retry. Do not proceed until ⌥+/ opens the cheat sheet.
1. Open a pane: have me press ⌥+Enter once. Confirm a new shell pane
appears to the right.
2. Move: have me press ⌥+h and ⌥+l to move between the two panes.
Confirm the focus ring follows. Then have me press ⌥+k and ⌥+j.
Mention click-to-focus only after these work.
3. Open more: have me press ⌥+Enter until there are 6 panes. If I get
lost, remind me ⌥+h/l moves between panes. Point out the viewport
scrolling right while every pane keeps its width. Then have me press
⌥+[ and ⌥+] to scroll the viewport left and right without moving focus.
Confirm the strip slides but focus stays put.
4. Reshape: have me press ⌥+r to cycle the focused column's width, then
⌥+b to split below. Mention ⌥+f
(full width) and ⌥+Shift+h/j/k/l (move pane in all four directions)
only after these work.
5. Agents: have me press ⌥+; to spawn an agent (first press offers what
is installed and remembers it). Confirm Cmd+V pastes text into the pane
(native terminal paste). Mention ⌥+Shift+; (picker), ⌥+d (spawn
directory), and gwae run "claude" (CLI start) after.
6. History: have me press ⌥+↑ and ⌥+↓ to scroll the focused pane's
history (add Shift or use PgUp/PgDn for a bigger jump). Then have me
press ⌃+⇧+K and ⌃+⇧+J for the three-line transcript scroll, noting it
goes to the agent itself inside agent panes. Then have me press ⌥+←
and ⌥+→ to pan content sideways. Confirm each moves content without
moving focus.
7. Stuck panes: have me hold ⌥ for the dashboard (» working, ! needs
input, ✗ failed), then ⌥+g to jump to the neediest pane. Mention
clicking a tile focuses it.
8. Help and quit: show ⌥+/ (cheat sheet), ⌥+q (close pane),
⌥+Shift+q (quit; last pane exiting quits gwae). Mention ⌥+w
(keep-awake) last.
9. Health: only now, have me type exit in panes until one shell is left,
then run gwae doctor in it and confirm exit 0. If anything fails, run
gwae init. Mention ~/.config/gwae/gwae.toml (all keys optional,
live-reloads) only here, at the end.
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

## Agent instructions

if you are an ai agent driving gwae, read [`docs/agents.md`](docs/agents.md) before spawning panes.
