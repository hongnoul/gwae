# Configuration

Location: `$XDG_CONFIG_HOME/strimux/strimux.toml` (default
`~/.config/strimux/strimux.toml`). TOML (ADR-008). All keys optional; missing
keys fall back to the defaults below.

Example:

```toml
default_column_width = { preset = "half" }   # or { cells = 80 }
scroll_margin = 2
center_focus = false
content_width = 0
default_agent = "claude"
background = "#1e1e2e"        # Catppuccin Mocha base
focus_color = "#74c7ec"       # Catppuccin Mocha sapphire — focus ring vs red failed
skeleton = true
skeleton_color = "#6c7086"    # Catppuccin Mocha overlay0
mouse = true
scroll_lines = 3
input_poll_ms = 2

[minimap]
show = true
mode = "reserved_quasimode"
max_width = 32
max_rows = 6
show_counts = true
hud_on_attention_ms = 2500
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `{ preset = "quarter" }` | Width of newly created columns. Widths are `{ preset = "quarter" \| "third" \| "half" \| "two_thirds" \| "three_quarters" \| "full" }` or `{ cells = N }`. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `0` | Logical grid content width (cells) of every pane, decoupled from the visible column width. Long lines up to this width do not wrap and can be revealed with horizontal pane scroll (`⌥+Left/Right`, the Option key on macOS). `0` (the default) follows the visible column width so lines wrap normally and there is no horizontal overflow to manage in a pane. |
| `default_agent` | string | `"jcode"` | The agent harness command that `;` (spawn-agent) launches. |
| `startup_panes` | integer | `1` | Number of equal-width quarter panes on screen at first launch. Each pane keeps a fixed `1/4` share of the viewport regardless of this count, so a value below `4` leaves the right side of the screen empty (shown as skeleton placeholder boxes, or covered by `background` with `skeleton = false`). The default `1` opens a single terminal in the leftmost quarter. |
| `background` | color | `#1e1e2e` | Color of the empty (uncovered) background behind the panes. Accepted forms: a 256-color index (`235`), a hex RGB string (`"#1e1e2e"`), or the literal `"default"` (the terminal's own background, usually black). Pane content always paints over it. Default is Catppuccin Mocha base. |
| `focus_color` | color | `#74c7ec` | Color of the 1-cell accent frame drawn around the focused box (an overlay on the pane's edge cells; it never shifts or resizes the pane). Accepted forms match `background`. Set to `default` to draw with the terminal's own background. Default is Catppuccin Mocha sapphire (cyan, distinct from the red `Failed` state). |
| `skeleton` | bool | `true` | Draw the skeleton: a 1-cell frame around every column box at full strip height, so the four-column container always reads. Pane content is inset 1 cell inside its frame, so the frame never covers anything a program draws. With fewer columns than fit, placeholder quarter-width boxes tile the empty right side; their interiors use the default (pane) background rather than `background`, so empty grids are not dimmed, and each shows a big block-font `strip.cell` identifier centered in the box. The focused box's frame uses `focus_color` instead of `skeleton_color`. |
| `skeleton_color` | color | `#6c7086` | Color of the skeleton frames around unfocused boxes. Accepted forms match `background`. Default is Catppuccin Mocha overlay0. |
| `mouse` | bool | `true` | Capture the mouse so the wheel scrolls *inside* the pane under the cursor (its own scrollback) instead of reaching the host terminal, where it walks the host's scrollback and the shell's previous/next prompt history. A pane running a full-screen app that asked for mouse reporting gets the event forwarded verbatim, translated into its own grid coordinates; one on the alternate screen without mouse reporting (e.g. `less`) gets arrow keys. Typing snaps a scrolled-back pane to the live bottom. Set to `false` to hand the wheel back to the host terminal. |
| `scroll_lines` | integer | `3` | Rows of pane scrollback moved per wheel notch. |
| `input_poll_ms` | integer | `2` | Milliseconds to wait in `event::poll` before checking PTY output and repainting. Lower values reduce perceived typing and backspace latency at the cost of more frequent wakeups. Default `2` (down from `10`) is low latency with modest CPU cost. Valid range `1..50`. Set to `1` for minimum possible input latency. |
| `minimap.show` | bool | `true` | Draw the minimap dashboard in the bottom-right corner. It appears once there is more than one pane (or more than one strip). Rows of the map are strips; each tile is a pane, its width proportional to the column's real width share. Tiles are tinted by status - blue `»` working, amber `!` wants attention, green `✓` done, red `✗` failed (non-zero exit) - the focused pane's tile uses `focus_color`, the focused strip gets a `❯` gutter chevron, and each tile's first cell shows its column digit (the same digit `⌥+1..9` jumps to). Status comes from OSC 133 shell integration when the pane emits it, else from an output-activity heuristic (silent for a few seconds → wants attention). |
| `minimap.mode` | string | `"reserved_quasimode"` | Presentation: `overlay` (legacy corner), `reserved` (1-row chrome), `reserved_quasimode` (hold ⌥/Alt to reveal the 1-row chrome or when attention Idle/Failed; while held also paints a centered minimap overlay sized by `max_width`/`max_rows`), `edge_ticks` (frame ticks), `off` (no chrome). In quasimode the 1-row chrome is still reserved so geometry never churns (no SIGWINCH) — it just stays blank at rest. |
| `minimap.max_width` | integer | `32` | Maximum width (in cells) of the minimap block; it shrinks to fit narrower terminals. Used for `overlay` and the centered minimap while holding ⌥/Alt in `reserved_quasimode`. |
| `minimap.max_rows` | integer | `6` | Maximum number of strips (map rows) shown; extra strips are cut off. Used for `overlay` and the centered minimap while holding ⌥/Alt in `reserved_quasimode`. |
| `minimap.show_counts` | bool | `true` | Draw the one-line summary above the map (overlay) or on the right of the reserved row: total pane count plus per-status tallies, e.g. `5 »2 !1 ✓1 ✗1` (zero counts skipped). |
| `minimap.hud_on_attention_ms` | integer | `2500` | Non-zero enables a centered HUD box (attention hint `» 1.3 needs you — ⌥+g` + keybind cheat-sheet) at startup and when attention (Idle/Failed) arises while `reserved_quasimode` is hidden and Alt is not held. The HUD persists until the next key press (any key); `0` disables. The value is a backward-compatible enable flag (any non-zero enables). |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
