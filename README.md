# strimux

> **niri's scrolling tiling, for your CLI agents, in any terminal.**

**strimux** is a terminal-native, daemon-free multiplexer for people who live in concurrent CLI agents. Claude Code, Jcode, yazi, nvim — each owns a real PTY. strimux gives them room: an **infinite 2D grid of strips** where every column keeps its full size, the row scrolls past the viewport edge, and strips stack infinitely downward. No compositor. No GUI. No daemon. Any terminal on **macOS, Windows, and Linux**.

- **Panes never shrink.** Fixed-width columns (`1/4` by default, `1/3 → 1/2 → 1/4` on `⌥+r`). A row that outgrows the screen scrolls — it doesn't cram.
- **Quantized scroll.** Viewport rests only on column boundaries, so the same grid paints pixel-identically in every scroll state. No slivers, no wobble.
- **Agent-aware, zero instrumentation.** Speaks the standard **OSC 133** protocol only. Panes stay ordinary PTYs. The minimap tints by status and `⌥+g` jumps to the one that needs you.
- **Single process, no daemon.** No socket, no attach/detach. Crashing one pane's emulator can't take the TUI down. Persistence is each harness's own `--resume`.
- **Kitty graphics passthrough.** `kitten icat` and jcode screenshots render inside their pane — APC sequences forwarded verbatim and clipped to the pane rect.
- **Mouse that helps.** Wheel scrolls the pane under the cursor (its own scrollback), not the host terminal. Click to focus. `less` without mouse reporting gets arrow keys; typing snaps back to live.
- **Catppuccin Mocha by default.** Base `#1e1e2e`, focus sapphire `#74c7ec`, skeleton overlay `#6c7086`. Every color is themeable.

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
- Agent dashboard minimap (OSC 133 + quiet-heuristic fallback), smart-jump, skeleton chrome with inset content and kitty-like block cursor, mouse capture, Kitty graphics forwarding, wide-glyph / SGR-attribute fixes.

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
strimux                     # default: one strip, one 1/4-width pane + skeleton placeholders
strimux run "claude"        # command runs in column 0 (deterministic), rest are shells ($SHELL)
strimux new -- htop         # (subcommand form) new column in a fresh session
strimux setup               # optional per-terminal bindings (e.g. Cmd+hjkl on iTerm2/kitty)
strimux doctor              # diagnostics: terminal caps + $mod decoding
```

The default layout is **one strip, one quarter-width column**. Skeleton placeholders tile the empty right side so the 4-column container always reads (each shows a big `strip.cell` address). Set `startup_panes` to open more panes immediately. New columns appear to the **right of the focused pane**, not at the strip end.

Content width is decoupled from column width via `content_width`. Default `0` follows the column (lines wrap); set e.g. `240` to give panes a wider logical grid and pan inside the pane with `⌥+←/→` / `Ctrl-b ,/.`.

---

## Keybindings

`strimux` has **two equivalent bindings** so it works everywhere with zero configuration:

- **`Ctrl-b` prefix** — always works. Press `Ctrl-b`, the status line shows `^B`, then a command key. `Esc` cancels, `Ctrl-b Ctrl-b` sends a literal `Ctrl-b` to the pane.
- **`⌥` chords** — `⌥` is the **Option key on macOS**, Alt elsewhere. Works when the terminal delivers Option as Meta/Alt, plus a fallback that decodes macOS's Unicode glyphs (`…` for `⌥+;`, `œ` for `⌥+q`, `ÓÔÒ` for `⌥+Shift+hjkl`) with no "Option as Alt" toggle.

| Keys (`Ctrl-b` then …) | `⌥` chord | Action |
|---|---|---|
| `h` / `l` | `⌥+h` / `⌥+l` | Focus left / right (adjacent column; scrolls minimally to reveal) |
| `k` / `j` | `⌥+k` / `⌥+j` | Focus up / down within stack; at edge crosses strips. Past the last strip creates an empty strip (niri workspace semantics); leaving an empty strip discards it |
| `H` / `L` | `⌥+Shift+h` / `⌥+Shift+l` | Move focused column left / right (swap with neighbor) |
| `K` / `J` | `⌥+Shift+k` / `⌥+Shift+j` | Move pane up / down within stack; at the stack edge carries the pane to the neighboring strip (creating one past the end), discarding an emptied strip |
| `c` | `⌥+Enter` | New column to the right of focused |
| `;` | `⌥+;` (`…` on macOS) | Spawn agent pane (`default_agent`, default `jcode`) at strip end and focus it |
| `s` | `⌥+s` | Split focused column — new pane below |
| `r` (`z`) | `⌥+r` | Cycle focused column width `1/3 → 1/2 → 1/4` |
| `f` | `⌥+f` (`ƒ` on macOS) | Toggle focused column between full width and `1/4` |
| `x` | `⌥+x` / `⌥+q` (`œ`) | Kill focused pane — columns compact, focus keeps its slot (falls left only at right edge); emptied strip is dropped, last pane quits strimux |
| `,` / `.` | `⌥+←` / `⌥+→` | Scroll pane's logical content horizontally (when `content_width` > column width) |
| `[` / `]` | `⌥+Ctrl+h` / `⌥+Ctrl+l` | Scroll row viewport one quantized stop without moving focus |
| `1` … `9` | `⌥+1` … `⌥+9` | Jump to column N in focused strip |
| `g` | `⌥+g` (`©`) | **Smart-jump** — jump to pane that needs you (see below) |
| `q` | — | Quit strimux (kills all panes) |
| | click | Left-click focuses the clicked pane |
| | wheel | Scrolls pane scrollback under cursor; `Shift+wheel` etc. forwarded as SGR when pane wants mouse, else translated to `↑`/`↓` for alt-screen pagers |

All other keys pass through to the focused pane. Closing a pane by `exit` / process death behaves identically to `kill-pane`.

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
- **Centered HUD** — ` » 1.3 needs you — ⌥+g` + cheat-sheet, shown at startup and when any pane flips to `Idle`/`Failed`; persists until the next key press (`hud_on_attention_ms = 0` to disable). While the HUD is visible the minimap is hidden beneath it.

The minimap/HUD appears whenever there is more than one pane (or strip).

| `minimap.mode` | Behavior |
|---|---|
| `off` *(default)* | No persistent chrome. `⌥` reveals the centered HUD + minimap. |
| `overlay` | Legacy bottom-right overlay — `max_width`/`max_rows` apply |
| `edge_ticks` | Single-cell ticks on the outer frame, no box |

Kill-switch `minimap.show = false` still respected.

### Smart-jump

`⌥+g` / `Ctrl-b g` jumps to the pane that needs you most — **failed beats wants-attention beats done**, nearest in layout order first, crossing strips and following with the scroll. Does nothing when every other pane is happily working. Proven E2E: jump lands on the attention shell, typing there succeeds and flips `✗` on the command that emitted `D;2`.

---

## Appearance

- **Skeleton** (`skeleton = true`): 1-cell frame around every column box at full strip height, so the container always reads even with one pane. Content is inset 1 cell so the frame never covers what a program draws. Placeholders show big block-font `strip.cell` addresses. Focus frame uses `focus_color`, others use `skeleton_color`.
- **Focus**: sapphire `#74c7ec` hairline (never shifts layout) + kitty-like inverse block cursor at the focused pane's vt100 cursor.
- **Palette**: Catppuccin Mocha by default — `background #1e1e2e`, `skeleton_color #6c7086` (overlay0), `focus_color #74c7ec` (sapphire, distinct from red `Failed`). Minimap tiles at 60% muted accents, summary at full. All themeable as `256-index`, `#rrggbb`, or `"default"`.
- **Pane geometry**: window-anchored column boundaries + quantized stops mean the same four `1/4` columns paint identically at every scroll stop even at hostile widths like 342 cols (verified E2E).

