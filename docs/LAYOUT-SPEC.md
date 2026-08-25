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
- Default new-column width: `1/4` (config `default_column_width`).
- A column contains 1..n panes stacked vertically, splitting strip height evenly.
- Gap between columns: 1 cell (themed divider).

## Invariants

1. **No implicit resize**: adding/removing/moving columns never changes any other
   column's width.
2. Each preset column renders at its own fixed fraction of the viewport, so a
   strip that grows past the screen keeps each column the same size and **scrolls
   right** (revealed by follow-focus) instead of shrinking every column.
3. Pane logical width = column width; logical height = its share of strip height.
4. Terminal window resize changes the focused row's height (unavoidable).
5. Order of columns on a row is total; no gaps (compaction on close). A pane
   whose process exits closes the same way as kill-pane: the column collapses
   and focus fills **left first** (left neighbor, or the pane above in a
   stack). A strip whose last pane closes is discarded and focus shifts to the
   strip **above** (or to the one below when it was the first strip), so the
   focus never rests on an empty strip. The last pane exiting quits strimux.
6. Rows never auto-relocate or reorder.

## Verbs (default keys)

`$mod` = `⌥` (the **Option** key on macOS, Alt elsewhere); `$mod` chords always
go to strimux, even with an agent focused.

| Verb | Key | Semantics |
|---|---|---|
| focus left/right | `⌥+h/l` | move focus to adjacent column; scroll minimally |
| focus up/down | `⌥+k/j` | move within column; cross strip at edge. Past the last strip a new empty strip is created (niri workspace semantics); an empty strip you leave is discarded, and you cannot create another while standing on an empty one |
| move-pane left/right | `⌥+Shift+h/l` | swap column with neighbor |
| move-pane up/down | `⌥+Shift+k/j` | move pane within column; at the stack edge the pane moves to the neighboring strip (creating one past the end), and an emptied strip is discarded |
| new-column | `⌥+Enter` | new column right of focused (launcher) |
| new-agent | `⌥+a` / `;` | new column running the default agent harness |
| split-down | `⌥+s` | new pane below focused |
| new-row | `⌥+Shift+Enter` | new row below focused |
| cycle-width | `⌥+r` | cycle focused preset width 1/3 → 1/2 → 1/4 |
| toggle-full-width | `⌥+f` | toggle focused column between full width and 1/4 |
| consume / expel | `⌥+,` / `⌥+.` | stack a neighbor / push pane out |
| center | `⌥+z` | center focused column |
| jump N | `⌥+1..9` | jump to Nth column |
| smart-jump | `⌥+g` | jump to next agent that needs you (OSC 133) |
| find | *(unassigned)* | fuzzy text search over panes. **Not implemented; `⌥+f` is taken by toggle-full-width** (see `binds.rs`, which is verified against the dispatcher). A key must be chosen before this ships |
| kill-pane | `⌥+x` | close pane; compact columns/rows; an emptied strip is dropped and focus shifts up |
| scroll viewport | `⌥+Ctrl+h/l` | page the row one column stop without moving focus |

## Scroll behavior

Scrolling is **quantized to column boundaries**. `scroll_x` only ever rests on
a valid **stop**: some column's left boundary, or `max_scroll` (the end stop
that pins the last column to the right viewport edge). A column therefore
always starts flush at x=0, so identical grids paint identically in every
scroll state: no partial-column slivers, no margin drift.

- **Follow-focus**: after a focus change, move to the *nearest stop* that
  fully reveals the focused column (minimal movement, niri feel). A column
  wider than the viewport is shown from its left edge. `scroll_margin` is
  ignored under quantization (partial-column margins are exactly the slivers
  quantization removes).
- **Free scroll** pages to the previous/next stop and does not move focus;
  first pane-bound keystroke snaps back per `snap_back`.
- Optional `center_focus` picks the feasible stop nearest the centered
  position.
- On resize, every row's scroll re-snaps to the nearest stop at the new
  geometry.

## Minimap

Status bar renders a compact 2D map, one line per nearby row, with the viewport
bracket, row labels, and per-pane agent-status dots (OSC 133).

## Non-goals (v1)

Daemon/detach/attach, free 2D canvas, floating panes, semantic agent panes,
mouse-driven chrome, overview zoom-out, images/sixel chrome, plugin system.
