# Copy and paste (`⌥+c` / `⌥+v`)

## Problem

gwae captures the mouse and the keyboard, which takes both halves of the
clipboard away from the host terminal and gives back only one of them.

Today:

* **Copy** exists in exactly one shape: a left-drag inside a pane
  (`select.rs`) that copies on release. There is no keyboard route, no
  "this pane" scope, and no "the turn the agent just finished" scope — the
  two scopes a user asks for by name when talking to an agent.
* **Paste is unowned.** gwae never enables bracketed paste on the host
  (`DECSET 2004`), never handles `Event::Paste`, and never re-emits the
  markers a child asked for. A `⌘+V` of five lines therefore arrives as raw
  key events, `\r` decodes to `KeyCode::Enter`, and `Cmd::Input(b"\r")`
  reaches the pane five times. In a shell that runs four half-typed
  commands; in an agent harness it submits four half-written prompts. This
  is the single worst thing in gwae's input path and it is invisible until
  it costs someone a wrong prompt.

So the request "bind Option+c/v" is really two requests stacked: make paste
*correct*, then put both halves on symmetric chords.

## Why `⌥+c` / `⌥+v` and not `⌥+y`

`docs/ROADMAP.md` reserved `⌥+y` for yank. That was the right shape (one
chord, context-chosen scope, amendable via a short-lived overlay) on the
wrong key. `⌥+c`/`⌥+v` wins for three reasons:

1. **It is already in the user's fingers.** Every platform spells this pair
   `⌘/Ctrl + c/v`. gwae's whole modifier story is "swap the platform
   modifier for `⌥` and everything else is where you expect".
2. **It is a pair.** `⌥+y` had no paste counterpart, so paste would have
   landed on an unrelated key and the two halves would never read as one
   feature in the cheat-sheet.
3. **`⌥+c` was already promised once.** `config.rs` carries a regression
   test (`cowsay_defaults_do_not_teach_dead_keys`) that exists because the
   default hints advertised `⌥+c` for a binding that never shipped. Users
   pressed it. Making it the copy chord is the cheapest way to stop that
   test from guarding a hole.

`⌥+y` becomes an alias, not a second design: same `Cmd`, same overlay, so
vi-fingered users lose nothing and the roadmap entry is not thrown away.

**Cost, stated plainly.** `⌥+c` and `⌥+v` currently fall through to the
pane as `ESC c` / `ESC v`, which readline reads as `capitalize-word` and
emacs as `scroll-down`. Both are rare inside a mux pane and both remain
reachable by typing `ESC` then the letter, since `handle_key` treats a bare
`Esc` as a chord preamble and forwards it. Document it in the FAQ rather
than discover it in an issue.

## Layer 0 — paste correctness (prerequisite, no new binding)

This ships first and alone. It is a bug fix, it needs no key, and every
later layer routes through it.

1. **Enable bracketed paste on the host**: `EnableBracketedPaste` next to
   `EnableMouseCapture` in `run_tui`, disabled in the same teardown paths
   (normal exit, the four early-return error paths, and the panic hook's
   restore) that already unwind mouse capture and DECAWM.
2. **Handle `Event::Paste(String)`** as a first-class event beside
   `Event::Key` / `Event::Mouse`, routed to the focused pane through the
   *same* snap-to-live path `Cmd::Input` uses.
3. **Re-bracket for the child.** `gwae-term` exposes
   `screen().bracketed_paste()` (vt100 already parses it, gwae just never
   read it). When the focused pane's grid has it set, wrap the payload in
   `ESC [ 200 ~` / `ESC [ 201 ~` so the child sees a paste, not typing —
   this is what makes an agent harness put a 40-line paste in its buffer
   instead of submitting line 1. When it is *not* set (a plain shell), send
   the bytes verbatim.
4. **Sanitize.** Strip `ESC [ 201 ~` from the payload itself (the standard
   paste-injection guard: a payload containing the end marker could
   otherwise close the bracket early and run its tail as keystrokes), and
   normalize `\r\n` → `\r`.
5. **Chunk.** Write in ≤4 KiB chunks with flushes so a huge paste cannot
   block the event loop behind a full PTY buffer.

**Feedback loop.** `crates/gwae/tests/paste_e2e.rs`, modeled on
`select_e2e.rs`: drive the real binary on a real PTY, send
`ESC[200~line1\rline2\rESC[201~`, and assert the child received one
bracketed run rather than two Enters. The child is `cat -v`-ish so the
assertion is on bytes, not on a screen render.

