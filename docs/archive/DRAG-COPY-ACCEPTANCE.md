# Drag-copy acceptance evidence

Current paste round trips use the host-native paste action. The earlier replay used Option+V, which has since been removed from gwae. Selection/copy assertions are unchanged.

Verified on macOS arm64 on 2026-09-09. Runtime fix: `262630f`.

The requirement is functional copy on drag release, not just selection highlighting.
The tests run the actual gwae executable under a host PTY, inject SGR mouse events,
and inspect the rendered screen, clipboard-helper input, and bytes received by the
child after paste. Clipboard helpers use isolated files so tests never replace the
user's OS clipboard.

## Before and after

The **same** `drag_copies_on_release_then_native_paste_delivers_selected_text` assertion
fails against the preserved pre-fix executable at `release highlights and copies
cells 1 through 6`. The old executable displays the highlight but never replaces
the clipboard. The restored executable and the installed local command pass:

1. While dragging, the clipboard remains unchanged.
2. Forward release copies exactly `LAIN_R` and displays `copied 1 line`.
3. A plain click clears the highlight without copying.
4. Reverse release copies exactly `PLAIN_R`.
5. A duplicate release does not copy again.
6. Option+V delivers exactly `PLAIN_R` to the real child PTY. There are exactly
   two native helper invocations, one per completed selection.

This is an observed behavior change, not a test-count proxy.

## Requirement and public-output mapping

Integration test names below are in [`paste_e2e.rs`](../crates/gwae/tests/paste_e2e.rs).
Unit test names are in [`select.rs`](../crates/gwae/src/select.rs).

| Requirement or changed output | Concrete check | Observed result |
| --- | --- | --- |
| Copy selected text only on release | `drag_copies_on_release_then_native_paste_delivers_selected_text` | Clipboard unchanged during drag. Forward and reverse releases write the exact selected slices. |
| Keep highlight and clipboard endpoints consistent | Same real-PTY test | Release extends the highlighted range and copies the extended range, not the previous drag frame. |
| `copied 1 line` confirmation | Same real-PTY test | Exact confirmation appears after a native helper succeeds. |
| `copied 2 lines` confirmation and Unicode preservation | `drag_copies_multiline_unicode_without_padding_or_duplicate_wide_cells` | Clipboard equals `ROW_ONE 日本語 é\nROW_TWO 끝`, with combining marks, no duplicate wide cells, and no grid-padding spaces. Toast says `copied 2 lines`. Paste returns identical UTF-8 bytes. |
| Plain clicks and duplicate releases must not overwrite the clipboard | `drag_copies_on_release_then_native_paste_delivers_selected_text` | Plain click preserves `LAIN_R`. A stray release after the second drag causes no third helper call and no range change. |
| Blank selections preserve clipboard and report `nothing to copy` | `empty_drag_does_not_replace_the_clipboard` and `blank_copy_leaves_all_clipboard_outputs_untouched` | No native helper or OSC 52 output. Original clipboard still pastes. Blank, newline-only, and whitespace-only strings are rejected. Exact toast is rendered. |
| Zero-sized grids cannot produce spurious copied text | `empty_grid_dimensions_have_no_text_to_copy` | Zero columns and zero rows both extract an empty string. |
| Dragging beyond a pane cannot copy a neighboring pane | `drag_outside_the_pane_copies_to_its_edge_not_the_neighbor` | Release at the far host edge copies only `AIN_READY` from the owning pane, once. |
| Mouse-reporting children own ordinary drags | `child_owns_plain_drag_but_shift_drag_copies_even_if_shift_is_released_first` | Child receives the exact three translated SGR events. Clipboard remains unchanged. |
| Shift-drag stays owned by gwae if Shift is released early | Same ownership test | Unmodified mouse-up completes the captured drag and copies `MOUSE_R`. A subsequent input barrier proves no drag tail leaked to the child. |
| Native-copy success must not also send OSC 52 | Main drag test and `native_helper_consumes_utf8_and_eof_without_a_second_terminal_write` | Helper consumes UTF-8 through EOF and exits successfully. Captured terminal output contains no OSC 52 write. |
| Failed native helper falls back without a false `copied` claim | `failed_or_stalled_native_copy_falls_back_without_claiming_confirmed_success` | Failing helper emits exact `ESC ] 52 ; c ; UExBSU4= BEL` for `PLAIN`. Screen says `copy sent to terminal`, not `copied 1 line`. |
| Stalled helper and a full stdin pipe cannot hang copying indefinitely | Same failure integration test and `a_helper_that_never_reads_cannot_block_a_pipe_sized_copy_forever` | Sleeping helper is terminated within the timeout budget. A 256 KiB blocked write returns failure. Pane accepts subsequent input after fallback. |
| SSH must target the host, not the remote clipboard | `ssh_copy_targets_the_host_terminal_without_touching_the_remote_clipboard` | Tested `SSH_CONNECTION` and `SSH_TTY` independently. Both emit exact OSC 52 with unconfirmed feedback, never invoke local `pbcopy`, and preserve the remote clipboard fixture. |
| Missing helpers trigger fallback | `absent_helper_sends_a_terminal_request_not_a_confirmed_copy` | Missing executable yields `SentToTerminal`, exact OSC 52 bytes, and `copy sent to terminal`. |
| OSC 52 encodes text safely and correctly | `osc52_encodes_utf8_padding_and_control_bytes_as_data` | Exact base64 matches for one-, two-, and three-byte groups, Japanese text, newlines, and escape characters. Text cannot break out of the OSC payload. |
| Output errors produce `clipboard unavailable` | `failed_terminal_write_or_flush_reports_unavailable` | Write failure and flush failure each return `Unavailable`. Its message is exactly `clipboard unavailable`. This is a unit check because broken terminal output cannot display an end-to-end toast. |
| Existing paste behavior remains usable | `option_v_pastes_the_clipboard_into_a_plain_pane`, `option_v_multiline_paste_arrives_as_one_block`, and `option_v_in_a_real_fish_pane_pastes_instead_of_opening_an_editor` | Single-line and multiline bytes arrive intact. Real fish buffers the paste and executes only after Enter. |
| Installed artifact contains the fix | Run the entire clipboard integration target with `GWAE_E2E_BIN` set to the installed command | All ten integration tests pass against the signed, atomically installed local executable, including the new SSH cases. |

