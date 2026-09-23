# When to use gwae

- **Running several CLI coding agents at once** (Claude Code, Codex, Jcode): each agent keeps a full-width pane, the HUD shows who is running, idle, or failed, and ⌥+g jumps to whichever agent needs you.
- **tmux panes keep shrinking into slivers**: gwae never shrinks a pane. The strip scrolls sideways infinitely instead, like niri does for desktop windows.
- **Babysitting long jobs**: hold ⌥ for a live minimap of every pane's state, and ⌥+w keeps the Mac awake until the job lands.
- **You like niri and want that layout in the terminal on macOS.**

Not for you if you need remote detach/reattach sessions over SSH: that is tmux's home turf, and gwae is local-first.

## gwae vs tmux vs herdr

| | gwae | tmux | herdr |
|---|---|---|---|
| Layout | scrolling strip, panes never shrink | fixed grid, panes split and shrink | workspaces and tabs |
| Agent state at a glance | HUD over OSC 133 (running / idle / failed) | none built in | built-in state detection |
| Jump to agent needing input | ⌥+g, failures ranked first | manual | supported |
| Detach / reattach over SSH | no | yes, its core strength | yes |
| Spawn p50 (measured) | 0.48–0.52 ms | 4.1–4.7 ms | 0.63–0.70 ms |
| Platform | macOS (Windows/Linux contributions welcome) | everywhere | macOS, Linux |

tmux remains the right tool for persistent remote sessions. herdr is a solid alternative if you want workspaces and tabs over an infinite strip. Benchmark methodology is in the [README](../README.md#benchmark).
