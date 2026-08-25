# How strimux compares

Honest table, so you can decide fast whether this is for you.

| Project | Layout | Runs inside a terminal? | Detach/persistence | Platforms |
|---|---|---|---|---|
| tmux | plane tiling (every pane on screen) | Yes | Yes (session/server) | macOS, Linux, *BSD |
| Zellij | plane tiling + floating, KDL layouts | Yes | Yes | macOS, Linux, Windows |
| Séance | niri strip (GUI) | No | socket control | Linux (GTK) |
| tairi | niri strip (GUI) | No | workspaces | macOS |
| panescale / mission-control | free canvas (GUI) | No | - | macOS |
| **strimux** | **2D niri strip grid** | **Yes** | **No (harness `--resume`)** | **macOS, Windows, Linux** |

## The key differences

- **No-shrink.** tmux/Zellij cram panes into a fixed screen; strimux scrolls an
  infinite strip, so panes keep full, natural size and agents stay readable.
- **Niri feel in a plain terminal.** Séance/tairi need a GUI or a compositor;
  strimux runs in any terminal you already use, on any OS.
- **No daemon.** If you want SSH session persistence that outlives the process,
  keep tmux. strimux deliberately delegates persistence to each agent harness
  (`claude --resume` / `jcode --resume`).

## Who it's for

CLI-agent users running many concurrent sessions (Claude Code, Jcode, yazi, nvim)
who want the niri layout and keyboard-first tiling inside their terminal, without
switching compositors or giving up their OS.

Want detach/SSH persistence? Use tmux - or run strimux and let your agent's
`--resume` carry the session.
