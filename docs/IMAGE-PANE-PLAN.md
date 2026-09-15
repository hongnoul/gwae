# First-Class Image Pane Plan

Status: draft. Goal: stop routing raster through the text-grid path.

## Problem

Today every pane is a `PtyPane` (`crates/gwae/src/tui/pty.rs:124`): a VT grid
plus a graphics sidecar (`graphics.rs` native `t=d f=24/32`, `graphics_legacy.rs`
`U=1,q=2` placeholders). The single render thread (`tui/render.rs:261`
`render_frame_with_images`) repaints every pane into one cell buffer and pushes
tile uploads through one `Host::pending` queue (`tui/mod.rs:1715-1778`).

A tdf page (e.g. 560x736 RGBA, larger fullscreen) therefore costs multi-MB
APC chunk decode, bilinear rescale, tile split, zlib, and reupload per redraw.
`archive/GWAE-GRAPHICS-VALIDATION.md` states it directly: one pane's host output volume
is the entire frame budget. `o=z` shrank bytes (855KB to 81KB startup, ~818KB
to ~44KB per page turn) but kept the coupling. One image pane stalls all text
panes.

## Decision

Add `PaneKind::Image` as a first-class pane alongside `PaneKind::Text`.

- Text pane: today's PTY grid. Zero terminal dependence. Unchanged.
- Image pane: a texture loader, not a terminal emulator. Owns a page texture,
  zoom, page index, and scroll. The composer treats it as a pixel rect, not
  cells with placeholders.

One pane kind, three producers:

| Producer | Input | Path |
|---|---|---|
| tdf native | child `t=d f=24/32` APC | promote PTY pane in place, keep PTY for input keys |
| jcode / legacy | child `U=1,q=2` placeholders, PNG/raw | same pane kind, smaller textures, same cache |
| native open | `gwae open doc.pdf` / image file | no child TUI at all, decode directly to texture |

PDF pages, raster images, and tdf output share one cache key, one zoom/page
control set, and one composer rect.

## Why this fixes latency

- Content-hash texture cache: idle image pane reuses its host texture id,
  emits zero bytes, skips raster/rescale/zlib entirely.
- Dirty tracking per pane kind: text frames take the fast cell-diff path and
  never walk image tiles unless the image pane itself changed.
- One quiet upload per page turn, not per frame. Page turns still cost one
  upload; typing in соседние text panes no longer waits behind it.
- Optional follow-up: move decode/rescale/zlib off the render thread into a
  small worker pool; the render thread only swaps in finished textures.

## Design

### Pane model (minimal refactor)

`archive/TUI-SPLIT-PLAN.md` says leave `graphics_*.rs` alone during the split. So do
not rewrite `PtyPane` on day one. Instead:

```rust
enum PaneKind {
    Text(PtyPane),      // today's path, untouched
    Image(ImagePane),   // new
}
struct ImagePane {
    id: PaneId,
    source: ImageSource, // TdfBacked { pty, child } | Native { path, page }
    texture: TextureKey, // content hash + size + zoom + page
    page: u32, zoom: Zoom, scroll: (i32, i32),
    last_upload: Option<HostTextureId>,
}
```

Phase 1 can ship as `PtyPane + Option<ImageView>` if the enum refactor is too
big mid-split; Phase 2 promotes to the enum once `tui/app.rs` lands.

### Promotion heuristic (tdf just works)

Keep installed tdf working with zero user change:

1. Monitor per-pane graphics commit rate in `feed_pane_output`.
2. When a pane emits sustained native image commits (e.g. >= N full-page
   commits in M seconds, or a single commit above a pixel threshold), promote
   it to `ImagePane { TdfBacked }`.
3. The PTY stays alive underneath for input (`q`, `z`, `o`, page keys). Grid
   output for that pane stops being painted as cells; only the texture rect
   is composited.
4. Demote back to text if the child exits image mode or the pane spawns a new
   foreground program.

