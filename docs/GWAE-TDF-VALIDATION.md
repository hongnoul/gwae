# GWAE / tdf validation ledger

Validated on macOS ARM64 on 2026-09-12 with Ghostty, Yazi 26.8.15 and tdf
0.5.0. The installed repair is commit `ccda77a`, release executable SHA-256
`cde69bc9e6f2d6cb833e02585ea4d8e6a1575743b29d09939abb6ffd783d7e5f`.
Subsequent changes add tests and documentation only.

## What actually improved

The original live tdf process was sampled blocked in
`get_font_size_through_stdio` while opening a PDF from Yazi. Replaying the
same initial-geometry test against the backed-up installed executable failed:

```text
host: rows=30 cols=120 pixels=960x480
before: START 28 29 0 0
required and repaired: START 28 29 232 448
```

The repaired executable returned the following **actual bytes read by the
child**, not responses supplied by the test:

```text
CSI 14 t -> ESC [ 4 ; 448 ; 232 t
CSI 16 t -> ESC [ 6 ; 16 ; 8 t
CSI 18 t -> ESC [ 8 ; 28 ; 29 t
```

The same real four-page PDF then displayed in Ghostty -> installed GWAE ->
Yazi -> Enter. The viewer showed a page image and `Rendered: 100%`, rather
than the previous startup wait or `Loading...` at 100%. A second selected PDF
opened after quitting the first. Page navigation reached `2 / 4`. This is a
functional improvement, **not parity with native Ghostty image quality**.

## Public behavior and its observed check

Test names below are in `crates/gwae/tests/pixel_size_e2e.rs` unless qualified.
The outer and inner PTYs are real kernel PTYs and the tests execute the actual
selected GWAE binary. The inner probe reads its own ioctl and stdin.
Dimensions written as `AxB` below mean columns x rows or pixel width x height.

| Requirement / changed public output | Concrete check and observed result |
| --- | --- |
| Pane pixels must be valid before the child first runs | `initial_child_ioctl_and_fragmented_coalesced_queries_match_pane_pixels`: first ioctl was 28 rows, 29 columns, 232x448 pixels. Pre-fix executable returned 0x0 and failed this assertion. |
| CSI 14/16/18 must report the requesting pane, not the host | Same test: exact replies were `4;448;232t`, `6;16;8t`, `8;28;29t`, matching the child's ioctl. |
| Fragmented and coalesced queries must not disappear, duplicate, or reorder | Same test sent bytes 35 ms apart and a combined four-query batch. Exact byte equality passed, including a quiet interval to detect duplicate replies. `gwae-term::query_replies_preserve_order_at_every_byte_boundary` also passed all split points and one-byte feeds. |
| DA/DSR behavior must remain correct | Same executable test moved the cursor to 3;7 and received `ESC[?6c`, `ESC[0n`, `ESC[3;7R`. `gwae-term::dsr_uses_live_cursor_even_when_scrollback_is_visible` returned 4;9 then 2;3 despite visible scrollback. |
| Unfocused panes must receive only their own replies | `differently_sized_panes_receive_only_their_own_query_responses`: focused pane reported 39x28 cells / 312x448 pixels, unfocused pane 29x28 / 232x448. Three overlapping request rounds matched each pane separately. |
| Padded host resizes must use integer cell pixels | `host_resize_and_pixel_only_font_change_update_ioctl_and_query_replies`: host 144x36 cells / 1303x619 pixels produced child 35x34 / 315x578 and CSI16 cell size 9x17. |
| Pixel-only font changes must update the kernel and replies | Same test kept host cells 144x36, changed pixels to 1735x727, and observed child 420x680 with 12x20 cells. No character-dimension resize was needed. |
| Width cycles and fullscreen must preserve parity | Same test: third-width child 39x28 / 312x448 and fullscreen child 119x28 / 952x448. The exact coalesced size replies passed after each real key binding. |
| Newly created panes must use current measurements, not startup defaults | `newly_spawned_pane_first_ioctl_uses_current_host_pixels`: after a host/font resize, real Alt+Enter created a child whose **first** ioctl was 35x34 / 420x680. Its CSI14/16/18 replies matched. |
| Reload must preserve live child PTYs and their pixel geometry | `hot_reload_inherits_live_pty_and_preserves_pixel_query_parity`: replacing a test-owned binary produced the adoption Ctrl+L and a reconstructed cursor at 1;1 instead of 3;7. The helper PID stayed identical with exactly one START record. Inherited ioctl/reply parity passed at 232x448, then 290x560 after a pixel-only change, then 420x680 after a cell resize. The user's installed executable was never replaced by this test. |
| Unknown metrics must remain unknown, not fabricated | `geometry::tests::unknown_or_invalid_metrics_are_not_fabricated` passed. `gwae-term::geometry_replies_use_current_measurements_and_resize` returned `4;0;0t`; `csi_16t_uses_current_measured_cells` returned `6;0;0t`. |
| Small or extreme dimensions must not overflow or disagree | `geometry::tests::extreme_dimensions_do_not_overflow_or_disagree_with_cell_metrics` passed. `gwae-term::pixel_callbacks_clamp_before_multiplying_and_reclamp_after_resize` returned 65520x65520 pixels, then 65534x65535 at the normalized 2x1 grid. |
| Cancellation, malformed CSI, and OSC/APC/DCS payloads must not trigger size replies | `gwae-term::csi_16t_rejects_payloads_cancelled_and_nonplain_sequences` and `query_like_payload_in_osc_and_apc_does_not_reply` passed every split point. No reply was emitted until a subsequent valid request. |
| Title events and synchronized-output replies must retain their behavior | `gwae-term::queries_do_not_change_titles_or_title_stack_behavior` observed second -> first -> empty title. `synchronized_output_flush_preserves_reply_order` matched the exact five-reply sequence. |
| tdf's combined picker probe must get measured font size without false graphics support | `gwae-term::tdf_picker_receives_font_size_without_advertising_unsupported_graphics` replayed its exact query at every split. Replies were DA, `6;34;16t`, then DSR. No Kitty success acknowledgement was invented. |
| The delivered binary must be the tested artifact | Installed `~/.bun/bin/gwae` SHA matches the release SHA above. All five expanded pixel-size tests passed against that installed path, including three independent reviewer replays. |

