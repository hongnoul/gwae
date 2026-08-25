# Changelog

All notable changes to this project will be documented in this file (keep-a-
changelog, updated per PR). strimux is pre-1.0; the format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- **`strimux tune` reports input latency across all three layers.** A keystroke
  crosses macOS, your terminal, and strimux — and strimux twice, since what
  you see is the program's echo making the return trip. Tuning only strimux's
  own knob therefore fixes a third of the problem. `tune` probes all three and
  says which are slower than they need to be, with the exact command for each.
  It writes **only strimux's own config**; macOS globals and your terminal's
  config are printed for you to apply, never edited silently. First-run
  onboarding offers the same thing once, right after you pick an agent, and
  stays silent when there is nothing to fix. `doctor` carries a summary line,
  and `docs/LATENCY.md` explains the reasoning (including why removing
  strimux's wait entirely would make typing *worse*, not better).

- **The first pane opens on your agent, not a bare shell.** strimux exists to
  drive agent harnesses, but every launch dropped you at a shell prompt to
  type the harness name yourself. Pane 1.1 now runs the same agent gateway
  `⌥+;` uses, so startup has exactly two outcomes: your configured agent is
  already running, or you get the selector and pick one (which is then saved,
  making every later launch the first case). `strimux run <cmd>` still wins,
  since that is you being specific.

- **Detection is no longer limited to an allowlist.** The picker only knew a
  fixed list of harnesses, so anything newer than the release — or a personal
  wrapper script — was invisible. It now merges three sources: `agents` from
  your config, the names it can label, and a scan of `PATH` for agent-shaped
  commands. You can also just type a command at the prompt. The scan skips
  system directories, without which a stock macOS `PATH` offers `ssh-agent`,
  `KernelEventAgent`, `b64encode` and the disk tool `gpt` above the real
  entries.

- **`⌥+;` now works before you have picked an agent.** It ran `default_agent`
  blind, so on a machine without that harness installed the pane's child died
  the instant it spawned and left a blank box with no explanation — the same
  thing that happens if you typo the command. The key now opens an *agent
  gateway* (`strimux agent`) in the pane: when `default_agent` resolves it
  `exec`s it immediately and paints nothing, and otherwise it lists the
  harnesses actually found on your `PATH` (`jcode`, `claude`, `codex`,
  `gemini`, `opencode`, `crush`, `aider`, `cursor-agent`, `amp`, `goose`),
  runs the one you choose, and saves it as `default_agent` so the next `;` is
  instant. Choosing nothing, or having nothing installed, opens a plain
  `$SHELL` with a note saying why. Because the gateway `exec`s, the pane's
  process *is* the harness: same pid, same PTY, and its own window title still
  reaches the host. `strimux doctor` reports the same resolution, and
  `strimux agent --print` shows it without prompting or running anything.

- **`⌥+<number>` reaches columns past 9.** `⌥+1..9` jumped on the keystroke,
  which capped addressable columns at nine: there is no `⌥+10` key, so a wide
  strip could only be reached by walking `⌥+l`. Holding `⌥` is already a mode
  (it reveals the HUD/minimap), so digits typed while it is down now accumulate
  into one number and commit when the modifier is released — `⌥` + `1` `2`
  focuses column 12. A pending number is echoed along the bottom row so a
  half-typed address is never mistaken for a dropped keystroke. Terminals that
  do not report a bare `⌥` release (no Kitty keyboard protocol) commit on a
  500ms idle instead, and any other chord commits immediately, so single-digit
  jumps feel exactly as they did.

### Changed
- **`default_agent` now defaults to unset rather than `"jcode"`.** Defaulting
  to one vendor's harness meant every user who had not installed *that* tool
  hit the dead-pane path above on their first `⌥+;`. Unset is now a normal
  state that the gateway resolves interactively, so first run works no matter
  which agent you use. An explicit `default_agent` keeps behaving exactly as
  before.

- **Key hints now name the platform's own modifier.** The cowsay hints and the
  cheat-sheet HUD hard-coded macOS vocabulary (`⌥`, `↵`, `⇧`), which is
  meaningless on Linux/Windows where the same key is `Alt` and the glyphs may
  not even exist in the terminal font. Both surfaces resolve key names through
  a new `keys` module, so they read `Alt+g` / `Enter` off macOS and can never
  disagree with each other.

### Fixed
- **Hints no longer teach bindings that do not exist.** The default cowsay list
  advertised `c` for "new pane" (never implemented) and told users to "press ;"
  with no modifier at all, which just types a semicolon into the focused pane;
  the HUD listed the same phantom `c` and labelled `q` as "quit" when it kills
  a pane (`⇧q` quits). The hint list is rewritten from the real key table and
  covered by tests that fail if a hint names a dead binding.

### Added
- **Empty grid cells now document themselves with a cowsay hint**: an empty
  placeholder box showed only its big block-font `strip.cell` identifier, which
  says *where* you are but not what to do about it. Each empty box now draws a
  small cow under the identifier speaking a keybinding hint (`Alt-Enter opens a
  column here`, `Press ; to spawn an agent`, ...), so a fresh workspace teaches
  its own bindings. The art is generated in-process (no `cowsay(1)` dependency)
  and wrapped to the box width. Which box says what is chosen by hashing the
  cell's position, never randomly, so a box always says the same thing and idle
  strimux still paints zero cells per frame. The identifier always wins: boxes
  too narrow (under 23 cells) or too short for both degrade to the label alone
  rather than a clipped cow. Configure via `[cowsay]` `enabled` / `messages`;
  both are picked up by live config reload.
- **Kitty graphics passthrough: images now render inside panes**: vt100 (the
  hosted emulator) silently swallows Kitty graphics APCs, so any child that
  drew images (jcode diagrams/screenshots, `kitten icat`) showed nothing.
  strimux now scans each pane's raw output with a chunk-safe state machine,
  forwards complete `ESC _ G … ESC \` sequences verbatim to the host terminal
  (when the host speaks the protocol: Kitty/Ghostty/WezTerm by env, or forced
  via `STRIMUX_KITTY_GRAPHICS=1/0`), and preserves the combining diacritics on
  `U+10EEEE` Unicode-placeholder cells through the grid and the painter, so
  virtual placements land exactly inside their pane and clip with it. Graphics
  *queries* (`a=q`) are dropped rather than forwarded because the host's reply
  cannot be routed back to the child. Combining marks on ordinary text
  (é as `e`+U+0301) also survive the grid now instead of being stripped.
- **The minimap is now an agent dashboard**: each pane's tile is tinted by
  live status - blue `»` working, amber `!` wants attention, green `✓` done,
  red `✗` failed (non-zero exit) - with the pane's `⌥+digit` column address in
  its first cell, a `❯` chevron marking the focused strip, and a one-line
  summary above the map tallying panes by status (`5 »2 !1 ✓1 ✗1`). Status is
  driven by OSC 133 shell integration when the pane emits it (`A` prompt,
  `C` running, `D;n` done/failed); panes without shell integration fall back
  to an output-activity heuristic (a few seconds of silence → wants
  attention). The map now appears whenever more than one pane exists, not
  only with multiple strips. Configurable under `[minimap]`
  (`show`, `max_width`, `max_rows`, `show_counts`).
- **Smart-jump (`⌥+g`)**: jump straight to the pane that needs
  you - failed beats waiting-for-input beats done, nearest first in layout
  order - crossing strips and following with the scroll. Does nothing while
  every other pane is happily working.
- **Touchpad/wheel scrolling scrolls the pane, not the host terminal**: strimux
  now captures the mouse, so a scroll gesture moves the scrollback of the pane
  under the cursor instead of falling through to the host terminal (where it
  scrolled the host's own buffer and walked the shell's previous/next prompt
  history). Panes that requested mouse reporting receive the event verbatim in
  their own grid coordinates; a pane on the alternate screen without mouse
  reporting (e.g. `less`) receives arrow keys. Typing snaps a scrolled-back
  pane back to the live bottom. Configurable with `mouse` and `scroll_lines`.

### Fixed
- **Pane close keeps the focus position**: closing a pane (`⌥+q` or process
  exit) used to always shift focus to the left neighbor. Focus now stays in
  the same slot, taking over the column that compacts in from the right, and
  only moves left when the closed pane was the rightmost one.
- **SGR attributes no longer bleed across a row ("line overflow")**: the
  painter reset attributes once per row but SGR codes are additive, so an
  underlined/bold run (e.g. a popup's underlined entries) leaked its
  attributes into every later run on that row, drawing underlines out to the
  right screen edge. The painter now resets attributes at the start of every
  style run.
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
  **stays in the same slot**, landing on the column that slides in from the
  right (or the pane above in a stack), and only falls to the left neighbor
  when nothing is left to the right.
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
- **Spawn an agent pane with `;`**: `⌥+;` (the Option key on macOS)
  spawns a new pane running the configured agent harness (`config.default_agent`,
  default `jcode`) at the rightmost of the focused strip and switches focus to
  it. Configurable via `default_agent = "claude"` (or any command).
- **Keybinding tweaks**: `⌥+q` now kills the focused pane (matching `⌥+x`)
  instead of quitting; quit is `⌥+Shift+q`. On macOS the `⌥+;` / `⌥+q`
  chords work out of the box (Option+`;` = `…`, Option+`q` = `œ`), no
  Option-as-Alt needed.
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
