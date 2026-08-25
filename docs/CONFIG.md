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
theme = "catppuccin-mocha"   # preset: catppuccin-mocha (default), catppuccin-latte, tokyo-night, gruvbox, nord, rose-pine, dracula, terminal
# or per-key overrides on top of a preset:
# [theme]
# preset = "nord"
# accent = "#ff0000"
# overlay = "#665c54"
skeleton = true
# legacy aliases (override theme.* when set):
# background = "#1e1e2e"     # -> theme.base
# focus_color = "#74c7ec"    # -> theme.accent
# skeleton_color = "#6c7086" # -> theme.overlay
mouse = true
scroll_lines = 3
input_poll_ms = 2

[minimap]
show = true
mode = "off"
max_width = 32
max_rows = 6
show_counts = true

[cowsay]
enabled = true
# messages = ["your own message", "another one"]
```

## Live reload

Saving the config file re-themes the **running** session: there is no restart,
so every pane and every agent keeps going. The file is polled for changes a
few times a second, and a one-line toast confirms the reload along the bottom
of the screen.

Only appearance is adopted. `startup_panes`, `mouse` and `default_agent` are
consumed once at launch (the panes already exist, mouse capture is already
negotiated with the host terminal, and running harnesses cannot be swapped
underneath), so changing them still needs a restart. Everything read every
frame - colors, `skeleton`, `[minimap]`, scroll behavior - takes effect
immediately.

A config that fails to parse mid-edit (an editor saving between keystrokes)
leaves the running settings alone and reports the error, rather than dropping
you back to defaults.

## Previewing themes (`⌥+t`)

`⌥+t` opens the theme picker. `←`/`→` (or `h`/`l`) step through the built-in
presets and each one is applied to the **live UI**, so the preview is the real
thing rather than a swatch. `⏎` keeps the previewed theme for this session and
shows the line to add to your config; `esc` restores whatever your config says.

The picker never writes to your config file - it would have to own your
formatting and comments to do that - so making a theme permanent is a
copy-paste of the line it shows you.

## Checking your config

A config file that fails to parse is **ignored entirely** (strimux falls back to
defaults rather than refusing to launch), and an unknown `theme` name silently
falls back to `catppuccin-mocha`. Both are easy to miss, so `doctor` reports
them:

```sh
strimux doctor
```

```
strimux doctor:
  config: /home/you/.config/strimux/strimux.toml
  config file: parses [ok]
  theme: nord [ok]
  layout smoke: columns 4 -> 5 on default row [ok]
```

A typo'd theme name is called out along with the valid names:

```
  theme: UNKNOWN "tokyonight-storm" -> falling back to catppuccin-mocha
    available: catppuccin-mocha, catppuccin-latte, tokyo-night, gruvbox, nord, rose-pine, dracula, terminal