## Actual application acceptance

These observations used real Yazi and tdf, not replacements for either program.

| Workflow requirement | Observed result |
| --- | --- |
| Same PDF in native Ghostty and inside GWAE | Both displayed the same SHA-256-matched four-page PDF. Native Ghostty showed legible high-resolution pages. GWAE showed a correctly bounded but blocky halfblock image at `Rendered: 100%`. Native-quality parity **fails**. |
| Spaced and Unicode filenames, normal Enter opener | Yazi selected `01 space λ.pdf` and `02 second.pdf`, selection count 2. Its actual foreground shell command quoted each complete path. The first tdf process opened the Unicode/spaced path and rendered the page. |
| Multiple selected PDFs open sequentially | Quitting the first viewer started a new tdf process for the second path under the same opener shell. Both real viewers reached `Rendered: 100%`. |
| Page navigation | Second PDF visibly reached `2 / 4`, with a different page image. |
| Resizing and clipping beside another pane | Real fullscreen produced a 245x65 child / 3430x2080 pixels. Cycling back to quarter-width produced 61x65 / 854x2080, matching its sibling shell. The PDF stayed inside its own pane. An old oversized page was initially clipped, then ordinary backward/forward navigation redrew it to fit at 100%. Immediate fit after shrinking **is not fully passing**. |
| Pane switching | A sibling fish pane was created through Alt+Enter, focus moved there and back, and the PDF process remained alive. The eventual page-2 capture showed the fitted image in the left pane and an uncontaminated shell in the next pane. This verifies the observed halfblock path, not Kitty host-image lifetime management. |
| Zoom | The running viewer's help explicitly said `Not using kitty, kitty-specific keybindings hidden`. Kitty zoom is unavailable in this fallback. This acceptance item **fails**, not merely untested. |
| Quit back to a usable Yazi | GUI process inspection confirmed both tdf processes exited and the same Yazi process survived. A separate real-application PTY run observed both PDFs at 100%, then a fresh Yazi `NOR` screen with both names. Sending `k` produced a new highlighted first-file row and `1/2` status. |
| Accurate compatibility documentation | README/site state that Kitty support is partial, not general compatibility. The compatibility page and this ledger explicitly retain negative native-quality, zoom, and immediate-resize results. No host-image support is claimed from a passing geometry test. |

## Reproduction and local evidence

```sh
cargo test -p gwae-term
GWAE_E2E_BIN="$HOME/.bun/bin/gwae" cargo test -p gwae --test pixel_size_e2e -- --nocapture
cargo test --workspace -- --test-threads=1
```

The repair's full serial workspace run passed 591 tests with 7 guarded helpers
ignored. The subsequent test-only additions passed all 5 pixel-size cases
(one guarded helper ignored), and all 43 terminal-core tests passed again.
These counts supplement, not replace, the observations above. A parallel run
had an unreproduced Shift-drag failure; the isolated test passed 20 times and
the serial paste suite passed all 22 cases. No geometry regression was found
in that investigation.

Private local artifacts are retained under `$JCODE_SCRATCH_DIR` and are not
committed because the screenshots contain the user's PDF:

- `gwae-pixel-before.log`: failing initial ioctl from the backed-up executable.
- `gwae-pixel-after.log`, `gwae-pixel-followup.log`: exact child reply/geometry logs.
- `pixel-spawn-reload-evidence/replays.log`: independent installed and checkout replays.
- `gwae-tdf-acceptance-20260912/native.png`: native high-resolution baseline.
- `gwae-tdf-acceptance-20260912/yazi-selected.png`, `hosted-first.png`, `help.png`:
  real selection, successful startup, and unsupported Kitty controls.
- `gwae-tdf-acceptance-20260912/hosted-second-nav-resize.png`,
  `hosted-post-resize-navigation.png`: initial clipped resize and subsequent fitted page 2.
- `gwae-tdf-acceptance-20260912/live-geometry.txt`: both live pane measurements.
- `gwae-tdf-acceptance-20260912/pty-workflow.py`, `pty-workflow-results.json`,
  `pty-*.ansi`: actual opener, two viewers, and responsive Yazi-return observations.

The real-application PTY harness initially sent selection keys before the
initial frame had fully drained and selected only the second PDF. It was
corrected to wait for output quiescence and each selection update, without
weakening the two-viewer assertions. The final run passed every workflow step.

The native-quality / zoom gap remains a separate unimplemented Kitty graphics
path. This ledger closes the **measurement and diagnosis loop**, not that
graphics implementation gap.
