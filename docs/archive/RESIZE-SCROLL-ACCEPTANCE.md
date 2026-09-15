# Resize and scrolling acceptance evidence

Verified on macOS on 2026-09-09 (UTC) against actual gwae executables in real
host PTYs. This is a before/after behavioral comparison, not source inspection.

- Baseline source: `fc9de2c`, before both fixes, built in an isolated scratch directory.
- Fixed implementation: `0dd5092` (scroll speed) and `fbbc896` (terminal reflow).
- Fixed executable: `target/release/gwae`.
- Fixed executable SHA-256: `98134a8d1292d7927794c892a0d020ee1f7668cbc62f8fa599e0a4fa158d64c9`.
- Both executables were driven by the same current acceptance tests.

## Observed improvement

| Requirement | Baseline observation | Fixed release observation |
| --- | --- | --- |
| Reflow existing output when widening a pane | Child reported `WINCH 28 39`, but output retained the old 29-column wraps. The exact viewport assertion failed. | The same 161-character paragraph changed from six rows at 29 columns to five rows at 39 columns, then three at 59 columns, without any child redraw. |
| Preserve text across repeated resize cycles | The baseline already failed at the first widening. | Three quarter/third/half cycles and two fullscreen round trips retained the paragraph and its `HARD-END` marker with exact expected wraps and no extra text. |
| Reflow when the host's width and height both change | Not claimed as a differential improvement. | Host sizes 24x96, 36x144, and 30x120 produced 7, 5, and 6 text rows at inner widths 23, 35, and 29. Returning to the initial geometry restored its exact text viewport. |
| Scroll three lines per Ctrl+Shift+K | Visible line range `(34, 60)` became `(33, 60)`: one line backward. The exact-stride assertion failed. | `(34, 60)` became `(31, 58)`: three lines backward. Three presses reached `(25, 52)`. |
| Ctrl+Shift+J reverses scrolling | Not reached after the baseline's stride failure. | One press from `(25, 52)` reached `(28, 55)`. Three presses restored the exact live range `(34, 60)`. |
| Keep child geometry and redraw synchronized | No regression comparison claimed. | Actual child `stty`/SIGWINCH logs matched each requested size. Header, right-edge ruler, and bottom-row marker appeared at the expected positions in tiled and fullscreen panes. |
| Preserve wheel behavior and harness passthrough | No regression comparison claimed. | Existing real-PTY wheel up/down, child mouse reporting, and both-direction harness key passthrough assertions passed. Each harness chord arrived exactly once. |

The full resize suite (3 tests) and scroll suite (6 tests) each passed three
release-binary replays: **27 passing test executions**. Both targeted
baseline checks failed for their intended behavioral assertions, not startup or
build errors. Logs include child geometry, observed wraps, line ranges, and
baseline failure screens.

The initial comparison exposed a weakness in the scroll test observer: decoding
each PTY read independently corrupted split ANSI and UTF-8 sequences. The test
now uses a stateful terminal decoder. Both negative controls and all 27 release
test executions were repeated successfully after this correction. PTY children
are also cleaned up when an assertion panics.

## Reproduce

The optional `GWAE_E2E_BIN` override must be an absolute executable path. Without
it, tests continue to launch Cargo's normal test binary. `--nocapture` prints the
observations used above.

```sh
cargo build --release --locked -p gwae --bin gwae
GWAE_E2E_BIN="$PWD/target/release/gwae" cargo test --locked -p gwae \
  --test resize_e2e --test scrollback_e2e -- --nocapture
```

Build the historical source outside the working tree, then run these two tests
with `GWAE_E2E_BIN` pointing to that executable. Both should fail at their
behavioral assertions:

```sh
GWAE_E2E_BIN=/absolute/path/to/baseline/gwae cargo test --locked -p gwae \
  --test resize_e2e primary_output_reflows_across_width_cycles_without_child_redraw \
  -- --exact --nocapture
GWAE_E2E_BIN=/absolute/path/to/baseline/gwae cargo test --locked -p gwae \
  --test scrollback_e2e ctrl_shift_jk_scrolls_history_three_lines_at_a_time_like_jcode \
  -- --exact --nocapture
```

Raw local logs are retained under
`$JCODE_SCRATCH_DIR/gwae-resize-scroll-acceptance-20260908-201851/`.
The terminal-core unit suite separately covers deep history, repeated lines,
Unicode, and styles. These PTY tests do not establish preservation of history
older than the configured scrollback limit or behavior in every GUI terminal.
