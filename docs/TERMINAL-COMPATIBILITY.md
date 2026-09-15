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

The sizing repair resolves these two failures. The subsequent pane-local
Kitty subset also lets tdf select its bitmap path instead of text-cell
halfblocks. Actual Yazi/tdf PTY runs verified two sequential PDFs, Kitty help,
fit/fill (`z`), zoom (`o`/`O`), page navigation and responsive return to Yazi.
The captured RGBA uploads contain readable PDF text and reversible zoom crops.
These are real application/protocol observations, not a completed visual
check of the final Ghostty framebuffer. See the [graphics ledger](archive/GWAE-GRAPHICS-VALIDATION.md).

An independent tdf 0.5.0 resize-order bug can leave the old page clipped until
another input event. It reproduces without GWAE: tdf computes layout from
ratatui's cached frame before `draw()` updates its dimensions. The one-line
[upstream patch](patches/tdf-0.5.0-resize-order.patch) calls `autoresize()` before
layout computation. A patched real tdf build passed a no-input shrink test.
GWAE does not mask this with duplicate resize signals. That patch is not
installed over the user's package-managed tdf.

## Partial Kitty graphics support

When host graphics are enabled, GWAE handles a bounded local subset rather
than forwarding child commands or blindly acknowledging capability probes:

- Direct RGB/RGBA transfers (`t=d`, `f=24/32`), validated queries, traditional
  placements/re-display, source crops, dimensions, pixel offsets, cursor policy,
  nonnegative layer ordering and supported local deletion modes.
- Replies are ordered with terminal queries and go only to the requesting
  child. Chunked transfers commit atomically after validation.
- Host textures use a separate process-wide ID namespace shared with legacy
  images. Native placements are rasterized and clipped into virtual tiles.
  Host commands are canonical and quiet. Child grids never contain host IDs.
- Hidden/replaced textures are deleted by owned ID. Retained child sources
  can be uploaded again on reveal. Alternate-screen transitions clear sources.
- Quiet Unicode-placeholder producers (`U=1,q=2`) have a separate compatibility
  path for direct raw pixels and sanitized PNGs. PNG chunks, checksums, exact
  bounded inflation and scanline filters are validated before host upload.
  Sparse placeholder coordinates are resolved before host-ID remapping.

Unsupported features include client file/temp/shared-memory transport, outer
Kitty compression, animation, negative-z placement and general Kitty protocol
compatibility. Traditional PNG transfers are not supported. Nonquiet virtual
starts are rejected rather than silently leaving a caller waiting. No
client-supplied path is opened or passed to the host. The cell-based compositor
replaces underlying text cells, so transparent images do not preserve those
glyphs. A binary reload clears image state and requires child retransmission.

Limits include 32 MiB per native source or pending transfer, 128 MiB retained
native sources per pane, 64 sources and 256 placements per native/legacy store,
and 16,384-pixel axes. Legacy retained command allocations are capped at 128 MiB
per pane, with bounded input/PNG validation scratch and 256-cell virtual axes.
The shared host texture budget is 128 MiB, with 32 MiB per raster tile.

Host support is inferred from terminal environment variables, not negotiated
with the GPU. The host must support Kitty Unicode placeholders. Set
`GWAE_KITTY_GRAPHICS=0` to disable the graphics path, or `=1` to opt in on a
known-capable host when environment detection is unavailable. In particular,
WezTerm must have its Kitty graphics support enabled. Disabled mode never
fabricates a graphics success reply.

## Regression checks

```sh
cargo test -p gwae-term
cargo test -p gwae --bin gwae graphics
cargo test -p gwae --test pixel_size_e2e
GWAE_E2E_BIN="$PWD/target/release/gwae" cargo test -p gwae --test pixel_size_e2e
```

The real-executable PTY tests verify startup geometry before the first frame,
fragmented and coalesced queries, padded host resize, pixel-only font changes,
fullscreen changes, and independent replies to focused and unfocused panes.
Core tests also replay the tdf image picker's combined probe at every split
boundary, and check cursor reports, unknown metrics, overflow, malformed
queries, and control-string isolation.

The executable graphics cases additionally check truthful probe/errors,
disabled mode, canonical quiet host uploads, two panes reusing child image ID
1, sibling-safe deletion, and rejected external media even when a child emits
visible virtual placeholders.

See [the acceptance map](archive/GWAE-ACCEPTANCE-MAP.md) for requirement-level and
changed-output observations, including known failures and blocked checks.
The [graphics ledger](archive/GWAE-GRAPHICS-VALIDATION.md) records actual application
evidence, and the [historical sizing ledger](archive/GWAE-TDF-VALIDATION.md) preserves
the pre-fix comparison. Aggregate test counts are not the acceptance evidence.
