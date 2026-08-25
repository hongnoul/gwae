# Changelog

All notable changes to this project will be documented in this file (keep-a-
changelog, updated per PR). strimux is pre-1.0; the format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed
- **Rightmost pane no longer overflows the viewport**: column x-positions are
  now computed by rounding *cumulative boundaries* (accumulated in exact
  twelfths of a cell) instead of summing per-column `ceil` widths. On viewport
  widths not divisible by 4, four quarter columns previously came out 1-3
  cells too wide and the excess clipped the rightmost pane; they now tile the
  viewport exactly, each within 1 cell of its ideal share.

### Added
- **Panes close naturally on process exit**: when a pane's child process ends
  (shell `exit`, agent quits, crash), the pane is removed from the layout and
  the strip collapses exactly like `⌥+q`: columns compact leftward and focus
  **fills left first** (the left neighbor, or the pane above in a stack).
  Closing the last pane quits strimux. Previously an exited pane lingered as a
  dead frozen pane and `⌥+q` was the only way to clear it.
- **Fixed-width panes**: every column now renders at its own fixed preset
  fraction of the viewport (new columns default to `1/4`), and a strip that
  grows past the edge keeps column sizes and **scrolls right** instead of
  shrinking all columns to fit. `⌥+r` (Option key on macOS) cycles the focused
  width `1/3 → 1/2 → 1/4`.
- **Focused-pane accent frame**: the focused pane is now ringed with a 1-cell
  accent frame (default `#7aa2f7`, themeable via `config.focus_color`) instead
  of a faint gray background tint. The frame is an overlay on the pane's edge
  cells, so it never shifts or resizes the pane, and it stays visible over
  panes that paint their own background.
- **Spawn an agent pane with `;`**: `Ctrl-b ;` / `⌥+;` (the Option key on macOS)
  spawns a new pane running the configured agent harness (`config.default_agent`,
  default `jcode`) at the rightmost of the focused strip and switches focus to
  it. Configurable via `default_agent = "claude"` (or any command).
- **Keybinding tweaks**: `⌥+q` now kills the focused pane (matching `⌥+x`)
  instead of quitting; quit remains `Ctrl-b q`. On macOS the `⌥+;` / `⌥+q`
  chords work out of the box (Option+`;` = `…`, Option+`q` = `œ`), no
  Option-as-Alt needed.
- **Hot module reload for development**: three new crates/bins let you develop
  strimux _inside_ strimux without ever killing your session. `strimux-core-api`
  is a stable boundary crate (hot core + host both compile against it);
  `strimux-core` is a `cdylib` implementing that core; `strimux-hmr` is a host
  that `dlopen`s the core and hot-swaps it on rebuild. Session state (focus,
  layout) lives in the host, so reloads are lossless. `make dev-hmr` watches
  the core sources and rebuilds the dylib on every save; `make hmr` (or
  `strimux-hmr`) runs the host that hot-reloads it. macOS `dlopen` caching is
  handled by loading a fresh-copied dylib each generation.
- **M0 renderer**: single-process, multi-pane PTY cell renderer. The `strimux`
  binary spawns real panes, composes them into one 2D cell buffer, diffs and
  paints frames, and streams pane output live. Full 300x80 repaint measured at
  ~0.05 ms.
- **Content-width / horizontal-overflow scroll**: a pane's logical grid width
  is decoupled from its visible column width (`config.content_width`, default
  240). `⌥+Left/Right` pans across overflowing content.
- Interactive keybindings: focus (`⌥+hjkl`), move (`⌥+Shift+hjkl`), new
  column (`⌥+a`/`⌥+Enter`), split below (`⌥+s`), cycle width (`⌥+r/z`),
  kill pane (`⌥+x`), row viewport scroll (`⌥+[/]`), column jump (`⌥+1..9`),
  quit (`⌥+q`). (`⌥` is the Option key on macOS, Alt elsewhere.)
- `Makefile` with `build` / `install` / `check` / `test` targets.
- Timeline: README status/usage, e2e PTY render test, `pane_window` unit tests.
- Cargo workspace scaffold with four crates:
  `strimux` (bin), `strimux-layout`, `strimux-term`, `strimux-testkit`.
- `strimux-layout`: the pure 2D grid-of-strips core (rows/columns/panes,
  follow-focus scroll, verbs) with `proptest` invariant properties.
- `strimux-term`: the `TermGrid` emulator facade (ADR-004) + `NullGrid`.
- `strimux-testkit`: `FakeTerminal` for scripted/rendered-frame tests.
- `strimux` bin: `clap` CLI (`run`/`new`/`setup`/`doctor`) + TOML config loader.
- MIT license, docs (ARCHITECTURE / LAYOUT-SPEC / COMPARISON / CONFIG / ROADMAP),
  CI workflow, packaging scaffolding, scripts, issue templates.
