# Roadmap

Milestones are demo-driven: each ends in something you can show as a gif.

## M0 - Skeleton and spike (1-2 weeks)
Kill the biggest risk first: prove the render path.
- [x] Cargo workspace scaffold, CI green on macOS + Linux
- [ ] Spike: one process, two PTY panes, custom cell-buffer renderer
- [ ] Horizontal scroll with full repaints, measure frame time at 300x80
- [ ] Prototype the emulator crate behind `TermGrid`; decide ADR-004
- **Exit**: vim usable in a cropped pane while scrolling; repaint < 4ms

## M1 - The 2D strip grid (2-3 weeks)
- [ ] Layout core (done in scaffold: rows/columns/panes + verbs + scroll)
- [ ] First row as a niri strip: new-column, split, focus+follow, cycle-width, consume/expel
- [ ] Infinite rows; status bar with a 2D minimap
- [ ] `$mod` = `⌥+hjkl` (Option key on macOS, Alt elsewhere) focus / `⌥+Shift+hjkl` move
- **Exit**: dogfood one daily agent session in a row

## M2 - Agent-first ergonomics (3-4 weeks) <- make-or-break
- [ ] Agent launcher + `⌥+a` / `;` default-harness spawn
- [ ] **OSC 133 status**: minimap dots + status tick
- [ ] **Smart-jump** `⌥+g`; fuzzy text summon (**key TBD** — `⌥+f` is toggle-full-width)
- [ ] Pane headers with resume hints; TOML config; layout property tests
- **Exit**: dogfood daily Claude Code + Jcode sessions

## M3 - Comfortable daily driver (3-4 weeks)
Copy mode, scrollback search, bracketed paste/focus/bell passthrough, scroll
animation, Windows ConPTY verification.

## M4 - Polish and launch (2-3 weeks)
Themes/truecolor, VHS gifs, docs, Homebrew tap + AUR + nix + winget, prebuilt
binaries, `strimux setup`. **Exit**: v0.1.0 installable in one command.

## M5 - Post-launch differentiation (ongoing)
Overview zoom, per-command pane rules, deeper PTY-compliant agent integration,
community presets.

## M6 - Stability to 1.0
Layout spec frozen, fuzz the emulator, 1.0 after 6 months of dogfooding and no
data-loss bugs for 3 months.
