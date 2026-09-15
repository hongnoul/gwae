# Native multiline paste acceptance evidence

Validated on macOS arm64 on 2026-09-09 (local time).
Original multiline fix: `7481d9e`. Baseline executable: `14c8761`.
The later single-key consolidation is validated separately below.

## Measured improvement

The same fixture pastes two `paste_probe` commands separated by a blank line
and followed by a newline into real interactive shells running inside the
actual gwae executable. The zsh fixture also includes Japanese, Korean, and
a combining accent as quoted arguments. Each `paste_probe` appends
one `EXECUTED` line to a dedicated file. A Ctrl+G binding writes a separate
barrier file without submitting the command buffer. Assertions run only
after that barrier proves the shell has processed the complete paste.

The host PTY harness follows the terminal protocol: it emits paste delimiters
only after gwae requests `DECSET 2004`. It does not use the Option+V clipboard
helper and does not force framing on the broken baseline.

| Actual shell | Baseline executions before Enter | Fixed release executions before Enter | Fixed release executions after Enter |
| --- | ---: | ---: | ---: |
| fish 4.8.1 | 2 | 0 | 2 |
| zsh 5.9 | 2 | 0 | 2 |
| macOS bash 3.2.57, no bracketed-paste support | 2 | 2 | Not needed: already executed |

This directly demonstrates the improvement in supporting shells: normal paste
no longer executes either command until Enter. It also demonstrates the limit:
macOS bash 3.2 still executes pasted newlines. No quiet-period heuristic or
counting of rendered shell text is used to infer execution.

### Direct observation of the editable prompt

The zsh barrier also records ZLE's actual `$BUFFER`, not just received PTY
bytes. On the baseline it was empty because the commands had already run.
On the fixed release it exactly matched the pasted text before Enter:

```text
paste_probe '日本語 é'

paste_probe '끝'
```

The byte-for-byte assertion includes the trailing newline after the second
command. The observed execution count at that point was zero. After Enter
it was two. This directly checks that native paste leaves the complete
multiline Unicode text editable inside a real shell.

## Original fix requirement-to-observation mapping

| Requirement | Regression | Observed result |
| --- | --- | --- |
| Native paste must not require Option+V | `native_paste_in_a_real_fish_pane_waits_for_enter`, `native_paste_in_a_real_zsh_pane_waits_for_enter` | Both failed on the baseline with two executions before Enter. Both passed on the fixed debug and release executables with zero before Enter and two after it. |
| Preserve blank lines and Unicode | `native_paste_in_a_real_zsh_pane_waits_for_enter`, `native_paste_preserves_blank_lines_and_unicode_in_plain_panes`, `native_paste_lf_input_preserves_consecutive_blank_lines`, `blank_lines_survive_paste_normalization` | The actual zsh edit buffer exactly matches the multiline Unicode payload, including its blank line and trailing newline. Raw captured bytes also retain consecutive blank lines, Japanese, Korean, and combining characters. LF-only payloads no longer lose blank lines. CR/CRLF normalization remains compatible with the existing paste encoder. |
| Paste content must not invoke gwae shortcuts | `native_paste_reframes_a_large_block_for_a_bracket_aware_child` | A block larger than 4 KiB, including literal `√`, arrives once with exactly one outer delimiter pair and all expected text. No second clipboard read replaces it. |
| Respect the child's own paste mode | `native_paste_tracks_child_mode_without_disabling_host_framing` | Child receives delimiters while enabled, then plain bytes after disabling its mode. Host framing stays enabled throughout. |
| Preserve non-supporting programs' input behavior | `native_paste_in_a_real_macos_bash_keeps_its_unbracketed_behavior`, plain-pane byte-capture tests | Bash 3.2 executes both pasted commands before Enter on both versions. Plain programs receive no unwanted escape markers. |
| Pasted newlines must not confirm quit | `native_paste_cannot_confirm_quit_or_leak_to_the_pane` | Paste cancels the prompt and is discarded. The next separately typed sentinel reaches the pane, without any pasted content. |
| Directory picker must consume paste | `native_paste_in_directory_picker_only_updates_the_filter` | Only the first CR-delimited path appears in the filter. No pasted content reaches the child. This initially exposed a CR-only split bug, which was fixed and rechecked. |
| Restore terminal state | `native_paste_mode_is_enabled_and_restored_on_exit` | Host mode is visibly enabled at startup and disabled on normal exit. |
| Do not break optional paste, copy, or lifecycle behavior | Existing clipboard, hot-reload, and teardown suites | Existing Option+V, drag-copy, helper-failure, reload, and teardown tests pass. |

