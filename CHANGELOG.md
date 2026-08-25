# Changelog

All notable changes to this project will be documented in this file (keep-a-
changelog, updated per PR). strimux is pre-1.0; the format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- **M0 renderer**: single-process, multi-pane PTY cell renderer. The `strimux`
  binary spawns real panes, composes them into one 2D cell buffer, diffs and
  paints frames, and streams pane output live. Full 300x80 repaint measured at
  ~0.05 ms.
- **Content-width / horizontal-overflow scroll**: a pane's logical grid width
  is decoupled from its visible column width (`config.content_width`, default
  240). `Alt+Left/Right` pans across overflowing content.
- Interactive keybindings: focus (`Alt+hjkl`), move (`Alt+Shift+hjkl`), new
  column (`Alt+a`/`Alt+Enter`), split below (`Alt+s`), cycle width (`Alt+r/z`),
  kill pane (`Alt+x`), row viewport scroll (`Alt+[/]`), column jump (`Alt+1..9`),
  quit (`Alt+q`).
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
