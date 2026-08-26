# Changelog

All notable changes to this project will be documented in this file (keep-a-
changelog, updated per PR). The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [1.0.1] - 2026-08-26

A maintenance release about staying out of the way: an idle session no longer
warms the laptop, and quitting gwae now really does take its processes with it,
whichever way you quit.

### Fixed
- **Quitting gwae leaves nothing running.** The force-quit overlay promises
  running commands are terminated immediately, and that is the one moment a
  user trusts a multiplexer. It was only true for jobs that stayed in the
  pane's process group: `nohup thing &`, a `setsid` daemon, or anything a
  pane's shell put in its own group via job control all survived and kept
  running with no window left to find them in. Teardown now walks the real
  process tree (collected before the root is signalled, since a dead root's
  children are reparented to init and become unfindable) and kills the group,
  the root, and every descendant deepest-first.

- **Being *killed* tears down the panes too, not just quitting.** `kill gwae`,
  closing the host terminal window (SIGHUP), a panic, or a failure during
  startup all bypassed teardown entirely and leaked the same detached work.
  Every exit path now funnels into one reaper: signal handlers for
  HUP/TERM/INT/QUIT, a panic hook, a drop guard over the TUI, and a final
  sweep on the normal path. The signal path does only async-signal-safe work
  and hands the process-table walk to a thread parked since startup, waiting
  on it with a 2s bound, because a mux that refuses to die when told to is
  worse than one that leaks. gwae still dies of the signal it was sent, so
  supervisors reading `128+signo` see the truth, and the terminal is handed
  back usable (alternate screen left, cursor shown) instead of stranding you
  in a black rectangle.

- **An idle session no longer burns ~3.5% of a core.** With nothing on screen
  changing, the render loop still polled the terminal size 500x a second; on
  macOS each of those opens and closes `/dev/tty`, which was ~75% of gwae's
  entire CPU time. The size check is now a 250ms backstop (the event path
  still handles every resize the host reports, instantly), and the input poll
  relaxes to 30ms after 750ms of quiet, snapping back on the first keystroke
  or byte of pane output. Measured on an idle 200x50 session: 3.4% -> 0.9%.

- **macOS: keystrokes dropped until you click, after switching Spaces.** kitty
  in native fullscreen owns its own Space; returning to it leaves the window
  `AXMain` but never key, so GLFW believes nothing is focused and discards
  every keystroke. kitty 0.48.2 carries the upstream fix for the windowed case
  but it is guarded on `!focusedWindowId` and does not cover native
  fullscreen. `docs/MACOS-FOCUS.md` documents both remedies and ships
  `gwae-focus-fix`, an NSWorkspace observer that re-focuses the window.

### Changed
- The E2E harness no longer hangs on an animating screen. `press()` drained
  "until output goes quiet", which never happens once the onboarding banner
  animates, and `screen()` could capture a half-written frame. Draining is now
  bounded and `screen()` returns a frame known to have been written end to
  end, so the suite reports a result instead of having to be killed by hand.

## [1.0.0] - 2026-08-25

First stable release. gwae ships as one static binary for macOS, Linux,
and Windows with the scrolling strip grid, quantized viewport, OSC 133 agent
dashboard, smart-jump, guided onboarding, Kitty graphics passthrough, theme
presets, and live-PTY E2E coverage. MIT licensed.

### Added
- **`⌥+↑` / `⌥+↓` read back through a pane's scrollback.** With the wheel no
  longer claimed by gwae (see Removed), the keyboard is the only route into
  a pane's history, so it needed one: `⌥+↑`/`⌥+↓` move three rows a notch, and
  `⌥+Shift+↑/↓` or `⌥+PageUp/PageDown` move about a screenful. Typing snaps the
  pane back to live, and a full-screen app on the alternate screen (vim,
  `less`) has no scrollback of ours to move, so it gets the arrow keys it
  expects instead. Covered by `tests/scrollback_e2e.rs`, which drives the real
  binary and reconstructs the painted screen.

