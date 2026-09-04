# Roadmap

Milestones are demo-driven: each ends in something you can show as a gif.

## M0 - Skeleton and spike — DONE (shipped in v1.0.0)
Kill the biggest risk first: prove the render path.
- [x] Cargo workspace scaffold, CI green on macOS + Linux
- [x] Spike: one process, two PTY panes, custom cell-buffer renderer
- [x] Horizontal scroll with full repaints, measure frame time at 300x80
- [x] Prototype the emulator crate behind `TermGrid`; decide ADR-004
- **Exit**: vim usable in a cropped pane while scrolling; repaint < 4ms

## M1 - The 2D strip grid — DONE (shipped in v1.0.0)
- [x] Layout core (done in scaffold: rows/columns/panes + verbs + scroll)
- [x] First row as a niri strip: new-column, split, focus+follow, cycle-width, consume/expel
- [x] Infinite rows; status bar with a 2D minimap
- [x] `$mod` = `⌥+hjkl` (Option key on macOS, Alt elsewhere) focus / `⌥+Shift+hjkl` move
- **Exit**: dogfood one daily agent session in a row

## M2 - Agent-first ergonomics — DONE (shipped in v1.0.0) <- was make-or-break
- [x] Agent launcher + `⌥+a` / `;` default-harness spawn
- [x] **OSC 133 status**: minimap dots + status tick
- [x] **Smart-jump** `⌥+g`
- [ ] Fuzzy text summon (**key TBD** — `⌥+f` is toggle-full-width) — deferred to M5
- [x] Pane headers with resume hints; TOML config; layout property tests
- **Exit**: dogfood daily Claude Code + Jcode sessions — ongoing, this is the launch gate

## M3 - Comfortable daily driver (in progress)
- [x] Scrollback (`⌥+↑/↓`, Ctrl+Shift+J/K, wheel), drag-select copy, kitty graphics passthrough
- [ ] Scrollback search
- [ ] `⌥+y` yank (pane/turn, text or image) — spec below
- [ ] Scroll animation
- [ ] Windows ConPTY runtime verification (builds today, unverified at runtime)

## M4 - Polish and launch — DONE (v1.0.0/v1.0.1)
Themes/truecolor, VHS gifs, docs, Homebrew tap + AUR + nix + winget, prebuilt
binaries, `gwae setup`. **Exit**: v1.0.1 installable in one command (install.sh, Homebrew tap, AUR, crates.io; winget pending).

### Staying current — DONE (`gwae upgrade`, ADR-016)

The half of distribution that launch skipped: getting the *next* release onto
the machines that already have gwae. `gwae upgrade` detects how a binary was
installed (installer receipt, then path), runs the routes gwae owns
(install.sh / brew / cargo) and only prints the command for the ones another
package manager owns (nix, AUR, distro, checkout). A once-a-day background
check surfaces a one-line notice naming the exact command. Spec:
[`UPDATES.md`](UPDATES.md).

## M5 - Post-launch differentiation (ongoing)
Overview zoom, per-command pane rules, deeper PTY-compliant agent integration,
community presets.

### Yank: pane/turn capture to clipboard — superseded by `⌥+c`/`⌥+v`

This landed as a copy/paste *pair* on the keys every platform already uses,
not as a lone `⌥+y`. `⌥+y` survives as an alias so the muscle memory this
entry assumed still works. Shipped: bracketed paste correctness, `⌥+v`
(with a large-paste confirmation), and `⌥+c` at selection or visible-pane
scope. Still open: turn scope (needs OSC 133 prompt *positions*, not just
status), whole-scrollback scope (needs a row-range accessor in `gwae-term`),
and the image variants.

See [docs/COPY-PASTE.md](COPY-PASTE.md) for the design, what shipped, and
the remaining enabling work in dependency order.


## M6 - Stability to 1.0
Layout spec frozen, fuzz the emulator, 1.0 after 6 months of dogfooding and no
data-loss bugs for 3 months.
