# strimux

> **niri's scrolling tiling, for your CLI agents, in any terminal.**

**strimux** is a terminal-native, daemon-free multiplexer for people who live in concurrent CLI agents. Claude Code, Jcode, yazi, nvim — each owns a real PTY. strimux gives them room: an **infinite 2D grid of strips** where every column keeps its full size, the row scrolls past the viewport edge, and strips stack infinitely downward. No compositor. No GUI. No daemon. Any terminal on **macOS, Windows, and Linux**.

- **Panes never shrink.** Fixed-width columns (`1/4` by default, `1/3 → 1/2 → 1/4` on `⌥+r`). A row that outgrows the screen scrolls — it doesn't cram.
- **Quantized scroll.** Viewport rests only on column boundaries, so the same grid paints pixel-identically in every scroll state. No slivers, no wobble.
- **Agent-aware, zero instrumentation.** Speaks the standard **OSC 133** protocol only. Panes stay ordinary PTYs. The minimap tints by status and `⌥+g` jumps to the one that needs you.
- **Single process, no daemon.** No socket, no attach/detach. Crashing one pane's emulator can't take the TUI down. Persistence is each harness's own `--resume`.
- **Kitty graphics passthrough.** `kitten icat` and jcode screenshots render inside their pane — APC sequences forwarded verbatim and clipped to the pane rect.
- **Mouse that helps, and stays out of the way.** Click to focus, drag to copy. A pane running a full-screen app that asked for mouse reporting gets every event forwarded verbatim in its own coordinates, so the wheel behaves inside vim or an agent TUI exactly as it would natively. strimux claims no wheel of its own; scrollback is `⌥+↑/↓`.
- **Catppuccin Mocha by default.** Base `#1e1e2e`, focus sapphire `#74c7ec`, overlay `#6c7086`. Every color is themeable, 8 presets ship, `⌥+t` previews them live, and saving the config re-themes the running session without restarting a single pane.

```sh
cargo install --path crates/strimux   # or: cargo build --release && make install
strimux                                # one strip, one quarter-width pane
strimux run "htop"                     # same layout; htop in the first pane
```

---

## Status

**M0 renderer shipped, M1/M2 substantially landed, pre-1.0.**

What works today and is interactively dogfoodable:

- Single-process, multi-pane PTY renderer: spawns real PTYs, composes every pane into one 2D cell buffer, diffs and paints with synchronized-update markers. Full 300×80 repaint ~0.05 ms.
- Pure layout core (`strimux-layout`) with quantized scrolling, fixed-width columns, niri-style dynamic strips and cross-strip pane moves. Covered by `proptest` invariants and render-frame tests.
- Emulator facade (`strimux-term`) behind `TermGrid`, unit-tested; E2E tests drive the real binary through a live PTY (minimap status, smart-jump, natural pane close, 342-col wobble test).
- Agent dashboard minimap (OSC 133 + quiet-heuristic fallback), smart-jump, skeleton chrome with inset content, kitty-like block cursor, mouse capture, Kitty graphics forwarding, wide-glyph / SGR-attribute fixes.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/LAYOUT-SPEC.md`](docs/LAYOUT-SPEC.md) (normative), and [`docs/ROADMAP.md`](docs/ROADMAP.md).

> You're 61 commits ahead of `origin/main` (scaffold era). The README on `origin` still says "Scaffold / M0". This draft corrects it.

---

## Install

**From source (recommended while pre-1.0):**

```sh
# installs to wherever cargo puts binaries (usually ~/.cargo/bin)
cargo install --path crates/strimux

