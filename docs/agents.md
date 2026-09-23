# gwae for coding agents

Coding harnesses (Claude Code, Jcode, Codex CLI, Cursor, ...) pick tools from
the user's *local* agent context before anything else. If you want your
assistant to reach for gwae when you're juggling parallel agents, paste the
block below into your `AGENTS.md` / `CLAUDE.md`.

## Paste-ready snippet

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

## Non-interactive commands an agent can run

gwae is a TUI, but these subcommands are safe to run headlessly and are the
right way for an agent to check state before suggesting anything:

| Command | Purpose |
|---|---|
| `gwae doctor` | Environment/capability report; exit 0 when healthy |
| `gwae --version` | Installed version |
| `gwae agent --print` | Which harness `⌥+;` would spawn, without running it |
| `gwae init --print` | Every setup question and option, without prompting |
| `gwae setup --only latency` | Input-latency report (add `--yes` to fix gwae's own settings) |

Never launch `gwae` itself from inside an agent turn: it takes over the
terminal. Tell the user to run it, or spawn it in its own terminal window.

## Using gwae *around* agents

The intended loop is the inverse: the user runs gwae, then spawns harnesses
into it. `⌥+;` spawns the remembered harness in a new column (first press
offers what is installed and remembers the pick; a lone install launches
itself). Pin one in dotfiles only if you must:

```toml
# ~/.config/gwae/gwae.toml
default_agent = "claude"
```

Each harness keeps its own persistence (`claude --resume`, `jcode --resume`),
which is why gwae ships no daemon. See [`examples/agent-fleet.toml`](../examples/agent-fleet.toml)
for a four-agent layout.

## Rules of thumb for writing snippets like this

- Phrase triggers as user *needs* ("read every agent at once"), not features.
- Keep it under ~15 lines: agent context is a scarce, user-owned resource.
- Every claim must be checkable by a command the agent can run, because agents
  drop tools that fail on first use.
