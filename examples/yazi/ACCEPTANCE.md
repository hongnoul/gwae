# Responsive Yazi acceptance evidence

Observed on 2026-09-12 (UTC), macOS arm64, using **installed gwae 1.3.2**
and **Yazi 26.8.15**. These are real executable/PTY tests, not mocked Lua
layout calculations. Assertions inspect Yazi's rendered terminal cells and
require matching frames to remain stable for 300 ms.

## Direct improvement over the previous layout

At the same 215-column host / 53-column default gwae pane, the same Yazi
process was measured before and after `ya emit plugin gwae-responsive`.
Neither the terminal width nor the selected file changed.

| Observation | Before | After |
|---|---|---|
| Parent sibling `UP` visible | Yes | No |
| Selected file body `PREVIEW` visible | Yes | No |
| Unselected filename as rendered | `FILE_NAME_that_….txt` | `FILE_NAME_that_needs_more_horizontal_room.txt` |
| Current-file text starts at host column (zero-based) | 12 | 5 |

The **entire 45-character filename becomes readable** in the default pane.
The file is not hovered, so the complete name cannot be a false positive
from the header, selected-file status, or preview. This directly verifies
that the reclaimed side-panel space improves the file list.

## Requirement-to-observation map

Test names below refer to
[`crates/gwae/tests/yazi_e2e.rs`](../../crates/gwae/tests/yazi_e2e.rs):

- **P**: `yazi_progressively_reveals_panels_in_real_gwae`
- **B**: `yazi_breakpoints_and_live_activation_are_reversible`
- **S**: `yazi_outside_gwae_is_unchanged`
- **U**: `yazi_default_pane_displays_more_of_the_actual_file_list`
- **C**: `yazi_custom_breakpoints_restore_the_configured_ratio`

| Requirement / documented output | Concrete check | Observed again in the whole-result replay below |
|---|---|---|
| Both side panels hidden at default width | P + U, real gwae quarter pane | `CURRENT.txt` visible, `UP` and `PREVIEW` absent at 53 columns. |
| Hiding panels gives files more useful space | U, same-process before/after control | The 45-character unhovered filename changes from truncated to completely visible. |
| Right preview revealed before left parent | P, real `Alt+r` quarter → third → half cycle | Neither side → preview only → both sides. |
| Exact default breakpoints | B, widths 63, 64, 95, 96 in both directions | 63: neither. 64/95: preview only. 96: both. |
| Resizing remains reversible | P + B, back to quarter and 40/160/63 columns | Side panels disappear again on shrink and return on growth. |
| Fullscreen transitions update automatically | P, real `Alt+f` enter/exit | Both sides in fullscreen, neither after restoring quarter width. |
| Host-terminal resizing updates automatically | P, PTY ioctl changes host 215 → 400 → 215 | Neither → both → neither while keeping the quarter preset. |
| Standalone Yazi remains unchanged | S, no `GWAE_PANE`, with setup and live plugin activation | Entire rendered text at 53 → 107 → 53 columns matches unmodified baselines byte-for-byte. |
| Initialization works with no gwae rebuild | P, isolated `init.lua` plus installed gwae executable | Responsive layout after startup without interaction. Installed gwae binary was not modified. |
| Existing Yazi sessions can enable via documented CLI | U, actual `ya emit plugin gwae-responsive` targeted using the test's `YAZI_ID` | Already-running Yazi changes to the compact layout without restarting or losing selection. |
| Repeated initialization/activation is safe | P initializes twice, B activates twice then widens | Wide layout still restores both panels. |
| Custom 80/120 thresholds work | C, 79/80/119/120 and reverse crossings | 79: neither. 80/119: preview only. 120: both. |
| Wide layout restores the configured ratio | C, nondefault `ratio = [2, 3, 5]` at width 160 | Current/preview text columns `(36, 80)` exactly match the unmodified baseline after narrowing and widening. |
| Openers and keybindings are not rewritten | U + C, byte-for-byte comparisons after use | `keymap.toml` and `yazi.toml`, including a custom opener, unchanged. |
| Ordinary directory navigation still works | U, real `h` and `l` while side panels are hidden | Parent directory entered, original directory reentered, long filename still fully visible. |
| Removing setup and restarting reverts the behavior | C, fresh Yazi with plugin files still installed, no setup call, `GWAE_PANE` present | Both side panels visible again at 79 columns. |
| Local installation matches tested implementation | `cmp` of installed plugin and repository example | Byte-identical. The user's existing `yazi.toml` was not edited. |
| Documented installation honors configuration paths and preserves existing configuration | Execute the README's actual shell snippet in four isolated configurations, including paths containing spaces | HOME fallback, XDG override, Yazi override, and empty-variable fallback all install exact plugin bytes to the correct location. Existing `init.lua`, `yazi.toml`, and `keymap.toml` remain byte-identical. |

