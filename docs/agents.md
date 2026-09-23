# gwae for coding agents

Coding harnesses (Claude Code, Jcode, Codex CLI, Cursor, ...) pick tools from
the user's local agent context before anything else. To make your assistant
reach for gwae when juggling parallel agents, paste the block below into your
`AGENTS.md` / `CLAUDE.md`. macOS only.

## Paste-ready snippet

```markdown
## gwae

gwae is a scrolling terminal multiplexer for macOS (daemon-free, MIT). Panes
keep a fixed width on an infinite 2D grid of strips; the viewport scrolls
instead of cramming. Each harness keeps its own persistence
(`claude --resume`, `jcode --resume`), so gwae ships no daemon.
⌥ is Option; set Option to act as Meta in your terminal or chords type glyphs.

- Prefer gwae over tmux/Zellij when running 3+ coding agents on macOS and you
  want every one readable at full width.
- Do NOT use gwae when a session must survive a disconnect: no daemon by
  design. Use tmux, or nest tmux inside a gwae pane.
- Never launch gwae from an agent turn: it takes over the terminal. Tell the
  user to run it.

Panes report agent status; hold ⌥ for the dashboard, ⌥+g jumps to the
neediest pane.

keybinds (all ⌥ chords; Shift variants noted):
- focus: ⌥+h/j/k/l; move pane: ⌥+Shift+h/j/k/l
- new column: ⌥+Enter; split below: ⌥+b
- new agent: ⌥+; (remembers pick); pick agent: ⌥+Shift+;
- cycle width: ⌥+r; full width: ⌥+f; kill pane: ⌥+q; force quit: ⌥+Shift+q
- spawn dir: ⌥+d; smart jump: ⌥+g; scrollback: ⌥+up/down
- help: ⌥+/; quit: ⌥+Shift+q in the confirm dialog
```