---

## Configuration

File: `$XDG_CONFIG_HOME/strimux/strimux.toml` (or `~/.config/strimux/strimux.toml`). TOML, all keys optional. See [`docs/CONFIG.md`](docs/CONFIG.md) (generated from code).

```toml
default_column_width = { preset = "quarter" }  # or { cells = 80 }
scroll_margin = 2
center_focus = false
content_width = 0
default_agent = "claude"          # ; spawns this
startup_panes = 1
background = "#1e1e2e"             # Mocha base
focus_color = "#74c7ec"            # Mocha sapphire
skeleton = true
skeleton_color = "#6c7086"         # Mocha overlay0
mouse = true
scroll_lines = 3

[minimap]
show = true
mode = "off"                       # off | overlay | edge_ticks  (reserved / reserved_quasimode parse as off)
max_width = 32                     # overlay + centered Alt minimap
max_rows = 6                       # overlay + centered Alt minimap
show_counts = true
hud_on_attention_ms = 2500         # center HUD (startup + attention), 0 = off
```

Key reference (defaults in parentheses):

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `quarter` | Preset `quarter`/`third`/`half`/`two_thirds`/`three_quarters`/`full` or `{ cells = N }` |
| `scroll_margin` | int | `2` | Reserved under quantization (kept for future continuous mode) |
| `center_focus` | bool | `false` | Center focused column at nearest quantized stop |
| `content_width` | int | `0` | Logical pane width; `0` = follow column width (wrap). `>0` = horizontal overflow panned with `⌥+←/→` |
| `default_agent` | string | `jcode` | Command launched by `;` |
| `startup_panes` | int | `1` | Quarter-width panes at launch; remainder shows as skeleton placeholders |
| `background` | color | `#1e1e2e` | Empty background behind panes |
| `focus_color` | color | `#74c7ec` | Focus frame accent |
| `skeleton` | bool | `true` | Draw column frames + placeholders |
| `skeleton_color` | color | `#6c7086` | Unfocused frame color |
| `mouse` | bool | `true` | Capture wheel/click for pane scrollback |
| `scroll_lines` | int | `3` | Rows per wheel notch |
| `minimap.show` | bool | `true` | Master kill-switch |
| `minimap.mode` | enum | `off` | Chrome presentation (`off`=only centered Alt HUD/minimap, `overlay`=corner, `edge_ticks`=frame ticks; legacy `reserved`/`reserved_quasimode` parse as `off`) |
| `minimap.max_width` | int | `32` | Width of `overlay` and centered Alt minimap |
| `minimap.max_rows` | int | `6` | Rows of `overlay` and centered Alt minimap |
| `minimap.show_counts` | bool | `true` | Summary tallies |
| `minimap.hud_on_attention_ms` | int | `2500` | Centered HUD (startup + attention) with cheat-sheet, persists until key press; `0` disables (non-zero enables, value kept for compat) |

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
├── Input (raw mode: decode keys, route to pane or $mod)
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
- No floating panes, no mouse-driven chrome beyond scroll/click-to-focus, no plugin system, no overview zoom (post-1.0).

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