## Replay

```sh
cargo test -p gwae --bin gwae select::tests -- --nocapture
cargo test -p gwae --test paste_e2e -- --nocapture
GWAE_E2E_BIN="$(command -v gwae)" cargo test -p gwae --test paste_e2e -- --nocapture
```

To compare a retained old executable, set `GWAE_E2E_BIN` to that file and run
`drag_copies_on_release_then_native_paste_delivers_selected_text`. The pre-fix version
must fail at the clipboard-on-release assertion, not merely compile differently.

Broader checks include `cargo test --workspace --no-fail-fast`,
`cargo check --workspace --all-targets`, `cargo fmt --all --check`, release build,
package file-list assertions, and `git diff --check`.

### Observed audit results

- Full workspace rerun after closing the SSH and zero-grid gaps: **562 passed,
  0 failed, 1 pre-existing ignored test**.
- Selection/clipboard unit target: **17 passed**.
- All clipboard PTY tests against the installed binary: **10 passed**.
- Identical single-test replay: pre-fix executable fails at release with the
  original clipboard and zero helper calls. Installed executable passes and
  pastes the newly selected text. This negative control was rerun during the audit.
- All-target compilation, formatting, package file inclusion, signature
  verification of the installed executable, and diff checks passed.
- Pre-fix executable SHA-256:
  `87459adcdce12216dd8d8339fedc97b7adad176b06004735f85b9d872aafe04f`.
- Installed, signed fixed executable SHA-256:
  `3b0a1f7c9e0a5850047b1a116431574160dae7222cbdb5defbf5852d492a03d9`.

## Limits

- Clipboard transport is exercised through actual subprocess and PTY boundaries,
  but native helpers are isolated fixtures. The user's actual clipboard is not
  read or changed by these tests.
- OSC 52 acceptance is controlled by the host terminal and cannot be confirmed
  merely by writing a request. The UI deliberately does not claim confirmation.
- Linux and Windows native clipboard services were not run in this macOS audit.
- One existing timing-sensitive splash/compositor test is ignored by the normal
  workspace suite and reserved for nightly runs.
- The fix is installed for new launches. Existing user panes were not explicitly
  restarted or terminated during verification.
