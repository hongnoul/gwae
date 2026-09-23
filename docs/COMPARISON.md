# How gwae compares

Honest table, so you can decide fast whether this is for you.

| Project | Layout | Runs inside a terminal? | Detach/persistence | Platforms |
|---|---|---|---|---|
| tmux | plane tiling (every pane on screen) | Yes | Yes (session/server) | macOS, Linux, *BSD |
| Zellij | plane tiling + floating, KDL layouts | Yes | Yes | macOS, Linux, Windows |
| Séance | niri strip (GUI) | No | socket control | Linux (GTK) |
| tairi | niri strip (GUI) | No | workspaces | macOS |
| panescale / mission-control | free canvas (GUI) | No | - | macOS |
| **gwae** | **2D niri strip grid** | **Yes** | **No (harness `--resume`)** | **macOS, Linux, Windows** |

## The key differences

- **No-shrink.** tmux/Zellij cram panes into a fixed screen; gwae scrolls an
  infinite strip, so panes keep full, natural size and agents stay readable.
- **Niri feel in a plain terminal.** Séance/tairi need a GUI or a compositor;
  gwae runs in any terminal you already use (Windows Terminal, kitty,
  Ghostty, WezTerm).
- **No daemon.** If you want SSH session persistence that outlives the process,
  keep tmux. gwae deliberately delegates persistence to each agent harness
  (`claude --resume` / `jcode --resume`).

## Who it's for

CLI-agent users running many concurrent sessions (Claude Code, Jcode, yazi, nvim)
who want the niri layout and keyboard-first tiling inside their terminal, without
switching compositors or giving up their OS.

Want detach/SSH persistence? Use tmux - or run gwae and let your agent's
`--resume` carry the session.

## Windows notes

Windows builds on the same single-process core: PTYs come from ConPTY via
`portable-pty`, input and rendering go through crossterm, and config, layout,
and the update receipt work the same. Three things differ:

- **Default shell is PowerShell.** Unix panes inherit `$SHELL` (else `sh`);
  on Windows `$SHELL` is normally unset and `sh` is not on PATH, so fresh
  panes and the agent fallback run `powershell.exe`.
- **The `$mod` glyph is `⌥`.** Key labels use the macOS glyphs everywhere
  (`⌥+g`, `⌥+↵`); on Windows that is the Alt key.
- **Hot reload and the focus daemon are unix-only.** `reload.rs` stubs them
  out with an explicit error, and e2e suites that drive real PTYs with unix
  shells are gated off (CI runs unit tests only on Windows).
