# Pane-local graphics validation

Validated on macOS ARM64 on 2026-09-12 with real Yazi 26.8.15 and tdf 0.5.0.
This follows the [historical sizing repair](GWAE-TDF-VALIDATION.md).
The [acceptance map](GWAE-ACCEPTANCE-MAP.md) ties ten outcome requirements and
twenty changed public-output boundaries to named checks and observed results,
including the still-failing installed resize and blocked Ghostty capture.

## Result and boundary

The new path accepts validated direct RGB/RGBA transfers and traditional
placements, answers the requesting pane, and renders clipped images using
owned host texture IDs. Actual tdf now selects Kitty mode. Its Yazi opener,
fit/fill, zoom and page navigation passed against the release candidate and
then again against the installed executable.
Decoded **actual host-upload pixels** show readable PDF text, not halfblocks.

This is not full Kitty compatibility. Final Ghostty framebuffer visual
acceptance is still incomplete: background capture of the test window on
another Space failed, including ScreenCaptureKit error -3811. The user's
active app was not brought behind the test window to force a screenshot.
Protocol observations and decoded textures are not represented as a GPU
screenshot or complete native-terminal parity.

The independent tdf resize bug is diagnosed and patched in a local build of
upstream tdf, but the package-managed executable has not been replaced.

## Requirement-to-check mapping

Names below are actual Cargo tests unless identified as an application replay.
The graphics unit/integration subset passed 71 tests. Counts supplement the
individual observations below, not replace them.

| Requirement | Check and observed result |
| --- | --- |
| Validated capability, not unconditional success | `tui::tests::graphics_feed_tests::exact_tdf_probe_is_acknowledged_only_when_graphics_are_enabled` passes every split. `graphics_enabled_real_child_probe_errors_and_quiet_canonical_upload` executes the real binary and observes direct query `i=31;OK`, shared-memory/compression `i=32/33;ENOTSUP`, then exact CSI16. The outer PTY never supplies graphics replies. |
| Disabled graphics must not claim support | `graphics_disabled_real_child_probe_has_no_false_ok_or_host_forwarding` observes only CSI16, no graphics OK or host packet. |
| Cursor/reply ordering across arbitrary reads | `mixed_text_and_apcs_use_each_command_cursor_at_every_split`, `final_image_chunk_uses_final_cursor_and_only_then_acknowledges`, `graphics_cursor_advance_precedes_following_text_and_cursor_query` and `graphics_and_terminal_query_replies_keep_exact_stream_order` pass with the actual pane feed and recording child writer. |
| Bounded parser and cancellation | `graphics_stream` tests cover every split, UTF-8, opaque strings, CAN/SUB, malformed ESC, exactly 64 KiB, one byte over, and a multi-MiB unterminated attack. `discarded_oversized_continuation_cannot_commit_a_partial_image` proves dropped oversized data cannot later produce an image/OK while prior images and terminal replies survive. |
| Atomic images and bounded resources | `graphics::tests::image_and_total_budgets_count_limits_and_atomic_failed_replacement`, `placement_limits_upserts_and_retransmission_remove_old_placements` and chunk/base64 tests reject invalid lengths, sizes and failed replacements. `graphics_legacy::tests::image_placement_and_allocated_source_budgets_are_enforced` bounds retained command allocations and counts. |
| Native and legacy commands share the child ID namespace | `mixed_mode_child_ids_switch_only_after_successful_source_commit` and `dispatcher_replaces_native_source_only_after_successful_legacy_commit` preserve an existing native source during a pending legacy transfer, invalidate it only on successful commit, and prevent the replaced legacy source from being uploaded after a new native commit. Queries and rejected transfers do not replace sources. |
| Two panes may both use child ID 1 | `graphics_two_real_panes_own_same_client_id_and_survive_sibling_deletion` observes separate 312x448 and 232x448 pane replies, distinct owned red/green host IDs, only the red ID deleted, and a surviving green placeholder/redisplay. `overlapping_child_ids_are_isolated_and_only_owned_ids_are_deleted` also checks exact cleanup bytes. |
| No external-media escape through virtual placeholders | `graphics_legacy_external_media_cannot_escape_via_visible_placeholders` sends real child `U=1` shared-memory/file packets plus visible placeholders. Both receive ENOTSUP, with zero host uploads/placeholders and no forwarded path. Legacy unit cases also reject temp files, compression and unknown controls. |
| PNG compatibility is bounded before reaching the host | `png_is_validated_sanitized_and_transmitted_canonically`, `png_huge_headers_crc_critical_chunks_bad_order_and_truncation_rejected`, `png_actual_inflation_filter_bytes_checksum_and_trailing_streams_are_bounded`, and `png_palette_transparency_16bit_and_adam7_are_supported` check exact inflation length, full compressed-input consumption, filters, checksums, supported formats and sanitized output. |
| Crop, scale and pane clipping | `crop_scale_and_cell_padding_are_applied_locally`, `bilinear_rgb_scaling_has_exact_endpoints_and_opaque_alpha`, `clipped_tiles_emit_explicit_coordinates_and_never_touch_outside_cells`, and `wide_scenes_split_before_diacritic_index_overflow` pass. The native-overlay frame test verifies style normalization and suppresses unmapped placeholders without a host. |
| Defined overlapping-image order | `equal_depth_orders_by_child_image_id_not_transmission_order` transmits ID 2 before ID 1 and verifies ID 2 is still the top equal-depth layer. Negative-z placements are rejected, not claimed supported. |
| Hide/reveal, refresh and replacement | `hidden_images_release_host_storage_and_reveal_reuploads_local_source`, `eviction_refresh_reuploads_without_changing_placeholder_ids`, `stale_budget_is_reclaimed_before_replacement_admission`, `native_and_legacy_share_one_live_host_budget` and `virtual_placement_revision_and_footprint_changes_upload_immediately` verify source retention, scoped host deletion, immediate updates and shared storage accounting. These are component checks, not a final multi-pane GUI screenshot. |
| Alternate screens and sparse placeholders | `alternate_screen_enter_and_leave_in_one_text_event_clear_graphics` checks all 47/1047/1049 modes including pending transfers. `sparse_marks_inherit_row_column_high_byte_and_survive_clipping` checks inherited coordinates and explicit remapped high bytes. |
| Host traffic must be canonical and quiet | Real-executable captures and `canonical_upload_chunks_are_bounded_quiet_and_direct` check only owned, quiet uploads/deletes. No child capability probe is forwarded to the host. |

