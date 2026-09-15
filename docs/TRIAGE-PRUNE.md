# gwae One-Time Triage Review (2026-09-15)

Baseline: ~29.2k lines in `crates/gwae/src`, 23 e2e files, 30 docs.
Rule used: the product is scrolling multiplexer + fixed-width panes + PTYs + Alt+hjkl + agent panes. Everything else had to earn its place. Verdicts are final unless you override.

## CUT now (dead, stub, or scope creep) — ~6k lines

| Feature | Size | Why it dies |
|---|---|---|
| `gwae new` subcommand | 5L stub (`main.rs:60` just logs) | Dead code. Never implemented. Delete subcommand. |
| `onboard/` (questions/screen/persist) | ~2kL | Duplicates `setup/`. Two onboarding systems; `setup` stage registry wins. `gwae init` becomes thin alias or dies. |
| `splash.rs` + splash_e2e | 369L + test | 1-second title card. Replace with 3-line println or nothing. |
| `preview/` (prefs/paint/encode) | 718L | Exists only to decorate onboarding questions. Dies with `onboard/`. |
| `cowsay.rs` + cowsay_e2e, `cell_labels` config | 325L + test | Joke filler for empty boxes. Bare borders are fine. |
| `install.rs` (btm installer) | 406L | A multiplexer installing a system monitor via brew is scope creep. Delete; document `brew install bottom` in one line. |
| Setup stub stages: behavior, bindings, keyboard, onboarding, layout-smoke | ~150L + `assets/terminal/` templates | Stubs and read-only reports with no apply path. `layout smoke` is a unit test living in prod. Delete all five. |
| `setup/focus.rs` (kitty Space daemon, 421L) + focus stage | 421L + Swift agent + plist | macOS + kitty + native-fullscreen-only fix shipped as a daemon with launchd label. Niche bug, huge surface. Extract to a doc snippet, delete the code. |
| `content_width` + Alt+Left/Right pane h-scroll | config + render paths | A second scroll axis nobody manages. Panes wrap; done. |
| Minimap + center HUD + toasts + edge ticks (`chrome.rs` bulk) + `minimap.*` config (5 keys) | ~1kL of 2406L | Dashboard chrome for a tool whose chrome should be borders + help. Keep help overlay + focus frame only. |
| Legacy color aliases `background`/`focus_color`/`skeleton_color` | config | Superseded by `theme` table. Delete. |
| `input_poll_ms` config key + `gwae tune` subcommand | key + `latency.rs` CLI half | Exactly one right answer (1ms). Hard-code it, keep `latency.rs` audit/report as `setup` stage + doctor line only. |
| `scroll_margin`, `center_focus` | 2 keys + viewport branches | One scroll behavior (minimal scroll, snap to column). Delete both keys. |
| `harness_dirs`, `agent_dirs`, `agent_dir_roots` | 3 keys + picker branches | `agent_dir` + `.git`-marker scan covers it. 4 keys collapse to 1. |
| pdf/pixel_size/yazi e2e (graphics) | ~70k of test code | Die with graphics (below). |

## CUT or feature-flag (the big call) — ~4k lines

**Kitty graphics (`graphics.rs` 1262 + `graphics_legacy` 1294 + host 762 + stream 579 + diacritics 37):** partial support is a liability — 12% of the repo, 3 e2e files, "partial" in the README. Either `CUT entirely` (my recommendation: no image path until someone owns it full-time) or `--features graphics` off by default. No partial-by-default.

**Updater auto-apply (`update/` 1.3k):** keep `upgrade --check` (detect source, print command). CUT every path that executes brew/cargo/installer. A multiplexer must never self-modify through package managers it doesn't own.

## KEEP but simplify (shrink in place)

| Feature | Action |
|---|---|
| `agent/` gateway (1.3k) | KEEP. Merge to 2 files (`gateway.rs` + `detect.rs`). Picker is 402L for a list + exec — halve it. |
| `tui/input.rs` (1419) | KEEP. CUT Ctrl+Shift+J/K scrollback variants; one scrollback binding. Caps-Lock/kitty-glyph compat stays (correctness, not chrome). |
| `tui/render.rs` + `pty.rs` + `term.rs` + `diff.rs` + `mouse.rs` + `osc.rs` | KEEP. Core. `reap.rs` merges into `pty.rs` (one teardown owner). |
| `select/` (600L, 3 files) | One file, basic drag-select + copy. CUT paste-negotiation variants; keep one bracketed-paste path. |
| Theme system (`theme/` 669 + picker) | KEEP preset names only (3–5 built-ins). CUT `terminal`-derived theme + per-key override table. |
| `spawndir/` (~900) | KEEP `agent_dir` + `.git` scan + picker UI. CUT the rest (above). |
| `keepawake.rs` (304) | KEEP logic (sleep kills agents — real need). CUT red focus-ring UI to a plain doctor line; keep one bool. |
| `reload.rs` (523) | KEEP. Daemon-free hot reload is the dev-velocity story; tested. |
| `reload`/`doctor`/`setup` (kept stages: config_file, theme, harness, spawn_dir, latency, updates-check, keep-awake) | 7 stages, not 12. |
| `gwae-layout` (1.9k), `gwae-term` (1.6k), `keys.rs`, `binds.rs` | KEEP. The cleanest code in the repo; the layout crate is the model to imitate. |
| `tui/mod.rs` App-struct split (TUI-SPLIT-PLAN step 14) | Do AFTER cuts — splitting 2.8k of code you're about to shrink is wasted motion. |

## KEEP untouched

`run` (the product), PTY ownership, focus nav hjkl / move pane, new column-row / kill pane, viewport scroll-snap, OSC133 attention + smart jump, click + wheel, help overlay, `doctor`, `geometry.rs`, `config.rs` loader (minus deleted keys).

## Target config (6 keys, down from ~25)

`default_column_width`, `theme` (preset name only), `default_agent`, `agent_dir`, `startup_panes`, `keep_awake`. Everything else deleted above.

## Target docs (5 files, down from 30)

Keep: README, CONFIG, ARCHITECTURE, KEYBINDS, TERMINAL-COMPATIBILITY. Move `*-ACCEPTANCE.md`, `LAUNCH-*`, `DOCS-GROWTH-*`, `NAV-REWRITE-PLAN`, `TUI-SPLIT-PLAN`, bench json, `patches/` to `docs/archive/`.

## Cut order (each = one commit, `cargo test` + `clippy` green)

1. **P0 (~1.5k):** `new` stub, splash, cowsay, preview-orphans, 5 setup stubs, legacy color aliases, `scroll_margin`/`center_focus`/`content_width`. Zero-risk deletions.
2. **P1 (~3k):** `onboard/` deletion + `init` alias, `tune` deletion + hard-code poll 1ms, `install.rs` deletion, spawndir 3-key collapse.
3. **P2 (~1.5k):** chrome diet (minimap/HUD/toasts/ticks/big-labels), select→1 file, agent→2 files, `reap`→`pty` merge.
4. **P3 (~4k, needs your call):** graphics delete/flag, updater apply-paths delete, focus-daemon extraction, docs archive.

Estimated total: ~10k of 29k lines (~35%), ~19 config keys, 8 e2e files, 25 docs. What remains is multiplexer + panes + pickers + doctor + upgrade-check.