```

and a config file that is not being applied at all points at the syntax error:

```
  config file: INVALID, so it is being ignored entirely: TOML parse error at line 2, column 6
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `{ preset = "quarter" }` | Width of newly created columns. Widths are `{ preset = "quarter" \| "third" \| "half" \| "two_thirds" \| "three_quarters" \| "full" }` or `{ cells = N }`. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `0` | Logical grid content width (cells) of every pane, decoupled from the visible column width. Long lines up to this width do not wrap and can be revealed with horizontal pane scroll (`⌥+Left/Right`, the Option key on macOS). `0` (the default) follows the visible column width so lines wrap normally and there is no horizontal overflow to manage in a pane. |
| `default_agent` | string | `"jcode"` | The agent harness command that `;` (spawn-agent) launches. If the command's executable is not found on `PATH` (e.g. jcode is not installed), the pane falls back to a plain `$SHELL` and a toast reports the missing agent. |
| `startup_panes` | integer | `1` | Number of equal-width quarter panes on screen at first launch. Each pane keeps a fixed `1/4` share of the viewport regardless of this count, so a value below `4` leaves the right side of the screen empty (shown as skeleton placeholder boxes, or covered by `background` with `skeleton = false`). The default `1` opens a single terminal in the leftmost quarter. |
| `theme` | string or table | `catppuccin-mocha` | Chrome color theme. A bare preset name (`theme = "tokyo-night"`) or a `[theme]` table with `preset` plus per-key overrides (`base`, `surface`, `overlay`, `accent`, `text`, `label`, `running`, `idle`, `done`, `failed`). Presets: `catppuccin-mocha` (default), `catppuccin-latte`, `tokyo-night`, `gruvbox`, `nord`, `rose-pine`, `dracula`, `terminal` (inherits the host terminal's ANSI 0-15 palette; single-word aliases `mocha`, `latte`, `tokyo`, `gruvbox`, `nord`, `dracula`, `ansi` also accepted). Colors accept a 256-color index (`235`), hex RGB (`"#1e1e2e"`), or `"default"`. |
| `[theme].preset` | string | `catppuccin-mocha` | Which built-in palette to start from (see `theme`). Unknown names fall back to `catppuccin-mocha` with a warning. |
| `[theme].base` | color | preset | Empty (uncovered) background behind the panes. |
| `[theme].surface` | color | preset | Background of the HUD and centered minimap panels. |
| `[theme].overlay` | color | preset | Skeleton frames around unfocused boxes. |
| `[theme].accent` | color | preset | Accent frame around the focused box. |
| `[theme].text` | color | preset | HUD and minimap text. |
| `[theme].label` | color | preset | Big block-font `strip.cell` label in placeholder boxes. |
| `[theme].running` | color | preset | Pane status tint: running. |
| `[theme].idle` | color | preset | Pane status tint: idle / wants attention. |
| `[theme].done` | color | preset | Pane status tint: succeeded. |
| `[theme].failed` | color | preset | Pane status tint: failed. |
| `background` | color | preset `base` | **Legacy alias for `theme.base`**. When set it overrides the resolved theme's `base`, so existing configs with `background = "#1e1e2e"` keep behaving as before. New configs should use `theme` / `[theme]`. |
| `focus_color` | color | preset `accent` | **Legacy alias for `theme.accent`**. Overrides the theme's `accent`; use `[theme] accent = ...` for new configs. |
| `skeleton_color` | color | preset `overlay` | **Legacy alias for `theme.overlay`**. Overrides the theme's `overlay`; use `[theme] overlay = ...` for new configs. |
| `skeleton` | bool | `true` | Draw the skeleton: a 1-cell frame around every column box at full strip height, so the four-column container always reads. Pane content is inset 1 cell inside its frame, so the frame never covers anything a program draws. With fewer columns than fit, placeholder quarter-width boxes tile the empty right side; their interiors use the default (pane) background rather than `background`, so empty grids are not dimmed, and each shows a big block-font `strip.cell` identifier centered in the box. The focused box's frame uses `focus_color` instead of `skeleton_color`. |
| `mouse` | bool | `true` | Capture the mouse so the wheel scrolls *inside* the pane under the cursor (its own scrollback) instead of reaching the host terminal, where it walks the host's scrollback and the shell's previous/next prompt history. A pane running a full-screen app that asked for mouse reporting gets the event forwarded verbatim, translated into its own grid coordinates; one on the alternate screen without mouse reporting (e.g. `less`) gets arrow keys. Typing snaps a scrolled-back pane to the live bottom. Set to `false` to hand the wheel back to the host terminal. |
| `scroll_lines` | integer | `3` | Rows of pane scrollback moved per wheel notch. |
| `input_poll_ms` | integer | `2` | Milliseconds to wait in `event::poll` before checking PTY output and repainting. Lower values reduce perceived typing and backspace latency at the cost of more frequent wakeups. Default `2` (down from `10`) is low latency with modest CPU cost. Valid range `1..50`. Set to `1` for minimum possible input latency. |
| `hud_opacity` | float | `1.0` | Opacity of the centered chrome panels (the `⌥+/` cheat-sheet HUD, the centered minimap, the theme picker), from `0.0` (invisible) to `1.0` (solid). Terminals have no alpha channel, so strimux composites the panel over whatever the panes painted underneath and emits the mixed truecolor result; both the panel fill and its text fade, so `0.75` reads as a wash over the pane rather than a solid slab. Requires a truecolor terminal to look right. |
| `minimap.show` | bool | `true` | Draw the minimap dashboard in the bottom-right corner. It appears once there is more than one pane (or more than one strip). Rows of the map are strips; each tile is a pane, its width proportional to the column's real width share. Tiles are tinted by status - blue `»` working, amber `!` wants attention, green `✓` done, red `✗` failed (non-zero exit) - the focused pane's tile uses `focus_color`, the focused strip gets a `❯` gutter chevron, and each tile's first cell shows its column digit (the same digit `⌥+1..9` jumps to). Status comes from OSC 133 shell integration when the pane emits it, else from an output-activity heuristic (silent for a few seconds → wants attention). |
| `minimap.mode` | string | `"off"` | Chrome presentation: `off` (no persistent row; `⌥`/Alt reveals centered HUD + minimap), `overlay` (bottom-right corner), `edge_ticks` (frame ticks). Legacy `reserved` / `reserved_quasimode` parse as `off` (no bottom row). |
| `minimap.max_width` | integer | `32` | Maximum width of the minimap. Used for `overlay` and the centered minimap while holding `⌥`/Alt. |
| `minimap.max_rows` | integer | `6` | Maximum number of strips (map rows) shown. Used for `overlay` and the centered minimap while holding `⌥`/Alt. |
| `minimap.show_counts` | bool | `true` | Summary tallies, e.g. `5 »2 !1 ✓1 ✗1` (zero counts skipped), above the map. |
| `cowsay.enabled` | bool | `true` | Draw a small cowsay under the block-font identifier in empty placeholder boxes, so an empty grid documents itself. The cow is skipped when the box is too small for it to fit whole (under 23 cells wide, or too short for label + art), so the identifier is never crowded out. |
| `cowsay.messages` | array of strings | keybinding hints (OS-aware: `⌥+g` on macOS, `Alt+g` elsewhere) | The pool each empty box draws its line from. Which box says what is chosen by hashing the cell's position, never randomly, so a given box always says the same thing and idle strimux does not repaint. An empty list disables the cow just like `enabled = false`. |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