Executable graphics cases are in `crates/gwae/tests/pixel_size_e2e.rs`, alongside
the five retained startup/resize/pixel-only/new-pane/reload geometry checks.

## Actual Yazi and tdf replay

The replay uses a real controlling PTY, the actual release GWAE executable,
the installed Yazi opener, and the installed tdf, with two real PDF paths
containing spaces and a Unicode character. No application or child reply is
stubbed. The outer PTY supplies measured 160x40 cells / 2560x1280 pixels and
explicitly enables graphics, but does not emulate a GPU.

Observed in the successful replay:

1. Yazi selected both paths using its normal selection keys and Enter opener.
2. The first tdf reached `Rendered: 100%` and emitted a complete native RGBA
   page texture of 560x736 pixels. Decoding the exact canonical upload and
   inspecting the resulting PNG showed readable text.
3. `?` displayed `When using Kitty` and `Zoom in and`, with no `Not using kitty`
   warning. Dismissing help restored the identical page-pixel hash.
4. `z` selected fill mode, producing a 560x960 texture. `o` changed its pixel
   hash and visibly enlarged the crop. `O` restored the exact earlier fill
   hash, and `z` restored the exact initial fit hash.
5. `l` produced a different page image. `h` restored the initial page's exact
   hash, verifying reversible navigation rather than just a status message.
6. Quitting the first viewer started a different tdf PID for the second path,
   also reaching 100% and emitting a complete native texture.
7. Quitting the second viewer left no tdf process. Yazi redrew `NOR` with both
   filenames, and `k` produced a fresh first-file selection and `1/2` status.

The first native replay timed out because the old halfblock harness performed
an expensive process scan after every small PTY read, throttling multi-MiB
image traffic. The harness was corrected to drain available bytes before
checking process state. No workflow predicate was weakened. The corrected
complete native replay passed in 7.9 seconds.

Private local evidence is under `$JCODE_SCRATCH_DIR/gwae-native-graphics-20260912`:

- `pty-workflow.py`, `pty-workflow.log`, `pty-workflow-results.json`.
- Per-step `.ansi` captures, `native-fit.png`, `native-fill.png`,
  `native-zoom-in.png`, `native-zoom-out.png`, and page/restoration PNGs.
- `host.ansi` from the earlier dedicated Ghostty candidate window also records
  real tdf selecting the native path, but is not a final-build GPU screenshot.
- `workspace-serial.log` for the final full workspace run.

PDFs, screenshots and application captures are not committed.

## Two real viewers: lifecycle follow-up

