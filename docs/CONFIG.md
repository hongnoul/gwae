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
background = "#1e1e2e"
focus_color = "#ff0000"
skeleton = true
skeleton_color = "#ffffff"
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `{ preset = "quarter" }` | Width of newly created columns. Widths are `{ preset = "quarter" \| "third" \| "half" \| "two_thirds" \| "three_quarters" \| "full" }` or `{ cells = N }`. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `0` | Logical grid content width (cells) of every pane, decoupled from the visible column width. Long lines up to this width do not wrap and can be revealed with horizontal pane scroll (`⌥+Left/Right`, the Option key on macOS). `0` (the default) follows the visible column width so lines wrap normally and there is no horizontal overflow to manage in a pane. |
| `default_agent` | string | `"jcode"` | The agent harness command that `;` (spawn-agent) launches. |
| `startup_panes` | integer | `4` | Number of equal-width quarter panes on screen at first launch. Each pane keeps a fixed `1/4` share of the viewport regardless of this count, so a value below `4` leaves the right side of the screen empty (covered by `background`). `2` gives two quarter panes with the right half empty. |
| `background` | color | `default` | Color of the empty (uncovered) background behind the panes. Accepted forms: a 256-color index (`235`), a hex RGB string (`"#1e1e2e"`), or the literal `"default"` (the terminal's own background, usually black). Pane content always paints over it. |
| `focus_color` | color | `#ff0000` | Color of the 1-cell accent frame drawn around the focused box (an overlay on the pane's edge cells; it never shifts or resizes the pane). Accepted forms match `background`. Set to `default` to draw with the terminal's own background. |
| `skeleton` | bool | `true` | Draw the skeleton: a 1-cell frame around every column box at full strip height, so the four-column container always reads. With fewer columns than fit, placeholder quarter-width boxes tile the empty right side. The focused box's frame uses `focus_color` instead of `skeleton_color`. |
| `skeleton_color` | color | `#ffffff` | Color of the skeleton frames around unfocused boxes. Accepted forms match `background`. |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
