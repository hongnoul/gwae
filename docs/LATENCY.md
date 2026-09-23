# Input latency

A keystroke you type in gwae is not shown by gwae. It travels to the
program in the pane, and what you see is that program's **echo** coming back.
So every character makes a round trip, and gwae sits on it **twice**:

```
you → macOS → kitty → gwae → pane PTY → agent
                                              ↓
you ← kitty ← gwae ← pane PTY ← ───── echo ┘
```

That is why latency is not one setting but three layers, and why gwae
bothers to look at the other two: tuning only its own knob fixes a third of a
problem.

```sh
gwae setup --only latency --print   # report all three layers
gwae setup --only latency --yes     # also write gwae's own fix
```

`setup --only latency` prints nothing to fix when there is nothing to fix, and
`gwae doctor` carries a one-line summary.

## What gets changed, and by whom

gwae writes **only its own config file**. Your machine's global settings
and your terminal's config are printed as exact commands for you to run.
Silently editing another program's config, or a machine-wide preference that
affects every app you use, is not something a multiplexer should do.

| Layer | Setting | Want | Why |
|---|---|---|---|
| macOS | `KeyRepeat` | `1` | Repeat rate for a held key. Stock is `6` (~90ms/char); `1` is ~15ms/char. This is the single biggest win for held-key delete, and no terminal setting can compensate for the OS not sending the keys. |
| macOS | `InitialKeyRepeat` | `10` | Delay before a held key starts repeating. Stock `25` is ~375ms of nothing happening. |
| macOS | `ApplePressAndHoldEnabled` | `0` | When on, holding a key opens the accent-picker popup instead of repeating at all. |
| kitty | `input_delay` | `0` | kitty's own wait before processing what a program printed — i.e. exactly the echo you are waiting to see. Default `3`. |
| kitty | `repaint_delay` | `1` | Minimum gap between screen updates. Default `10` caps you at ~100 FPS. |
| kitty | `sync_to_monitor` | `no` | Default `yes` caps drawing at your monitor's refresh. See the note below — under gwae this is safe. |
| gwae | — | — | Nothing to tune. The loop blocks on a channel that a keystroke or a pane byte wakes immediately; there is no poll interval on the input path. |

The macOS values go below what System Settings exposes: its "Fast" slider
stops at `KeyRepeat 2`, and `1` is faster still.

## Why `sync_to_monitor no` is safe here

Turning off vsync normally trades tearing for latency. Under gwae you do
not make that trade, because gwae wraps every repaint in **synchronized
update** markers (`ESC[?2026h` / `ESC[?2026l`). The terminal buffers the whole
frame and applies it atomically, so a frame can never be shown half-drawn
even without vsync. You get the latency win and keep a clean screen.

## Why gwae has no latency knob of its own

gwae's main loop used to wait for keystrokes with a timeout
(`input_poll_ms`), draining pane output between polls — so every echoed
byte paid up to a poll tick of queue latency. Three ways to spend an idle
moment:

| Strategy | Added latency | Idle CPU | How the OS scheduler sees you |
|---|---|---|---|
| Timeout (the old `input_poll_ms`) | up to that many ms | wakes 1000×/sec at `1` | sleepy, priority kept high |
| Busy spin (no wait) | ~0 in theory | 100% of a core | **CPU hog, priority lowered** |
| Block until ready | ~5µs | ~0 | asleep, priority kept high |

gwae now takes the third row: a dedicated thread blocks in the terminal
event read and forwards into the same channel the pane readers use, and the
main loop's single blocking wait wakes on an interrupt for a keystroke or a
pane byte alike. Measured in a real-PTY harness this cut echo p50 from
~2.5ms to ~0.34ms — faster than mux designs that put a server process on
the round trip, because in-process there is nothing between the keyboard
and the PTY.

`input_poll_ms` still parses for config compatibility, but it only paces
periodic housekeeping (terminal-size re-check, note expiry, status flips).
`gwae doctor` no longer reports it as a latency setting at any value.

## Scale check

Roughly, per keystroke round trip:

| Stage | Time |
|---|---|
| USB keyboard polling | ~8ms |
| macOS input stack | ~1-2ms |
| kitty (`input_delay 0`) | ~0-3ms |
| gwae (both directions) | ~0.3ms (event-driven wake) |
| Display refresh @120Hz | ~8ms |

USB polling and display refresh dominate and no software here can change
them. The settings above are worth taking because they are free, not because
any one of them is transformative on its own.
