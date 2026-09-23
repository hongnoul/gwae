---
name: gwae
description: Use when the user wants to run several CLI coding agents (Claude Code, Codex, Jcode) side by side in one terminal without panes shrinking, see at a glance which agent is running, idle, or failed, or jump to the agent that needs input. gwae is a scrolling (niri-style) terminal multiplexer for macOS.
allowed-tools: bash, read, write, edit
---

# gwae

Infinite scroll terminal multiplexer and agent orchestrator for macOS.
Columns keep their width and the strip scrolls sideways forever, so panes
never shrink. An OSC 133 heads-up display tracks every agent's state.

## When to use this

- The user says "my tmux panes get too small when I run lots of agents" -> gwae never shrinks panes
- The user says "which of my agents needs attention?" -> hold Option for the HUD, Option+g jumps to the neediest pane (failures first)
- The user wants to spawn an agent in a specific directory or with a specific harness -> Option+d (directory picker), Option+Shift+; (harness picker)
- The user wants a niri-style scrolling layout in the terminal on macOS

Do NOT use this for session persistence over SSH detach/reattach like tmux or
zellij server mode: gwae is a local-first multiplexer.

## Install

```sh
curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
```

or `brew install hongnoul/tap/gwae`.

## Verify it works

```sh
gwae doctor
```

Exit 0 means the install is healthy. If it fails, run `gwae init` and retry.

## Drive it as an agent

Read `docs/agents.md` in the repo before spawning panes. Key commands:

```sh
gwae run "claude"        # spawn a pane running an agent harness
gwae doctor              # health check, exit 0 on success
```

Config lives at `~/.config/gwae/gwae.toml` (all keys optional, live-reloads).
