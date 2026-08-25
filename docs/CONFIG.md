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
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `{ preset = "half" }` | Width of newly created columns. Widths are `{ preset = "quarter" \| "third" \| "half" \| "two_thirds" \| "three_quarters" \| "full" }` or `{ cells = N }`. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `0` | Logical grid content width (cells) of every pane, decoupled from the visible column width. Long lines up to this width do not wrap and can be revealed with horizontal pane scroll (`⌥+Left/Right`, the Option key on macOS). `0` (the default) follows the visible column width so lines wrap normally and there is no horizontal overflow to manage in a pane. |
| `default_agent` | string | `"jcode"` | The agent harness command that `;` (spawn-agent) launches. |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