The first delivery checked these lifecycle properties with component tests and
synthetic red/green child programs. A subsequent **actual two-tdf replay** now
passes as well, twice in about 8.8 seconds, against the same installed hash.
It uses two unmodified tdf 0.5.0 processes, one-page display (`-m 1`) and
different page colors to distinguish the panes. Both view the same real PDF.
Actual child wire captures confirm that **both transmit image ID 1**.

The host PTY is 160x60 cells with 8x16-pixel cells. A test-only PTY recorder
captures each real viewer's output and mirrors the **actual GWAE-provided
ioctl** to its nested PTY on SIGWINCH. Every recorded outer/inner tuple matches:
39x58 cells / 312x928 pixels, then 159x58 / 1272x928 for the fullscreen pane,
then 39x58 / 312x928 again. It never invents child query replies or image data.
Host frames are replayed through the project's tested `gwae-term` grid, not a
GPU emulator. This remains explicitly separate from Ghostty visual acceptance.

Observed checks, using zero-based host cell coordinates:

| Step | Observed result |
| --- | --- |
| Dismiss initial GWAE help | The intentional startup HUD initially masks part of one image. A harmless key removes it, revealing two complete 35x23-cell rectangles, 280x368 RGBA pixels each. |
| Tiled panes | Image bounds are x3..37 and x43..77, y19..41. Every placeholder resolves to an owned upload and valid row/column/placement coordinates. The distinct page colors have distinct hashes and host IDs. |
| Fullscreen first viewer | The sibling's host texture is deleted. After a harmless layout tick, the first page is 624x800 pixels at x42..119, y5..54. The extra tick is needed by unpatched tdf and is not counted as immediate resize success. |
| Return to quarter width, no child input | The old large page is clipped to one cell at x39, 8x800 pixels, never into its neighbor. The sibling is revealed at its original bounds/hash with a new owned host ID. Its child transmit count stays at one, proving local source reuse rather than child retransmission. The first viewer's immediate fit still fails. |
| Subsequent harmless key | The first page returns to its original 280x368 texture and exact original hash. Both complete image rectangles are inside their respective panes. |
| Navigate only the second viewer | Its actual source ID changes to page 2, and its pixels change. The first viewer keeps the same host ID/hash. Returning to page 1 restores the second viewer's original hash without retransmitting that retained page source. |
| First viewer exits normally | Only its texture disappears. The same sibling tdf PID survives, with the same texture ID/hash, relocated correctly into the vacated left column. |
| Second viewer exits normally | Both recorded tdf PIDs are gone, GWAE exits with status zero, all owned host textures are deleted, and the final decoded frame contains no image placeholders. Every observed delete addresses a previously owned ID. |

The initial harness attempts exposed two measurement issues, not product fixes:
the startup HUD legitimately occludes image cells, and macOS `/usr/bin/script`
did not propagate resized nested-PTY dimensions here. The recorder replaced
that unsuitable capture layer and logs ioctl parity. The strict complete-image
assertion remains in force after dismissing the startup HUD.

Private evidence under `gwae-native-graphics-20260912/lifecycle` includes
`real-viewers.py`, `record-pty.py`, `decode-host.rs`, and successful directories
`run-1789179285580928000` and `run-1789179401883849000`. Each contains per-step
host `.ansi`/decoded-text captures, actual child `.ansi` captures, process and
geometry logs, owned-ID events, `results.json`, and a `PASS` marker. No user
config, package-managed binary or PDF was changed by these follow-up checks.

## Independent resize hypothesis

The pre-draw layout lag reproduces directly in tdf without GWAE, and in a
minimal program using the same ratatui ordering. Before the correction the
minimal trace reported `resize pre=100x20 draw=40x20`. Calling `autoresize()`
before reading `get_frame()` changed it to `resize pre=40x20 draw=40x20`.

The [tdf 0.5.0 patch](patches/tdf-0.5.0-resize-order.patch) applies cleanly,
typechecks and builds with stable Rust. A real patched tdf no-input shrink
from 160x80 to 40x80 changed the page from a 112x71-cell image to a correctly
fitted 36x24-cell image. A subsequent ordinary key did not change that fit.
Evidence is in `$JCODE_SCRATCH_DIR/tdf-resize-lag-20260912/PATCH-VALIDATION.md`
and `patched-real-tdf-resize-results.json`.

This validates the responsible fix without adding duplicate SIGWINCH delivery
to GWAE. The patched tdf is scratch-only, not installed or submitted upstream.

### Combined native path: unpatched versus patched tdf

