# Architecture

gwae is a **single-process, daemon-free** multiplexer (ADR-003 reversed,
ADR-011). One `gwae` process owns every PTY, hosts every pane's grid, and
composes the whole screen into a single 2D cell buffer. There is no client-
server protocol, no socket, no attach/detach.

```
gwae (one process)
├── Layout core (rows / strips / columns / panes) - pure, no I/O
├── Pane tasks (one per PTY: read bytes -> parse -> update grid)
├── Composer (coalesce damage -> single 2D cell buffer)
├── Render (diff buffer -> batched ANSI -> terminal)
├── Input (raw mode: decode keys, forward to focused pane or handle $mod)
└── OSC 133 trackers (per-pane status: running/idle/done -> minimap + smart-jump)
```

## Crate map

| Crate | Kind | Responsibility |
|---|---|---|
| `gwae` | bin | the whole TUI: raw mode, input decode, PTY hosting, composer, render loop, launcher, minimap, OSC 133 |
| `gwae-layout` | lib | **the pure core**. 2D grid of strips (rows/columns/panes) + verbs + scroll math. No I/O, no async, no PTY. Property-tested. |
| `gwae-term` | lib | emulator facade behind a `TermGrid` trait (ADR-004), damage tracking |
| `gwae-testkit` | lib | fake PTYs, scripted terminals, snapshot harness |

`gwae-layout` depends only on `std` + `serde`. `gwae-term` isolates the
emulator-crate choice behind `TermGrid`; swapping `alacritty_terminal` <-> `wezterm-term`
touches one crate.

## Rendering pipeline

One 2D cell buffer holds everything: every visible pane's grid region plus
gwae's own chrome (centered HUD/minimap, launcher, focus outline). Damage from
any pane merges into this one buffer; the render diffs the whole buffer to the
terminal with synchronized-update markers (`ESC[?2026h/l`).

Horizontal scroll of a strip is a **full repaint** (terminals can't blit
horizontally), so frames must be fast: budget < 4ms for a 300x80 viewport.
Scroll animation is optional and gated on sync-update support and frame budget.

## Pane sizing

Panes have a **logical size** (cols x rows) set by their column width and strip
height. This is the size reported to the PTY (`TIOCSWINSZ`), so full-screen apps
lay out at logical size. The viewport crops, never resizes: a column wider than
the viewport is panned, and app inside is unaffected.

## Crash / persistence (deliberately thin)

When the process exits, panes and their processes end; gwae persists **no
session state** (ADR-015). "Panes and their processes end" is a hard guarantee,
not a hope, and it holds for *every* way gwae can exit. See
[Process teardown](#process-teardown). Resume is the harness's job (`claude --resume`,
`jcode --resume`). A panic in one pane's emulator must not take the TUI down
(one task per pane, supervisor pattern).

## Process teardown

A multiplexer that leaks background processes is worse than useless: the work
keeps burning CPU with no window left to find it in. So every exit path funnels
into one reaper (`crates/gwae/src/reap.rs`), and every pane's root pid is
registered the moment it is spawned.

| How gwae leaves | What runs |
| --- | --- |
| `⌥+⇧+q` (force quit), last pane closed, `⌥+q` | `kill_pane_tree` per pane, then a final `reap_all` sweep |
| `SIGTERM`, `SIGHUP`, `SIGINT`, `SIGQUIT` | signal handler wakes the reaper thread, waits (≤2s), then re-raises so the parent sees `128+signo` |
| panic (unwind or abort) | panic hook reaps, then defers to the previous hook |
| early `return` out of `run_tui` | `reap::Guard` drop guard |
| `SIGKILL` | uncatchable; the PTY hangup takes well-behaved children only |

Three kills are used per pane, because each catches processes the others miss:

1. **`killpg` on the pane's group** — the pane's shell and its foreground job.
2. **`kill` on the root pid** — the shell itself.
3. **A `ps` tree walk, deepest-first** — everything that left the group on
   purpose (`nohup cmd &`, `setsid`), *and* everything an interactive shell put
   in its own group via job control. This is the case a group kill alone
   silently misses, which is why the signal path cannot live in the handler.

Signal handlers may not allocate, lock, or fork, so the handler does only
signal-safe work (atomic stores, `write(2)`) and hands the deep sweep to a
thread parked on a pipe since startup. The handler waits on that thread with a
2s bound: a mux that refuses to die when told to is worse than one that leaks.

The handler also restores the terminal by hand (leave alt screen, show cursor,
pop kitty flags, mouse off, autowrap on) because `crossterm` is not
signal-safe, and dying in the alt screen strands the user in a black rectangle.

`crates/gwae/tests/teardown_e2e.rs` drives a real gwae over a real PTY and
asserts against the actual process table for each of these paths.

## Staying current

gwae updates itself **the way it was installed, or not at all** (ADR-016).
`crates/gwae/src/update.rs` detects the install source (config, then the
installer's receipt, then the binary's path), maps it to a route, and runs only
the routes gwae owns (`install.sh`, `brew`, `cargo`). Nix store paths, distro
packages, and checkouts get their package manager's command printed instead:
overwriting a file another package manager tracks leaves that manager
describing a machine that no longer exists.

The daily check is a bare `HEAD` of the `releases/latest` redirect (not
`api.github.com`, whose 60/hr per-IP limit is shared across a NAT), carries
nothing about the user, and only ever produces a one-line notice. Full
reasoning: [`UPDATES.md`](UPDATES.md).

## Terminal requirements

Minimum 256-color + standard cursor addressing; wants truecolor, synchronized
updates, kitty keyboard protocol, mouse SGR (passed through). `⌥` (the Option
key on macOS, Alt elsewhere) is the universal `$mod` (ADR-014); macOS may add an
optional `Cmd+hjkl` snippet via `gwae setup`.

## Open questions

- ADR-004: which emulator crate (`alacritty_terminal` vs `wezterm-term`) wins
  the `TermGrid` trait. Decide in the M0 spike by prototyping both.