- **Onboarding offers to install `btm`.** The last question of the guided setup
  offers [bottom](https://github.com/ClementTsang/bottom), the system monitor
  that makes a good permanent neighbour to an agent pane, defaulting to yes. On
  macOS a yes installs Homebrew first when it is missing: that is gwae's
  implementation detail, so it happens silently rather than becoming a second
  question about a package manager the user may never have heard of. The offer
  is skipped entirely when `btm` is already installed (a question whose only
  honest answer is "already done" teaches people that setup asks things it
  already knows), and `GWAE_NO_INSTALL=1` turns it off for unattended runs.
  The summary reports what actually happened on the machine, not what was
  answered: "yes" and "installed" are different claims.

- **`gwae tune` reports input latency across all three layers.** A keystroke
  crosses macOS, your terminal, and gwae — and gwae twice, since what
  you see is the program's echo making the return trip. Tuning only gwae's
  own knob therefore fixes a third of the problem. `tune` probes all three and
  says which are slower than they need to be, with the exact command for each.
  It writes **only gwae's own config**; macOS globals and your terminal's
  config are printed for you to apply, never edited silently. First-run
  onboarding offers the same thing once, right after you pick an agent, and
  stays silent when there is nothing to fix. `doctor` carries a summary line,
  and `docs/LATENCY.md` explains the reasoning (including why removing
  gwae's wait entirely would make typing *worse*, not better).

- **The first pane opens on your agent, not a bare shell.** gwae exists to
  drive agent harnesses, but every launch dropped you at a shell prompt to
  type the harness name yourself. Pane 1.1 now runs the same agent gateway
  `⌥+;` uses, so startup has exactly two outcomes: your configured agent is
  already running, or you get the selector and pick one (which is then saved,
  making every later launch the first case). `gwae run <cmd>` still wins,
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
  gateway* (`gwae agent`) in the pane: when `default_agent` resolves it
  `exec`s it immediately and paints nothing, and otherwise it lists the
  harnesses actually found on your `PATH` (`jcode`, `claude`, `codex`,
  `gemini`, `opencode`, `crush`, `aider`, `cursor-agent`, `amp`, `goose`),
  runs the one you choose, and saves it as `default_agent` so the next `;` is
  instant. Choosing nothing, or having nothing installed, opens a plain
  `$SHELL` with a note saying why. Because the gateway `exec`s, the pane's
  process *is* the harness: same pid, same PTY, and its own window title still
  reaches the host. `gwae doctor` reports the same resolution, and
  `gwae agent --print` shows it without prompting or running anything.

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
- **Onboarding is one question per screen, driven with the arrow keys.** The
  guided setup was a single long transcript of numbered lists that scrolled
  past as you answered, and every question needed a number followed by Enter.
  It now clears to one question at a time: `↑↓` or `j`/`k` moves the highlight,
  `→`/`l`/`⏎` goes to the next question, `←`/`h`/`⌫` goes *back* to the
  previous one with your earlier answer still selected, and a digit picks an
  option outright without an Enter. The flow ends on a summary screen listing
  every setting as it now stands and the file it landed in — itself a step you
  can back out of, so an answer you regret on the way out is fixable without
  re-running setup.

- **Latency tuning is applied silently, before the first question.** It used to
  be a prompt at the end of onboarding, which asked users to adjudicate a
  number they cannot evaluate. `input_poll_ms` has exactly one right answer, so
  gwae now sets it in its own config file before setup draws anything, and
  the summary screen reports only what is left for the user to do (kitty and
  macOS settings, which gwae will never edit for them).

- **The inset skeleton frames now ship off.** Framing every column and insetting
  its content by a cell is a strong look, and it was both the default and a
  setup question. Panes are now full-bleed out of the box, with focus shown as
  an accent background tint; `skeleton = true` brings the frames back for
  anyone who wants them, and setup no longer asks. Placeholder boxes are no
  longer coupled to the frames, so an empty grid still shows where panes go
  (and still advertises the keybindings via `[cowsay]`) either way.

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

### Removed
- **Mouse wheel scrollback, and the `mouse` / `scroll_lines` keys.** gwae
  captured the wheel to scroll the pane under the cursor, which meant it also
  had to translate the wheel into arrow keys for alt-screen pagers, and it put
  two knobs in the config (and two questions in setup) for behavior most users
  never asked to change. The wheel now goes to a child that requested mouse
  reporting, verbatim in its own coordinates, and nowhere else. Mouse capture
  itself stays on, since click-to-focus and drag-to-copy depend on it. Old
  configs keep loading: the keys are simply no longer read. Scrollback moves
  with the new `⌥+↑/↓` binding below.

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
  gwae still paints zero cells per frame. The identifier always wins: boxes
  too narrow (under 23 cells) or too short for both degrade to the label alone
  rather than a clipped cow. Configure via `[cowsay]` `enabled` / `messages`;
  both are picked up by live config reload.
- **Kitty graphics passthrough: images now render inside panes**: vt100 (the
  hosted emulator) silently swallows Kitty graphics APCs, so any child that
  drew images (jcode diagrams/screenshots, `kitten icat`) showed nothing.
  gwae now scans each pane's raw output with a chunk-safe state machine,
  forwards complete `ESC _ G … ESC \` sequences verbatim to the host terminal
  (when the host speaks the protocol: Kitty/Ghostty/WezTerm by env, or forced
  via `GWAE_KITTY_GRAPHICS=1/0`), and preserves the combining diacritics on
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
- **Touchpad/wheel scrolling scrolls the pane, not the host terminal**: gwae
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
  Closing the last pane quits gwae. Previously an exited pane lingered as a
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
- **M0 renderer**: single-process, multi-pane PTY cell renderer. The `gwae`
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
  `gwae` (bin), `gwae-layout`, `gwae-term`, `gwae-testkit`.
- `gwae-layout`: the pure 2D grid-of-strips core (rows/columns/panes,
  follow-focus scroll, verbs) with `proptest` invariant properties.
- `gwae-term`: the `TermGrid` emulator facade (ADR-004) + `NullGrid`.
- `gwae-testkit`: `FakeTerminal` for scripted/rendered-frame tests.
- `gwae` bin: `clap` CLI (`run`/`new`/`setup`/`doctor`) + TOML config loader.
- MIT license, docs (ARCHITECTURE / LAYOUT-SPEC / COMPARISON / CONFIG / ROADMAP),
  CI workflow, packaging scaffolding, scripts, issue templates.