A further independent replay runs each **real tdf executable directly under
the installed GWAE**, with no nested recorder or `/usr/bin/script`. The host
PTY is 160x60 cells / 1280x960 pixels, and each run has an isolated HOME/XDG.
The same actual PDF, native graphics path, quarter/fullscreen bindings and
harmless `x` follow-up key are used for both binaries.

| Transition | Package-managed tdf 0.5.0 | Local patched tdf 0.5.0 |
| --- | --- | --- |
| Initial quarter-width pane | 280x368 pixels, 35x23 image cells | Identical dimensions and pixel hash |
| Fullscreen, no further child input | Still the old 280x368 page | Correct 624x800 page, 78x50 cells |
| Subsequent `x` | Finally changes to 624x800 | No output or image change |
| Quarter width, no further child input | Old page clipped to 8x800 at x39, one image column | Correct 280x368 fit and original pixel hash |
| Subsequent `x` | Finally restores the original 280x368 page | No output or image change |

The fitted image rectangles are exactly x3..37/y19..41 in quarter width and
x42..119/y5..54 fullscreen. Both executables produce identical fitted pixel
hashes. This separates GWAE's successful clipping of stale child placements
from tdf's failed immediate relayout. The patched path fixes both enlargement
and shrink without a compensating key or duplicate resize notification.

The initial recording harness collected observations but did not fail on all
acceptance conditions. A separate **fail-fast validator** therefore reparses
every captured graphics packet independently, decodes the actual host frame,
and checks exact RGBA lengths/hashes, complete per-cell placeholder coordinates,
quiet canonical uploads, scoped deletes, unchanged executable hashes, actual
child command/PID continuity, no timeouts, and normal exit. Both recorded cases
pass these assertions. A second complete comparison passes in 15.4 seconds,
including a final raw capture and grid replay after `q`: no live host textures,
no image placeholders and no descendant processes remain. This is actual
application wire/grid acceptance, not a Ghostty GPU screenshot.

Private evidence is under
`$JCODE_SCRATCH_DIR/tdf-native-resize-verify-20260912T021356Z`, including
`native_tdf_resize_compare.py`, `validate_results.py`, `all-results.json`,
`independent-validation.json`, and per-stage `.ansi`/decoded-text captures.
The repeated run and its final cleanup captures are in `repeat-with-cleanup/`.
The package-managed tdf SHA-256 remains
`e00642315b768c3f913e8893dc37afef268a2f625ac8d5dbac06246f75a95aac`.
The scratch patched binary is
`efcb33c9200f21823b491d522f5c19e1c48af909f3d62ff33cebe93549e00290`.
**Installed tdf still has the resize lag.** No package override or Yazi opener
change was made to substitute the patched executable.

### Confidence recheck

A subsequent audit rechecked the three application-level claims rather than
relying on completion labels. All nine saved workflow PNGs decode to the exact
RGBA bytes in their actual host uploads. Reversible hashes, distinct sequential
tdf PIDs, Kitty help and the same responsive Yazi process all agree with the
recorded assertions. Both two-viewer runs also pass an independent full-buffer
host-packet parse and per-cell frame replay, including actual child image IDs,
recorder ioctl parity, source reuse, sibling process survival and final cleanup.
The scratch audit is `gwae-native-graphics-20260912/confidence-audit.py`.

The audit found that the old minimal ratatui JSON filename had been overwritten
by its fixed run. It was therefore not usable as preserved paired evidence.
Separate unpatched/patched binaries were built with identical dependencies and
only the `autoresize()` call differing. A fresh paired PTY run asserts
`resize pre=100x20 draw=40x20` without the call and
`resize pre=40x20 draw=40x20` with it. Distinct captures and binary hashes are
preserved in `tdf-resize-lag-20260912/confidence-paired-ratatui/paired-results.json`.
This minimal check supplements, not replaces, the actual tdf comparisons above.
The committed patch also reconstructs the built tdf source byte-for-byte from
the pristine archive. All nine source/manifest files were compared, and only
`src/main.rs` differs. No GPU acceptance or installed-tdf fix is inferred.

The final traceability pass added direct project regressions for SGR58 color
and reset mapping, absolute/clamped graphics cursor behavior with pending wrap,
alternate-screen/RIS epoch counting, and RIS invalidation of visible/pending
graphics. All four pass in the real Cargo targets: 46 terminal tests and 72
graphics tests. These additions change tests only, not installed runtime code.
The acceptance map's 67 distinct named test references were mechanically
matched to individual observed passing results, not inferred from totals.

