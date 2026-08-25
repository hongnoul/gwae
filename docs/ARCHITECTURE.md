# Architecture

strimux is a **single-process, daemon-free** multiplexer (ADR-003 reversed,
ADR-011). One `strimux` process owns every PTY, hosts every pane's grid, and
composes the whole screen into a single 2D cell buffer. There is no client-
server protocol, no socket, no attach/detach.

```
strimux (one process)
├── Layout core (rows / strips / columns / panes) - pure, no I/O
├── Pane tasks (one per PTY: read bytes -> parse -> update grid)
├── Composer (coalesce damage -> single 2D cell buffer)
├── Render (diff buffer -> batched ANSI -> terminal)
├── Input (raw mode: decode keys, forward to focused pane or handle $mod)
└── OSC 133 trackers (per-pane status: running/idle/done -> minimap + smart-jump)
```

## Crate map

| Crate | Kind | Responsibility |
|---|---|---|
| `strimux` | bin | the whole TUI: raw mode, input decode, PTY hosting, composer, render loop, launcher, minimap, OSC 133 |
| `strimux-layout` | lib | **the pure core**. 2D grid of strips (rows/columns/panes) + verbs + scroll math. No I/O, no async, no PTY. Property-tested. |
| `strimux-term` | lib | emulator facade behind a `TermGrid` trait (ADR-004), damage tracking |
| `strimux-testkit` | lib | fake PTYs, scripted terminals, snapshot harness |
| `strimux-hmr` | bin | hot-reload **host**: owns the session (raw mode, input, frame buffer, state) and `dlopen`s the core, swapping it on rebuild |
| `strimux-core` | cdylib | hot-swappable **core** (`handle_key` / `render`) loaded by the host; rebuilt and swapped at runtime |
| `strimux-core-api` | lib | the stable HMR boundary both host and core compile against; never recompiled on a reload |

`strimux-layout` depends only on `std` + `serde`. `strimux-term` isolates the
emulator-crate choice behind `TermGrid`; swapping `alacritty_terminal` <-> `wezterm-term`
touches one crate.

## Rendering pipeline

One 2D cell buffer holds everything: every visible pane's grid region plus
strimux's own chrome (minimap, status bar, launcher, focus outline). Damage from
any pane merges into this one buffer; the render diffs the whole buffer to the
terminal with synchronized-update markers (`ESC[?2026h/l`).

Horizontal scroll of a strip is a **full repaint** (terminals can't blit
horizontally), so frames must be fast: budget < 4ms for a 300x80 viewport.
Scroll animation is optional and gated on sync-update support and frame budget.

## Pane sizing

Panes have a **logical size** (cols x rows) set by their column width and strip
height. This is the size reported to the PTY (`TIOCSWINSZ`), so full-screen apps
lay out at logical size. The viewport crops, never resizes: a column wider than
the viewport is panned, and app inside is unaffected.

## Crash / persistence (deliberately thin)

When the process exits, panes and their processes end; strimux persists **no
session state** (ADR-015). Resume is the harness's job (`claude --resume`,
`jcode --resume`). A panic in one pane's emulator must not take the TUI down
(one task per pane, supervisor pattern).

## Terminal requirements

Minimum 256-color + standard cursor addressing; wants truecolor, synchronized
updates, kitty keyboard protocol, mouse SGR (passed through). `⌥` (the Option
key on macOS, Alt elsewhere) is the universal `$mod` (ADR-014); macOS may add an
optional `Cmd+hjkl` snippet via `strimux setup`.

## Open questions

- ADR-004: which emulator crate (`alacritty_terminal` vs `wezterm-term`) wins
  the `TermGrid` trait. Decide in the M0 spike by prototyping both.
