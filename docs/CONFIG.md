# Configuration

Location: `$XDG_CONFIG_HOME/strimux/strimux.toml` (default
`~/.config/strimux/strimux.toml`). TOML (ADR-008). All keys optional; missing
keys fall back to the defaults below.

Example:

```toml
default_column_width = { preset = "half" }   # or { cells = 80 }
scroll_margin = 2
center_focus = false
content_width = 240
default_agent = "claude"
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `{ preset = "half" }` | Width of newly created columns. Widths are `{ preset = "quarter" \| "third" \| "half" \| "two_thirds" \| "three_quarters" \| "full" }` or `{ cells = N }`. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `240` | Logical grid content width (cells) of every pane, decoupled from the visible column width. `0` follows the visible column width (no overflow reveal). |
| `default_agent` | string | `"claude"` | The harness `Alt+a` spawns. |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