A fresh installed Yazi/tdf workflow also passed again in 7.8 seconds, under
`gwae-native-graphics-20260912/acceptance-final-20260912T0226`. An independent
audit verified its exact upload-to-PNG bytes, reversible hashes, sequential
real child PIDs and return to the same responsive Yazi process. This exercises
the actual opener and viewer, not a substitute child. Its outer PTY still has
no GPU, so the final visual acceptance constraint remains unchanged.

## Reproduction

### Delivered artifact

Implementation commit: `39efea8`. The signed release artifact was atomically
installed at `~/.bun/bin/gwae` after the complete serial workspace passed
**668 tests, zero failures, seven guarded helpers ignored**, across 29 targets.
The release and installed files are byte-identical, SHA-256:

```text
90a7a810a90b72885dae643c9718a8728eb753764dbaaea7165f408e945f979d
```

All **nine** actual-executable pixel/graphics cases then passed against the
installed path, with one guarded helper ignored (`installed-e2e.log`). The
complete real Yazi/tdf native workflow passed again in 7.8 seconds. Its result
JSON records the installed path and this exact executable hash.

The previous installed sizing-only executable is preserved locally as
`gwae-native-graphics-20260912/gwae-before-native-39efea8` under the scratch
directory, with its original SHA-256
`cde69bc9e6f2d6cb833e02585ea4d8e6a1575743b29d09939abb6ffd783d7e5f`.
No package-managed tdf binary or user PDF was changed. These delivery checks
do not close the final Ghostty framebuffer or installed-tdf resize limitations.

### Commands

```sh
cargo test -p gwae --bin gwae graphics
cargo test -p gwae-term
cargo test -p gwae --test pixel_size_e2e
cargo test --workspace -- --test-threads=1
cargo build --release -p gwae --bin gwae
GWAE_E2E_BIN="$PWD/target/release/gwae" cargo test -p gwae --test pixel_size_e2e
```

The private real-application replay can select another executable through
`GWAE_ACCEPT_BIN`. See [terminal compatibility](TERMINAL-COMPATIBILITY.md) for
supported operations, quotas, transparent-text compositing and reload limits.

## Compressed host uploads (2026-09-13)

Image panes made the whole session feel sluggish, not just the image pane:
every pane is painted from one thread, so one pane's host output volume is the
entire frame budget.

A full-pane page raster (1400x1000 RGBA) is 5.6 MB, which base64 alone expands
to ~7.4 MB of host output per redraw. Uploads now use the Kitty protocol's
`o=z` zlib option at the cheapest level, which is both smaller and faster than
sending raw base64.

Measured with real tdf 0.5.0 under real gwae on a real PTY
(`cargo test -p gwae --test pdf_e2e -- --ignored`):

| Measurement (real tdf, real PTY) | before | after |
| --- | --- | --- |
| host output at startup | 855,885 bytes | 81,053 bytes |
| one page turn | ~818,000 bytes | ~44,000 bytes |
| 20 page turns | 16,370,652 bytes | 898,852 bytes |

A still page costs 0.00-0.20% of a core, since the texture cache is keyed by
content and an unchanging page is never re-rastered. Twenty page turns in
2.4 seconds cost under 3% of a core.

## Why gwae re-rasterizes, and what it costs

gwae is an emulator in the middle, not a pass-through. Kitty graphics place
images at cell coordinates in the host's single grid, and the protocol has no
clipping rectangle, so forwarding a child's packets unchanged would let one
pane's image spill across its neighbors. gwae therefore decodes each child
image, crops and scales it to the pane rect, and re-emits it under its own
host IDs.

That cost is real but not perceptible. Keystroke to first byte of the
resulting redraw, for a fit/fill toggle that re-scales the whole page and
defeats the texture cache (10 samples per run, real tdf under real gwae):

| build | mean | worst |
| --- | --- | --- |
| uncompressed uploads | 4.7-5.8 ms | 8.8-13.9 ms |
| `o=z` uploads | 5.9-6.0 ms | 11.6-13.8 ms |

Both are inside one 60 Hz frame and their ranges overlap, so compression is
CPU-neutral here while removing ~800 KB of writes per page turn. Isolated,
zlib level 1 is also *faster* than base64 alone on a full-pane raster
(1.06 ms vs 4.13 ms for 7.4 MB), because it has far less output to encode.

No sub-cell scrolling limitation was observed. True pass-through would require
the host to expose child surfaces or scissor rects, neither of which the Kitty
protocol or Ghostty offers today.

For a representative text page the raster itself compresses 5,600,000 -> 40,674
bytes (137x) in 2.4 ms. Real Ghostty acknowledges an `o=z` transfer with
`OK`, confirmed by a direct `a=q` query against the live terminal.
