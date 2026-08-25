# Configuration

Written for you by `strimux init` (the guided first-run setup, also offered
once by the agent gateway). Setup asks about the keys most people want to
change; the rest of this file is the hand-edit surface, and nothing here needs
to go through setup. Everything below can equally be hand-edited, and
strimux live-reloads appearance keys while it runs. `strimux init` only
rewrites the keys you answer and preserves your comments.

Location: `$XDG_CONFIG_HOME/strimux/strimux.toml` (default
`~/.config/strimux/strimux.toml`). TOML (ADR-008). All keys optional; missing
keys fall back to the defaults below.

Example:

```toml
default_column_width = "half"     # or "quarter", "two-thirds", "full", or 80 (cells)
scroll_margin = 2
center_focus = false
content_width = 0
default_agent = "claude"           # first pane + ; launch this
agents = ["my-agent-wrapper"]     # extra names for the selector
theme = "catppuccin-mocha"   # preset: catppuccin-mocha (default), catppuccin-latte, tokyo-night, gruvbox, nord, rose-pine, dracula, terminal
# or per-key overrides on top of a preset:
# [theme]
# preset = "nord"
# accent = "#ff0000"
# overlay = "#665c54"
skeleton = false             # true draws the 1-cell inset column frames
# legacy aliases (override theme.* when set):
# background = "#1e1e2e"     # -> theme.base
# focus_color = "#74c7ec"    # -> theme.accent
# skeleton_color = "#6c7086" # -> theme.overlay
input_poll_ms = 2            # applied silently by setup; 1 is the recommended value

[minimap]
show = true
mode = "off"
max_width = 32
max_rows = 6
show_counts = true

cell_labels = false          # true brings back the big `strip.pane` labels

[cowsay]
enabled = false              # true draws the hint cow in empty boxes
# messages = ["your own message", "another one"]
```

## What `strimux init` asks, and what it does not

Setup is one question per screen: `↑↓`/`jk` picks an option, `→`/`l`/`⏎` moves
on, `←`/`h`/`⌫` goes back, a digit selects without Enter, `s` skips a question
and `esc` takes the defaults for the rest. It ends on a summary screen listing
every setting and the file it landed in, where only `⏎` (leave) and `⌫` (back
to the last question) do anything. It asks about `theme`,
`startup_panes`, `default_column_width`, `center_focus`, `content_width`,
`cell_labels` and `cowsay.enabled`, then offers to install `btm`.

Everything else here is hand-edit only, deliberately:

* `input_poll_ms` has exactly one right answer, so setup **applies it
  silently** before the first question rather than asking. Settings that only
  you can change (kitty, macOS) are reported once on the summary screen.
* `skeleton`, `[minimap]` geometry and `scroll_margin` are niche tastes; a
  setup flow long enough to cover them is one nobody finishes.
* `btm` is not a config key at all - it is an action on the machine, so it is
  never written to this file. On macOS a yes installs Homebrew first if it is
  missing. Set `STRIMUX_NO_INSTALL=1` to turn the offer off entirely.

## Live reload

Saving the config file re-themes the **running** session: there is no restart,
so every pane and every agent keeps going. The file is polled for changes a
few times a second, and a one-line toast confirms the reload along the bottom
of the screen.

