# Input latency

A keystroke you type in strimux is not shown by strimux. It travels to the
program in the pane, and what you see is that program's **echo** coming back.
So every character makes a round trip, and strimux sits on it **twice**:

```
you → macOS → kitty → strimux → pane PTY → agent
                                              ↓
you ← kitty ← strimux ← pane PTY ← ───── echo ┘
```

That is why latency is not one setting but three layers, and why strimux
bothers to look at the other two: tuning only its own knob fixes a third of a
problem.

```sh
strimux tune           # report all three layers
strimux tune --apply   # also write strimux's own fix
```

`strimux tune` prints nothing to fix when there is nothing to fix, and
`strimux doctor` carries a one-line summary.

## What gets changed, and by whom

strimux writes **only its own config file**. Your machine's global settings
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
| kitty | `sync_to_monitor` | `no` | Default `yes` caps drawing at your monitor's refresh. See the note below — under strimux this is safe. |
| strimux | `input_poll_ms` | `1` | How long strimux's loop waits for a keystroke. It is on the round trip twice, so this costs double. |

The macOS values go below what System Settings exposes: its "Fast" slider
stops at `KeyRepeat 2`, and `1` is faster still.

## Why `sync_to_monitor no` is safe here

Turning off vsync normally trades tearing for latency. Under strimux you do
not make that trade, because strimux wraps every repaint in **synchronized
update** markers (`ESC[?2026h` / `ESC[?2026l`). The terminal buffers the whole
frame and applies it atomically, so a frame can never be shown half-drawn
even without vsync. You get the latency win and keep a clean screen.

## Why strimux has `input_poll_ms` at all

strimux's main loop waits for a keystroke with a **timeout**. The wait itself
is not the cost — the cost is being late to notice. Three ways to spend an
idle moment:

| Strategy | Added latency | Idle CPU | How the OS scheduler sees you |
|---|---|---|---|
| Timeout (`input_poll_ms`) | up to that many ms | wakes 1000×/sec at `1` | sleepy, priority kept high |
| Busy spin (no wait) | ~0 in theory | 100% of a core | **CPU hog, priority lowered** |
| Block until ready | ~5µs | ~0 | asleep, priority kept high |

Removing the wait entirely is the tempting-looking option and the worst one.
A spinning process gets demoted by the scheduler, and a demoted process can
be preempted for a full ~10ms quantum — with the pane's echo sitting unread
in the channel the whole time. You would trade a 1ms constant for 10ms of
jitter, and jitter is far more noticeable than a constant offset.

The genuinely better fix is a **blocking** read on stdin (which is what the
pane readers already do), so the loop wakes on an interrupt instead of a
timer. That would remove `input_poll_ms` entirely rather than tune it. Until
then, `1` is the right value.

## Scale check

Roughly, per keystroke round trip:

| Stage | Time |
|---|---|
| USB keyboard polling | ~8ms |
| macOS input stack | ~1-2ms |
| kitty (`input_delay 0`) | ~0-3ms |
| strimux (both directions) | ~2ms at `input_poll_ms = 1` |
| Display refresh @120Hz | ~8ms |

USB polling and display refresh dominate and no software here can change
them. The settings above are worth taking because they are free, not because
any one of them is transformative on its own.
