# Terminal sizing and image applications

## Pane geometry

GWAE derives cell pixel dimensions from the host terminal's `TIOCGWINSZ`
measurement. It excludes fractional window padding and gives each child PTY
its own drawable pixel width and height, not the whole host window's size.
The correct pane size is set before the child starts. Resizes, fullscreen
changes, newly spawned panes, and PTYs inherited across reload use the same
calculation. Pixel-only font changes are refreshed by the size poll.

When the host does not supply usable pixel metrics, pixels remain zero.
GWAE does not invent a font size. Extreme dimensions are clamped consistently
between the PTY ioctl and escape-sequence replies.

Child terminal queries are parsed as a stream and answered on that child's
PTY, including when the child is not focused:

| Request | Reply |
| --- | --- |
| `CSI 14 t`, text area in pixels | `CSI 4 ; height ; width t` |
| `CSI 16 t`, cell size in pixels | `CSI 6 ; height ; width t` |
| `CSI 18 t`, text area in cells | `CSI 8 ; rows ; columns t` |
| Device attributes and status | Terminal-core replies |
| `CSI 6 n`, cursor position | Actual pane cursor position |

Replies preserve request order across split and coalesced reads. Text inside
OSC, APC, and other control-string payloads is not treated as a size query.
The extra streaming parser handles `CSI 16 t`, which the pinned terminal
core does not implement.

## tdf 0.5

Two missing size paths previously prevented viewing PDFs:

1. Zero PTY pixel dimensions plus no `CSI 14 t` reply left tdf blocked in its
   startup size query.
2. Without `CSI 16 t`, its image picker could choose default 10x20 metrics
   while its PDF renderer used the measured cell size. Converted pages could
   exceed the available width, leaving `Loading...` despite `Rendered: 100%`.

The sizing repair resolves these two failures. It does **not** implement
native-quality Kitty rendering. In Ghostty through GWAE, tdf currently falls
back to text-cell halfblocks. Pages render, but small PDF text is blocky and
can be unreadable. Native Ghostty remains preferable for reading PDFs.

GWAE's existing Kitty passthrough is designed around Unicode-placeholder
virtual placements. tdf uses traditional placements and expects graphics
acknowledgements. Supporting those safely requires pane-scoped image IDs and
lifetime management, cursor-aware placement and clipping, and graphics reply
routing or validated local image storage. Merely forwarding capability probes
or returning unconditional success is not sufficient. No such support is
advertised by this sizing repair.

## Regression checks

```sh
cargo test -p gwae-term
cargo test -p gwae --test pixel_size_e2e
GWAE_E2E_BIN="$PWD/target/release/gwae" cargo test -p gwae --test pixel_size_e2e
```

The real-executable PTY tests verify startup geometry before the first frame,
fragmented and coalesced queries, padded host resize, pixel-only font changes,
fullscreen changes, and independent replies to focused and unfocused panes.
Core tests also replay the tdf image picker's combined probe at every split
boundary, and check cursor reports, unknown metrics, overflow, malformed
queries, and control-string isolation.