## Original fix checks, before single-key consolidation

- 20 clipboard/paste PTY integration tests passed against `target/release/gwae`.
- All three real-shell compatibility checks passed against the debug build.
- Running those same three against the baseline produced the expected two
  fish/zsh failures and one bash compatibility pass.
- Earlier full native-paste comparison: eight of nine tests failed against
  the baseline, all nine passed after the fix.
- 380 workspace library/binary unit tests passed.
- Eight hot-reload and eight teardown integration tests passed.
- Strict workspace/all-target Clippy and `git diff --check` passed.

Re-run the executable-level checks:

```sh
cargo build --release -p gwae
GWAE_E2E_BIN="$PWD/target/release/gwae" \
  cargo test -p gwae --test paste_e2e -- --nocapture
cargo test --workspace --lib --bins
cargo test -p gwae --test hotreload_e2e --test teardown_e2e
cargo clippy --workspace --all-targets -- -D warnings
```

Set `GWAE_E2E_BIN` to a saved pre-fix executable to reproduce the baseline
comparison without changing the checked-out source.

## Single-key consolidation: whole-result verification

Native paste is now the only gwae paste action. The separate Option+V
command, clipboard readers, picker shortcut, and agent-specific shortcut
routing are removed. The host sends the content once and the child owns
content interpretation. This does not remap or remove a child's own keys.

| Requirement | Check | Observed result on the final build |
| --- | --- | --- |
| One advertised shortcut | `native_paste_is_the_only_advertised_paste_shortcut`, `only_native_paste_is_advertised_in_hud_and_cow` | The rendered HUD contains Cmd+V and no Option+V paste entry. The binding table contains exactly one paste row, with the native label. |
| No separate clipboard read | `option_v_is_no_longer_a_gwae_clipboard_shortcut` | ESC+v and literal `√` reach the child unchanged, with zero clipboard helper reads. The saved pre-consolidation executable fails this exact test by pasting the fixture clipboard twice instead. |
| Native paste owns delivery feedback | `native_paste_confirms_delivery_into_a_plain_pane`, `native_multiline_paste_confirms_delivery_and_warns_without_brackets` | One-line paste is confirmed. CR-only multiline paste delivers all three lines, including the blank line, and reports the correct count plus the unsupported-child warning. |
| Same route in agent panes | `native_paste_reaches_a_marked_agent_once_without_a_clipboard_read` | A marked fixture-agent pane receives exactly one native paste block, including the supplied image-path text, with zero clipboard reads. This tests gwae routing, not a real agent's image attachment behavior. |
| Preserve native picker paste | `native_paste_puts_a_path_into_the_picker_filter`, `native_paste_in_directory_picker_only_updates_the_filter` | The first native-pasted path reaches the filter and subsequent pasted lines do not reach the child or accept the picker. No clipboard helper or global PATH mutation is needed. |
| Keep the actual multiline shell outcome | Real fish/zsh tests above | Both still execute zero commands before Enter and two after. zsh's actual edit buffer exactly matches Unicode, blank lines, and trailing newline. Bash 3.2 retains its documented unbracketed behavior. |

After all consolidation changes, `cargo test --workspace` passed **574 tests**.
The release binary was rebuilt and all **22 paste/clipboard acceptance tests**
passed against that artifact. Strict workspace/all-target Clippy and
`git diff --check` also passed. Thus the completed acceptance loop was rerun
over the entire final result, not only over the added checks.

The marked-agent fixture never contacts a provider. Real Jcode source was
inspected to confirm its native handler recognizes image file paths and image
URLs, but no image-only desktop clipboard workflow is claimed. Ordinary
terminal paste cannot transport arbitrary clipboard image bytes on its own.

## Validation boundaries

These are real executable, PTY, and shell observations. The host terminal's
paste protocol is emulated, not a physical Cmd+V keystroke.

An additional native Terminal.app check was attempted, but macOS denied
Apple Events automation (`-1743`) before the isolated fixture launched.
No native desktop success is claimed. The original clipboard was preserved
with all its formats, restored after the blocked attempt, and the temporary
clipboard backup was removed. No automation permissions were changed.

The user's installed/running gwae was not replaced. The validated release
artifact is `target/release/gwae`. Hosts and children that do not support
bracketed paste cannot acquire its safety guarantees from this fix.