## Layer 1 — `⌥+v`, paste from gwae's own read of the clipboard

Layer 0 covers `⌘+V` typed into gwae. `⌥+v` covers the cases it cannot:
terminals that do not implement bracketed paste, remote sessions, and users
whose muscle memory is now "every gwae verb is an `⌥` chord".

* Read the clipboard the mirror of `copy_to_clipboard`: `pbpaste` /
  `wl-paste --no-newline` / `xclip -o -selection clipboard` / `xsel -ob` /
  PowerShell `Get-Clipboard`. No OSC 52 read path — the reply would land on
  gwae's stdin mid-frame and most terminals refuse the read anyway for good
  security reasons. Degrade with a toast: `clipboard unreadable`.
* Feed the result into the exact function Layer 0 wrote. One paste path,
  two entrances.
* **Guard large and multi-line pastes.** Over ~4 lines or ~2 KiB, do not
  paste immediately: toast `paste 38 lines? ⌥+v again to confirm` and
  commit on a repeat within `NOTE_LINGER`. gwae already owns this exact
  interaction for `⌥+Shift+q` (`draw_quit_confirm`), so it costs a
  paragraph, not a mechanism. This is the difference between a mux that
  pastes a repo into an agent prompt and one that asks first.

## Layer 2 — `⌥+c`, copy with a scope

Keyboard copy, with the scope chosen by context rather than by asking:

1. a live drag-selection in the focused pane → the selection;
2. else the pane has OSC 133 marks → the last turn;
3. else → the visible pane plus its scrollback.

The toast reports what happened and advertises the amendments:
`copied turn · 38 lines   p pane  s sel`. Within `NOTE_LINGER`, `p`/`s`/`t`
re-copy at that scope; anything else commits. Amending is free because the
clipboard is a scratch register — unlike `⌥+q`, there is nothing to undo.

Enabling work, unchanged from the roadmap's yank entry and in this order:

1. **Scrollback text API** in `gwae-term`. `TermGrid` exposes the visible
   screen plus `scrollback_offset` only, so "the whole pane" is currently
   unreachable. Needs a row-range accessor over the vt100 scrollback.
2. **Prompt marks.** `tui.rs` keeps OSC 133 *status* (`saw_osc133`) but
   discards position. Store per-pane `PromptMark { abs_row, exit }`, fixed
   up as rows scroll out, to delimit a turn. Turn scope is the unit people
   actually paste elsewhere, so this is the layer that earns the feature.
3. **`Effect::Menu`.** The second keystroke of the toast is not expressible
   in today's `Effect`, and `binds.rs` is load-bearing: its test feeds every
   entry through the real `handle_key`. Add `Effect::Menu(&[MenuItem])` plus
   a test that presses `⌥+c` then each item key and asserts the resulting
   `Cmd::Copy { scope }`. Without that the cheat-sheet and the cowsay hints
   can drift from the dispatcher, which is the exact failure `binds.rs`
   exists to prevent.

## Layer 3 — images, behind a config key

`image_clipboard = false` by default. Cells → RGBA via `theme.rs` and an
embedded mono font → PNG (`shot.rs`), then macOS `osascript` «class PNGf»,
Wayland `wl-copy -t image/png`, X11 `xclip -t image/png`, Windows
`Set-Clipboard -Path`. `⌥+Shift+c` takes the shot. No OSC 52 equivalent
exists, so over SSH write a file and toast the path.

## Sequencing

| Phase | Ships | Depends on | Why here |
|---|---|---|---|
| P0 | bracketed paste in/out, chunking, sanitizing | — | Correctness bug; silently corrupts agent prompts today |
| P1 | `⌥+v` with the large-paste confirm | P0 | Makes paste a gwae verb, not a host accident |
| P2 | `⌥+c` selection + pane scope | P0, scrollback API | Keyboard copy without a mouse |
| P3 | turn scope, `⌥+y` alias, `Effect::Menu` | P2, prompt marks | The scope agents make people want |
| P4 | image copy | P3, encoder | Nice-to-have, gated by config |

## Invariants

* `binds.rs` stays the single source of truth: a new chord means a `Bind`
  with a `hint`, a README table row, and a dispatcher assertion, or the
  build fails. That is three files by design.
* The clipboard is never written by a plain click (`select.rs` already
  drops empty selections) and never read without the user asking.
* Every new path degrades loudly with a toast, never silently: a mux that
  drops a paste is worse than one that says it cannot paste.
