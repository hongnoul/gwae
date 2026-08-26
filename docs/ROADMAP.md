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
- [x] Scrollback (`⌥+↑/↓`), drag-select copy, kitty graphics passthrough
- [ ] Scrollback search
- [ ] `⌥+y` yank (pane/turn, text or image) — spec below
- [ ] Scroll animation
- [ ] Windows ConPTY runtime verification (builds today, unverified at runtime)

## M4 - Polish and launch — DONE (v1.0.0/v1.0.1)
Themes/truecolor, VHS gifs, docs, Homebrew tap + AUR + nix + winget, prebuilt
binaries, `gwae setup`. **Exit**: v1.0.1 installable in one command (install.sh, Homebrew tap, AUR, crates.io; winget pending).

## M5 - Post-launch differentiation (ongoing)
Overview zoom, per-command pane rules, deeper PTY-compliant agent integration,
community presets.

### Yank: `⌥+y` pane/turn capture to clipboard (planned, M3/M5)

Drag-select already copies an arbitrary rectangle (`select.rs`). The gap is the
two scopes a user actually asks for by name: *this whole pane* and *this one
turn* — and copying either as an **image**, not just text.

**Binding.** One chord, `⌥+y`, not four. It copies **text immediately** using a
context-chosen scope, then leaves a short-lived overlay that can amend the
choice. Scope precedence:

1. live drag-selection in the focused pane → the selection;
2. else pane has OSC 133 marks → the current turn;
3. else → the full pane, top of scrollback to last line.

The toast reports what happened and advertises the rest, e.g.
`copied turn · 38 lines    p pane  s sel  ⇧ image`. Within `NOTE_LINGER`,
`p`/`t`/`s` re-yank at that scope and Shift makes it a PNG; anything else or the
timeout commits. Amending is safe because the clipboard is a scratch register —
unlike `⌥+q`, there is nothing to undo. `⌥+y y` repeats the last yank. Panes
without shell integration degrade loudly: `copied pane · no shell marks`.

**Enabling work, in dependency order.**

1. *Scrollback text API* in `gwae-term`: `TermGrid` exposes only the visible
   screen plus `scrollback_offset`, so "top of session" is currently
   unreachable. Needs a row-range accessor over the vt100 scrollback.
2. *Prompt marks*: `tui.rs` keeps OSC 133 **status** (`saw_osc133`) but not
   positions. Store per-pane `PromptMark { abs_row, exit }`, fixed up as rows
   scroll out, to delimit a turn.
3. *Image encoder* (`shot.rs`): cells → RGBA via the `theme.rs` palette and an
   embedded mono font, out as PNG, so a shot looks like the pane.
4. *Image clipboard*: macOS `osascript` «class PNGf», Wayland `wl-copy -t
   image/png`, X11 `xclip -t image/png`, Windows `Set-Clipboard -Path`. No OSC
   52 equivalent exists, so over SSH write a file and toast the path.

**Phasing.** P1 `⌥+y` with pane+selection text only (needs 1). P2 turn scope
(needs 2) — one agent turn is the natural unit to paste elsewhere. P3 the image
variants (needs 3-4), behind an `image_clipboard` config key.

**Invariant to preserve.** `binds.rs` is the single source of truth and its test
feeds every entry through the real `handle_key`. A menu's second keystroke is
not expressible in today's `Effect`, so this needs `Effect::Menu(&[MenuItem])`
plus a test that presses `⌥+y` then each item key and asserts the resulting
`Cmd::Yank { scope, payload }`. Without that, the cheat-sheet and cowsay hints
could drift from the dispatcher — the exact failure that module exists to
prevent.

## M6 - Stability to 1.0
Layout spec frozen, fuzz the emulator, 1.0 after 6 months of dogfooding and no
data-loss bugs for 3 months.