Only appearance is adopted. `startup_panes` is consumed once at launch (the
panes already exist), so changing it still needs a restart. `default_agent` is
read fresh by the agent gateway each time `;` opens a pane, so editing it (or
letting the gateway save your pick) applies to the *next* agent pane without a
restart; panes already running a harness keep running it. Everything read every
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
  agent: claude [ok]
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
| `default_column_width` | width | `"quarter"` | Width of newly created columns. A preset name (`"quarter"`, `"third"`, `"half"`, `"two-thirds"`, `"three-quarters"`, `"full"`; separators and case are ignored, and `"1/2"` style also works), a bare integer for fixed cells (`80`), or the table forms `{ preset = "half" }` / `{ cells = 80 }`. |
| `onboarded` | bool | unset | Written by `strimux init` to record that the guided setup has run, so the agent gateway offers it exactly once. Delete it to be offered again. |
| `scroll_margin` | integer | `2` | Cells of context kept visible around the focused column when scrolling. |
| `center_focus` | bool | `false` | Always center the focused column (niri's centered mode) instead of scrolling minimally. |
| `content_width` | integer | `0` | Logical grid content width (cells) of every pane, decoupled from the visible column width. Long lines up to this width do not wrap and can be revealed with horizontal pane scroll (`⌥+Left/Right`, the Option key on macOS). `0` (the default) follows the visible column width so lines wrap normally and there is no horizontal overflow to manage in a pane. |
| `default_agent` | string | `""` (unset) | The agent harness `;` launches, and what the **first pane** opens on at startup. When unset, or not on `PATH`, you get the **agent selector** instead: it lists harnesses it knows, anything agent-shaped found on your `PATH`, and anything in `agents`; pick one (or type any command) and it is saved here, so every later launch goes straight to it. With nothing found it opens a plain `$SHELL`. `strimux run <cmd>` overrides the first pane. See `strimux agent --print`. |
| `agents` | array of strings | `[]` | Extra agent commands to offer in the selector, for a harness whose name strimux cannot guess (or a wrapper script of your own). Entries that are not installed are simply not listed. |
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
| `skeleton` | bool | `false` | **Off by default, and never asked about by `strimux init`**: the inset frames are a taste, so they are opt-in from this file alone. When `true`: draw the skeleton: a 1-cell frame around every column box at full strip height, so the four-column container always reads. Pane content is inset 1 cell inside its frame, so the frame never covers anything a program draws. With fewer columns than fit, placeholder quarter-width boxes tile the empty right side; their interiors use the default (pane) background rather than `background`, so empty grids are not dimmed, and each shows a big block-font `strip.cell` identifier centered in the box. The focused box's frame uses `focus_color` instead of `skeleton_color`. |
| `input_poll_ms` | integer | `2` | Set to `1` **silently by `strimux init`**, before the first question: it has exactly one right answer, so it is not worth a question. Milliseconds the event loop waits for a keystroke before checking PTY output and repainting. strimux sits on the keystroke round trip twice (your key in, the program's echo out), so this costs roughly double. `1` is the recommended value; run `strimux tune` to check this and the macOS/terminal settings around it. Valid range 1..50. See `docs/LATENCY.md`. |
| `minimap.show` | bool | `true` | Draw the minimap dashboard in the bottom-right corner. It appears once there is more than one pane (or more than one strip). Rows of the map are strips; each tile is a pane, its width proportional to the column's real width share. Tiles are tinted by status - blue `»` working, amber `!` wants attention, green `✓` done, red `✗` failed (non-zero exit) - the focused pane's tile uses `focus_color`, the focused strip gets a `❯` gutter chevron, and each tile's first cell shows its column digit (the same digit `⌥+1..9` jumps to). Status comes from OSC 133 shell integration when the pane emits it, else from an output-activity heuristic (silent for a few seconds → wants attention). |
| `minimap.mode` | string | `"off"` | Chrome presentation: `off` (no persistent row; `⌥`/Alt reveals centered HUD + minimap), `overlay` (bottom-right corner), `edge_ticks` (frame ticks). Legacy `reserved` / `reserved_quasimode` parse as `off` (no bottom row). |
| `minimap.max_width` | integer | `32` | Maximum width of the minimap. Used for `overlay` and the centered minimap while holding `⌥`/Alt. |
| `minimap.max_rows` | integer | `6` | Maximum number of strips (map rows) shown. Used for `overlay` and the centered minimap while holding `⌥`/Alt. |
| `minimap.show_counts` | bool | `true` | Summary tallies, e.g. `5 »2 !1 ✓1 ✗1` (zero counts skipped), above the map. |
| `cowsay.enabled` | bool | `false` | Draw a small cowsay under the block-font identifier in empty placeholder boxes, so an empty grid documents itself. Off by default: empty boxes ship as a bare skeleton. The cow is skipped when the box is too small for it to fit whole (under 23 cells wide, or too short for label + art), so the identifier is never crowded out. |
| `cell_labels` | bool | `false` | Draw the big block-font `strip.pane` identifier in empty placeholder boxes. Off by default; set `true` to bring the address labels back. |
| `cowsay.messages` | array of strings | keybinding hints (OS-aware: `⌥+g` on macOS, `Alt+g` elsewhere) | The pool each empty box draws its line from. Which box says what is chosen by hashing the cell's position, never randomly, so a given box always says the same thing and idle strimux does not repaint. An empty list disables the cow just like `enabled = false`. |

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
