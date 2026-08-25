# Changelog

All notable changes to this project will be documented in this file (keep-a-
changelog, updated per PR). strimux is pre-1.0; the format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Cargo workspace scaffold with four crates:
  `strimux` (bin), `strimux-layout`, `strimux-term`, `strimux-testkit`.
- `strimux-layout`: the pure 2D grid-of-strips core (rows/columns/panes,
  follow-focus scroll, verbs) with `proptest` invariant properties.
- `strimux-term`: the `TermGrid` emulator facade (ADR-004) + `NullGrid`.
- `strimux-testkit`: `FakeTerminal` for scripted/rendered-frame tests.
- `strimux` bin: `clap` CLI (`run`/`new`/`setup`/`doctor`) + TOML config loader.
- MIT license, docs (ARCHITECTURE / LAYOUT-SPEC / COMPARISON / CONFIG / ROADMAP),
  CI workflow, packaging scaffolding, scripts, issue templates.