All five Yazi test scenarios passed against the installed executable. The
three existing `resize_e2e` regressions also passed, covering native PTY
size notification, redraw, and reflow. This evidence concerns the documented
text/layout workflow, not every possible third-party layout plugin or image
backend. Combining layout-owning plugins is explicitly unsupported.

## Whole-result replay after completing the requirement map

On **2026-09-12 at 00:41:49–00:42:18 UTC**, the complete current result at
`d6dc122` was checked again, including the earlier implementation from
`ff27613`. This was a fresh execution of every mapped check, not a summary
of checks run before the map existed. Every row above reports the result
observed again in this replay.

| Replay | Actual result |
|---|---|
| All five Yazi scenarios, selecting installed gwae | **5 passed, 0 failed, 0 ignored**, 4.28 s. P and U launched the installed binary. |
| All five Yazi scenarios, selecting current Cargo-built gwae | **5 passed, 0 failed, 0 ignored**, 4.36 s. P and U launched `target/debug/gwae`. |
| Native PTY resize, redraw, and primary-screen reflow regressions | **3 passed, 0 failed**, 0.72 s. Child size notifications, width rulers, bottom-row redraw, and 161-character soft-wrap reflow matched the expected viewports. |
| README installation command | **4 path-selection cases passed**, including spaces and existing configuration files. |
| Installed initialization and plugin parity | Setup call present. Installed plugin byte-identical to the tested source. |
| Formatting and strict Clippy for both acceptance targets | Both exited successfully. |

Both full Yazi runs independently reproduced the same direct improvement:
`FILE_NAME_that_….txt` became the entire
`FILE_NAME_that_needs_more_horizontal_room.txt`, and file-list text moved
from host column **12 to 5**. Both also reproduced exact configured-ratio
restoration at **(36, 80)** and byte-identical standalone rendered text.
The breakpoint, resize, navigation, configuration-preservation, and rollback
observations in the map all passed again. No implementation changes were
needed after this replay.

Fresh local evidence is retained under
`$JCODE_SCRATCH_DIR/gwae-yazi-whole-result-20260912T0041/`:

- `yazi-installed.log` and `yazi-source-built.log`: per-transition PASS
  observations and individual test results.
- `resize.log`: actual child dimensions, redraw positions, and reflow results.
- `installation-and-parity.log`: all four installation outcomes and hashes.
- `requirement-replay.json`: all 18 requirement rows matched to fresh PASS
  observations in every applicable run.
- `format.log` and `clippy.log`: static-check output.

Rendered before/after and configured-ratio screen artifacts are under
`$JCODE_SCRATCH_DIR/gwae-yazi-e2e-37324-*` (installed) and
`$JCODE_SCRATCH_DIR/gwae-yazi-e2e-37485-*` (source-built).
The installed gwae and Yazi SHA-256 values still match those below.
The tested and installed plugin SHA-256 is
`386acaeaa63a904b27bfe542ab4b593b91f5a5fb6f7de51b85302f48b7bcc57b`.

## Reproduce

```sh
GWAE_E2E_BIN="$(command -v gwae)" \
  cargo test -p gwae --test yazi_e2e -- --ignored --nocapture
env -u GWAE_E2E_BIN cargo test -p gwae --test yazi_e2e -- --ignored --nocapture
cargo test -p gwae --test resize_e2e -- --nocapture
cargo fmt --all -- --check
cargo clippy -p gwae --test yazi_e2e --test resize_e2e -- -D warnings
```

The tests print before/after screen artifact paths under
`$JCODE_SCRATCH_DIR/gwae-yazi-e2e-*` (or the system temp directory if unset).
They isolate configuration and runtime sockets from the user's sessions.

Installed binary SHA-256 values used for this observation:

```text
gwae 3b0a1f7c9e0a5850047b1a116431574160dae7222cbdb5defbf5852d492a03d9
yazi c648220dc7d4bd934199b9894fef7b41da6f0aed53e12449433c22e33acf133a
```
