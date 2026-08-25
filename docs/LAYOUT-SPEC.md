# Layout Specification (normative)

The spec for scrollable tiling in cell space, adapted from niri's semantics to a
character grid. When in doubt, this file wins.

## Coordinate system

An **infinite 2D grid of strips** (ADR-013). Every pane lives at a `(row, x)`
coordinate.

- **Row**: one horizontal strip (height = terminal viewport minus chrome width).
  Rows stack infinitely **downward** and are user-named; they never auto-relocate.
- **Viewport**: the window `[scroll_x, scroll_x + view_cols)` onto the focused row.
- Exactly one pane is focused.

## Columns

- A column occupies a contiguous x-range with a **width**: a preset fraction of
  the viewport (`1/4, 1/3, 1/2, 2/3, 3/4, 1`) or fixed cells.
- Default new-column width: `1/2` (config `default_column_width`).
- A column contains 1..n panes stacked vertically, splitting strip height evenly.
- Gap between columns: 1 cell (themed divider).

## Invariants

1. **No implicit resize**: adding/removing/moving columns never changes any other
   column's width.
2. Pane logical width = column width; logical height = its share of strip height.
3. Terminal window resize changes the focused row's height (unavoidable).
4. Order of columns on a row is total; no gaps (compaction on close).
5. Rows never auto-relocate or reorder.

## Verbs (default keys)

`$mod` = `Alt`; `$mod` chords always go to strimux, even with an agent focused.

| Verb | Key | Semantics |
|---|---|---|
| focus left/right | `Alt+h/l` | move focus to adjacent column; scroll minimally |
| focus up/down | `Alt+k/j` | move within column; cross row at edge |
| move-pane left/right | `Alt+Shift+h/l` | swap column with neighbor |
| move-pane up/down | `Alt+Shift+k/j` | move pane within column; cross row at edge |
| new-column | `Alt+Enter` | new column right of focused (launcher) |
| new-agent | `Alt+a` | new column running the default agent harness |
| split-down | `Alt+s` | new pane below focused |
| new-row | `Alt+Shift+Enter` | new row below focused |
| cycle-width | `Alt+r` | next preset width |
| consume / expel | `Alt+,` / `Alt+.` | stack a neighbor / push pane out |
| center | `Alt+z` | center focused column |
| jump N | `Alt+1..9` | jump to Nth column |
| smart-jump | `Alt+g` | jump to next agent that needs you (OSC 133) |
| find | `Alt+f` | fuzzy text search over panes |
| kill-pane | `Alt+x` | close pane; compact columns/rows |
| scroll viewport | `Alt+Ctrl+h/l` | scroll the row without moving focus |

## Scroll behavior

- **Follow-focus**: after a focus change, scroll the minimum so the focused
  column is fully visible, honoring `scroll_margin` (default 2).
- **Free scroll** does not move focus; first pane-bound keystroke snaps back per
  `snap_back`.
- Optional `center_focus` centers the column always.

## Minimap

Status bar renders a compact 2D map, one line per nearby row, with the viewport
bracket, row labels, and per-pane agent-status dots (OSC 133).

## Non-goals (v1)

Daemon/detach/attach, free 2D canvas, floating panes, semantic agent panes,
mouse-driven chrome, overview zoom-out, images/sixel chrome, plugin system.