# or, build then drop into the first writable bin on your PATH
cargo build --release        # -> target/release/strimux
make install                 # picks ~/.cargo/bin, /opt/homebrew/bin, ~/.local/bin, etc.
# override: PREFIX=~/bin make install
```

**Packaging scaffolding** lives in `packaging/` (Homebrew `strimux.rb`, AUR `PKGBUILD`, Nix `flake.nix`, Windows notes). Prebuilt binaries / taps land in M4.

**Requirements:** 256-color + cursor addressing minimum. Wants truecolor, synchronized updates (`ESC[?2026h`), kitty keyboard protocol, and SGR mouse — all auto-detected, all gracefully degraded. Rust 1.85+.

---

## Quick start

```sh
strimux                     # default: one strip, one 1/4-width pane + placeholder boxes
strimux run "claude"        # command runs in column 0 (deterministic), rest are shells ($SHELL)
strimux new -- htop         # (subcommand form) new column in a fresh session
strimux init                # guided setup: theme, layout, chrome, btm (safe to re-run)
strimux setup               # optional per-terminal bindings (e.g. Cmd+hjkl on iTerm2/kitty)
strimux doctor              # diagnostics: config + theme validity, layout smoke
```

The default layout is **one strip, one quarter-width column**. Skeleton placeholders tile the empty right side so the 4-column container always reads (each shows a big `strip.cell` address). Set `startup_panes` to open more panes immediately. New columns appear to the **right of the focused pane**, not at the strip end.

Content width is decoupled from column width via `content_width`. Default `0` follows the column (lines wrap); set e.g. `240` to give panes a wider logical grid and pan inside the pane with `⌥+←/→`.

---

## Keybindings

Every action is an **`⌥` chord**. `⌥` is the **Option key on macOS**, Alt elsewhere. It works when the terminal delivers Option as Meta/Alt, plus a fallback that decodes macOS's Unicode glyphs (`…` for `⌥+;`, `œ` for `⌥+q`, `ÓÔÒ` for `⌥+Shift+hjkl`) with no "Option as Alt" toggle.

| `⌥` chord | Action |
|---|---|
| `⌥+h` / `⌥+l` | Focus left / right (adjacent column; scrolls minimally to reveal) |
| `⌥+k` / `⌥+j` | Focus up / down within stack; at edge crosses strips. Past the last strip creates an empty strip (niri workspace semantics); leaving an empty strip discards it |
| `⌥+Shift+h` / `⌥+Shift+l` | Move focused column left / right (swap with neighbor) |
| `⌥+Shift+k` / `⌥+Shift+j` | Move pane up / down within stack; at the stack edge carries the pane to the neighboring strip (creating one past the end), discarding an emptied strip |
| `⌥+Enter` / `⌥+a` | New column to the right of focused |
| `⌥+Shift+Enter` | New strip (row) below the focused one |
| `⌥+;` (`…` on macOS) | Spawn agent pane at strip end and focus it (`default_agent`, or pick one if unset) |
| `⌥+s` | Split focused column — new pane below |
| `⌥+r` | Cycle focused column width `1/3 → 1/2 → 1/4` |
| `⌥+f` (`ƒ` on macOS) | Toggle focused column between full width and `1/4` |
| `⌥+x` / `⌥+q` (`œ`) | Kill focused pane — columns compact, focus keeps its slot (falls left only at right edge); emptied strip is dropped, last pane quits strimux |
| `⌥+←` / `⌥+→` | Scroll pane's logical content horizontally (when `content_width` > column width) |
| `⌥+[` / `⌥+]` | Scroll the row viewport left / right without moving focus |
| `⌥+↑` / `⌥+↓` | **Scrollback** — read back through the focused pane's history, 3 rows a notch. `⌥+Shift+↑/↓` and `⌥+PageUp/PageDown` move ~a screenful. Typing snaps back to live. A full-screen app (vim, `less`) owns its own scrolling, so it gets the arrow keys instead |
| `⌥+←` / `⌥+→` | Pan wide content sideways when `content_width` exceeds the column (`⌥+Shift` for a bigger step) |
| `⌥+1` … `⌥+9` | Jump to column N in focused strip. Keep `⌥` down and keep typing to address columns past 9 (`⌥` + `1` `2` → column 12); the number commits when `⌥` is released, or after ~500ms on terminals that don't report the release |
| `⌥+g` (`©`) | **Smart-jump** — jump to pane that needs you (see below) |
| `⌥+t` (`†` on macOS) | **Theme picker** — step presets with `←`/`→`, live-previewed on the real UI; `⏎` keeps, `esc` restores |
| `⌥+/` or `⌥+?` (`÷` / `¿` on macOS) | **Toggle the cheat-sheet HUD** — same overlay shown at startup; any other key dismisses it |
| `⌥+Shift+q` | Force-quit strimux — opens a centered confirmation overlay; press `⌥+Shift+q` again (or `⏎`) to kill every pane, any other key cancels |
| click | Left-click focuses the clicked pane |
| drag | Left-drag inside a pane selects text (inverse highlight) and copies it on release. Panes that grab the mouse (vim, agent TUIs) keep it, so hold `Shift` there to select instead |
| wheel | Forwarded as SGR to a pane that asked for mouse reporting (vim, agent TUIs), so it behaves natively there. strimux claims no wheel of its own |

All other keys pass through to the focused pane. Closing a pane by `exit` / process death behaves identically to `kill-pane`.

### Adding a keybinding

Bindings live in **one place**: `crates/strimux/src/binds.rs`. Each entry declares its trigger, the `Cmd` it must produce, a short cheat-sheet label, and a **mandatory one-line cowsay hint** — the `hint` field is not optional, so a binding and its natural-language explanation are bijective by construction and the cow can never fall behind the dispatcher. The cheat-sheet HUD, the cowsay hints and this table all render from that registry.

The dispatcher in `tui::handle_key` remains the authority; the registry only *claims* what it does. Tests enforce the agreement, so a new or re-bound key fails the build until every surface is consistent:

- `advertised_bindings_match_the_dispatcher` — replays each entry (Meta path plus the macOS glyph fallback) through the real `handle_key` and requires the declared effect.
- `hints_are_bijective_with_bindings` — one non-empty, unique hint per binding, each leading with its own key label.
- `every_binding_is_documented_in_the_readme` — the table above must list it.

---

## Agent awareness

### OSC 133 status

If a pane's shell emits [OSC 133](https://gitlab.freedesktop.org/terminal-wg/specifications/-/blob/master/docs/OSC-133.md) (`A` prompt → `C` running → `D;n` done/failed), strimux tracks per-pane status natively:

- `»` **Working** (blue) — command running
- `!` **Wants attention** (amber) — idle with output / prompt waiting
- `✓` **Done** (green) — exited 0
- `✗` **Failed** (red) — non-zero exit

Panes without shell integration fall back to a **quiet heuristic**: a pane silent for a few seconds flips to `!` so the dashboard still triages it.

### Minimap — an agent dashboard

No bottom status row. Hold `⌥`/Alt to see status (centered, no pane shrinkage):

- **Centered minimap** — one row per strip, one tile per pane (width ∝ column share), tinted by status, focused tile in `focus_color`, `❯` on focused strip, digit `⌥+1..9` per tile, summary `5 »2 !1 ✓1 ✗1`.
- **Centered HUD** — ` » 1.3 needs you — ⌥+g` + cheat-sheet, shown once at startup, toggled any time with `⌥+/`, and dismissed on the first key press. While the HUD is visible the minimap is hidden beneath it.

The minimap/HUD appears whenever there is more than one pane (or strip).

| `minimap.mode` | Behavior |
|---|---|
| `off` *(default)* | No persistent chrome. `⌥` reveals the centered HUD + minimap. |
| `overlay` | Legacy bottom-right overlay — `max_width`/`max_rows` apply |
| `edge_ticks` | Single-cell ticks on the outer frame, no box |

Kill-switch `minimap.show = false` still respected.

### Smart-jump

`⌥+g` jumps to the pane that needs you most — **failed beats wants-attention beats done**, nearest in layout order first, crossing strips and following with the scroll. Does nothing when every other pane is happily working. Proven E2E: jump lands on the attention shell, typing there succeeds and flips `✗` on the command that emitted `D;2`.

---

## Appearance

- **Skeleton** (always on, no key): 1-cell frame around every column box at full strip height, with content inset 1 cell so the frame never covers what a program draws. The focused box's frame is the accent color. Placeholder boxes tile the empty right side and can show block-font `strip.cell` addresses (`cell_labels`) and keybinding hints (`[cowsay]`).
- **Focus**: accent hairline (never shifts layout) + kitty-like inverse block cursor at the focused pane's vt100 cursor.
- **Palette**: Catppuccin Mocha by default — base `#1e1e2e`, overlay `#6c7086`, accent `#74c7ec` (sapphire, distinct from red `Failed`). Pick a preset with `theme = "nord"` (also `catppuccin-latte`, `tokyo-night`, `gruvbox`, `rose-pine`, `dracula`, `terminal` which inherits the host's ANSI 0-15), or override any key in `[theme]` (`base`, `surface`, `overlay`, `accent`, `text`, `label`, `running`, `idle`, `done`, `failed`). Minimap tiles at 60% muted accents, summary at full. Legacy `background`/`focus_color`/`skeleton_color` still work as aliases for `theme.base`/`accent`/`overlay`. All colors as `256-index`, `#rrggbb`, or `"default"`.
- **Pane geometry**: window-anchored column boundaries + quantized stops mean the same four `1/4` columns paint identically at every scroll stop even at hostile widths like 342 cols (verified E2E).

---

## Configuration

The first time `⌥+;` (or the first pane) opens the agent gateway, strimux runs
a short guided setup. It opens with a one-second animated title card (the
wordmark wipes in, painted in the palette you are about to be asked about; any
key skips it, and it only greets you on a genuine first run), then after
picking your harness you get one question per screen: theme (with live color
swatches), panes at launch, column width, scroll style, logical pane width,
cell labels, keybinding hints, and finally an offer to install
[`btm`](https://github.com/ClementTsang/bottom), the system monitor that makes
a good neighbour to an agent pane.

`↑↓` or `j`/`k` picks an option, `→`/`l`/`⏎` moves to the next question,
`←`/`h`/`⌫` goes back to the previous one, a digit selects without Enter, `s`
skips a question and `esc` takes the defaults for the rest. The flow ends on a
summary screen listing every setting as it now stands and the file it landed
in. Only two keys act there: `⏎` leaves, `⌫` goes back to fix an answer.
Everything else is inert, so the one screen reporting what was written to disk
cannot be dismissed by a stray keypress.

Two things are deliberately *not* questions. Input latency
(`input_poll_ms`) has one right answer, so it is applied silently before the
first question is drawn, and anything only you can change (kitty or macOS
settings) is reported once on the summary rather than interrupting the flow.
Installing `btm` needs Homebrew on macOS, so a yes installs that too rather
than asking about a package manager you may never have heard of; it is skipped
entirely if you already have `btm`, and `STRIMUX_NO_INSTALL=1` turns the offer
off for unattended runs.

Setup runs once. `strimux init` re-runs it any time (defaults become whatever
your config currently says, so accepting every default changes nothing),
`strimux init --print` shows every question without asking, and
`strimux init --print-splash` dumps every frame of the title card. The card is
plain SGR over full repaints, so it animates correctly whether it runs on a
bare terminal or inside a strimux pane.

File: `$XDG_CONFIG_HOME/strimux/strimux.toml` (or `~/.config/strimux/strimux.toml`). TOML, all keys optional. See [`docs/CONFIG.md`](docs/CONFIG.md) (generated from code).

```toml
default_column_width = "quarter"  # or "half", "two-thirds", "full", or 80 (cells)
scroll_margin = 2
center_focus = false
content_width = 0
default_agent = "claude"          # first pane + ; spawn this
startup_panes = 1
theme = "catppuccin-mocha"        # palette preset (see Appearance)
# [theme]
# preset = "nord"
# accent = "#ff0000"              # override any key on top of the preset
# overlay = "#665c54"
# legacy aliases (override theme when set):
# background = "#1e1e2e"          # -> theme.base
# focus_color = "#74c7ec"         # -> theme.accent
# skeleton_color = "#6c7086"      # -> theme.overlay
cell_labels = false

[minimap]
show = true
mode = "off"                       # off | overlay | edge_ticks  (reserved / reserved_quasimode parse as off)
max_width = 32                     # overlay + centered Alt minimap
max_rows = 6                       # overlay + centered Alt minimap
show_counts = true
```

Key reference (defaults in parentheses):

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `quarter` | Preset `quarter`/`third`/`half`/`two_thirds`/`three_quarters`/`full` or `{ cells = N }` |
| `scroll_margin` | int | `2` | Reserved under quantization (kept for future continuous mode) |
| `center_focus` | bool | `false` | Center focused column at nearest quantized stop |
| `content_width` | int | `0` | Logical pane width; `0` = follow column width (wrap). `>0` = horizontal overflow panned with `⌥+←/→` |
| `default_agent` | string | *(unset)* | Agent for the first pane and `;`. Unset means you get the selector, which saves your pick |
| `agents` | array | `[]` | Extra commands to offer in the selector |
| `startup_panes` | int | `1` | Quarter-width panes at launch; the remainder shows as placeholder boxes |
| `theme` | string/table | `catppuccin-mocha` | Palette preset or `[theme]` table with `preset` + per-key overrides; see Appearance / `docs/CONFIG.md` |
| `background` | color | theme `base` | Legacy alias for `theme.base` |
| `focus_color` | color | theme `accent` | Legacy alias for `theme.accent` |
| `skeleton` | bool | `false` | Draw 1-cell inset column frames. Hand-edit only; setup does not ask |
| `skeleton_color` | color | theme `overlay` | Legacy alias for `theme.overlay` |
| `cell_labels` | bool | `false` | Block-font `strip.pane` addresses in placeholder boxes |
| `onboarded` | bool | *(unset)* | Written by setup so it is offered once; delete to be offered again |
| `minimap.show` | bool | `true` | Master kill-switch |
| `minimap.mode` | enum | `off` | Chrome presentation (`off`=only centered Alt HUD/minimap, `overlay`=corner, `edge_ticks`=frame ticks; legacy `reserved`/`reserved_quasimode` parse as `off`) |
| `minimap.max_width` | int | `32` | Width of `overlay` and centered Alt minimap |
| `minimap.max_rows` | int | `6` | Rows of `overlay` and centered Alt minimap |
| `minimap.show_counts` | bool | `true` | Summary tallies |

---

## Why strimux

tmux divides a fixed screen; strimux scrolls an infinite one. Séance/tairi need a GUI or compositor; strimux runs in the terminal you already use. See [`docs/COMPARISON.md`](docs/COMPARISON.md).

| Project | Layout | In a terminal? | Detach? | Platforms |
|---|---|---|---|---|
| tmux | plane tiling | Yes | Yes (server) | macOS/Linux/*BSD |
| Zellij | plane tiling + floating | Yes | Yes | macOS/Linux/Windows |
| Séance | niri strip (GUI) | No | socket | Linux (GTK) |
| tairi | niri strip (GUI) | No | workspaces | macOS |
| **strimux** | **2D niri strip grid** | **Yes** | **No (`--resume`)** | **macOS/Windows/Linux** |

**No-shrink** — agents stay readable. **Niri feel** — `⌥+hjkl` / `⌥+Shift+hjkl`, dynamic strips, quantized stops, minimal follow-focus. **No daemon** — if you need SSH persistence that outlives the process, keep tmux; strimux delegates to `claude --resume` / `jcode --resume`.

---

## Architecture

Single process, no client-server (see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)):

```
strimux (one process)
├── Layout core (rows/strips/columns/panes) — pure, no I/O
├── Pane tasks (one per PTY: bytes → parse → grid + OSC 133)
├── Composer (coalesce damage → single 2D cell buffer)
├── Render (diff buffer → batched ANSI, sync-update markers)
├── Input (raw mode: decode keys, route to pane or ⌥)
└── Minimap / smart-jump (per-pane status → chrome)
```

| Crate | Role |
|---|---|
| `strimux` | bin — raw mode, PTY hosting, composer, render, input, minimap, OSC 133, Kitty APC forwarding, mouse |
| `strimux-layout` | pure 2D grid + verbs + quantized scroll + minimap model. `std` + `serde` only. `proptest` invariants |
| `strimux-term` | `TermGrid` emulator facade + damage tracking (isolates the emulator-crate choice) |
| `strimux-testkit` | fake PTYs + scripted terminals + snapshot harness |

`scroll_x` only rests on column-boundary stops (or `max_scroll`); follow-focus picks the nearest stop that fully reveals the focused column. See [`docs/LAYOUT-SPEC.md`](docs/LAYOUT-SPEC.md) for the normative spec.

---

## Non-goals

Read before filing a feature request:

- **No daemon / detach / attach.** Your agents already `--resume`.
- No free 2D canvas — we are a structured grid of strips.
- No floating panes, no mouse-driven chrome beyond scroll/click-to-focus/drag-to-copy, no plugin system, no overview zoom (post-1.0).

The hot-module-reload scaffold briefly lived in-tree to develop strimux inside strimux; it was removed — the shipped binary never depended on it and it drifted from the real painter.

---

## Development

```sh
cargo build --release
cargo test --workspace          # layout proptests + unit tests + live-PTY E2E
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
make check                      # clippy
make test                       # cargo test --workspace
```

Layout invariants live as `proptest` properties in `crates/strimux-layout/tests/invariants.rs` (quantized-stop, tiling, shape-identity, focus-never-clipped, page-stop, cross-strip move). E2E PTY tests live in `scripts/e2e_*.py` and under `crates/strimux/src/tui.rs`.

---

## License

MIT. See [LICENSE](LICENSE).