Same rule covers jcode legacy image bursts, with a smaller size threshold.

### Rendering

- Composer clips the image rect to the pane rect (reuse today's tile-clip
  math in `graphics_host.rs:Host::prepare`, but keyed by texture hash).
- Still page: zero host bytes, zero reencode. `refresh_due` only fires on
  texture change, not on every frame.
- Host gate stays: image panes require Kitty graphics on the host
  (`host_supports_kitty_graphics`). Non-Kitty hosts get a text fallback pane
  ("image unavailable on this terminal") or halfblocks, never garbage APC.
- This keeps gwae running inside any terminal. The host stays dumb; the pane
  gets honest. No per-terminal pane code.

### Input

- Tdf-backed image pane: forward keys to the child PTY unchanged. No VT
  semantics needed on our side beyond what the child already parses.
- Native image pane: handle `j/k`/arrows (scroll), `+/-` (zoom),
  `n/p` or `l/h` (page), `q` (close), with the same keymap infra as text
  panes so bindings stay consistent.

### Native commands

- `gwae open doc.pdf [page]` and image paths decode directly (PDF via
  existing tdf-equivalent renderer or a small `poppler`/`pdfium` helper;
  decide dependency in Phase 2).
- Same `ImagePane`, producer = file instead of child. Enables reload
  persistence below without a child retransmit.

### Reload

Today reload preserves PTY geometry but drops image maps; children must
retransmit. With this plan:

- Native image panes persist trivially: path + page + zoom re-decode on
  adopt. No retransmit needed.
- Tdf-backed panes still need one child redraw after exec, but only one page,
  not a full tile storm, because the texture cache key survives in the
  handover file.

### Config

- `image_pane = "auto" | "off"`: `auto` promotes on heuristic, `off` keeps
  today's behavior for minimalists.
- Keep `GWAE_KITTY_GRAPHICS=1/0` as the host capability override.

## Phases

1. **Isolate (1-2 days).** Content-hash texture cache + per-pane dirty bit.
   Idle tdf pane emits nothing. Measure: typing RTT in a text pane with a
   tdf pane open, bytes/frame, render ms. No pane-model change.
2. **Promote (2-3 days).** `ImageView` state on `PtyPane`, promotion
   heuristic, skip cell painting for promoted panes, input passthrough.
   tdf works unmodified. Fallback message on non-Kitty hosts.
3. **Native open (3-5 days).** `gwae open` for PDF/PNG/JPEG, direct decode,
   same viewer controls. Reload persistence for native panes.
4. **Thread (optional).** Background decode/rescale/zlib workers. Only if
   Phase 1 metrics still show page-turn stalls.

## Acceptance

- Text-pane keystroke echo RTT with an idle tdf pane open matches no-tdf
  baseline (today it does not).
- Still image pane: 0 host image bytes per frame.
- Page turn: exactly one quiet texture upload, bounded (today's `o=z`
  budget or better).
- Installed tdf unmodified: open, fit/fill, zoom, page nav, quit all work.
- Reload with a native PDF pane: page returns without child retransmit.
- Non-Kitty host: clean fallback, no APC garbage.

## Non-goals

- No host-specific pane backends (Kitty/WezTerm/Ghostty remote control).
  That surrenders layout ownership and SSH support for one workload.
- No custom terminal. Kitty stays the recommended thin host; image panes
  only need its reference graphics renderer, not its multiplexer features.
- No transparent-glyph compositing. Image rects still cover cells; the lie
  being removed is pretending raster is text, not the compositor itself.

## Open questions

- PDF decode dependency for native open: shell to `pdftoppm`, link
  `poppler`, or reuse tdf as decoder initially?
- Promotion thresholds: tune from real tdf traces (page pixel sizes above).
- Enum refactor timing vs `archive/TUI-SPLIT-PLAN.md` step 14 (`tui/app.rs`).
