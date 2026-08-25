#!/usr/bin/env bash
# Record the gwae/gwae "fire demo": fast panning navigation, btm + yazi and
# a swarm of jcode agents studying MIT 5.111 chemistry, across three strips.
#
# The whole take is driven through kitty remote control (`kitten @ send-text`),
# so it is deterministic and repeatable: nobody touches the keyboard while
# `screencapture -v` rolls. Re-run until a take is good; tweak the beats, not
# your fingers.
#
#   scripts/demo_record.sh              # -> ~/Desktop/gwae-demo.mp4, ~25s
#   OUT=/tmp/take2.mov scripts/demo_record.sh
#   DRY=1 scripts/demo_record.sh        # drive the UI, record nothing
#   PACE=1 SPEED=1 scripts/demo_record.sh   # the original, unhurried 90s take
#
# Runtime is cut two ways, because neither alone is enough. PACE scales every
# beat in the script, which is free but bottoms out: panes need real time to
# spawn and draw, and agents answer at whatever speed they answer. SPEED is an
# ffmpeg retime of the finished file, which shortens anything (including an
# agent's thinking) but turns motion frantic if pushed far. Roughly half from
# each lands a ~25s cut that still reads.
#
# Chord encoding: every gwae chord goes out as Option-as-Meta, i.e. ESC +
# key, and shifted chords as ESC + the *shifted* character (ESC ':' for
# ⌥+Shift+;, ESC 'J' for ⌥+Shift+j). The macOS Unicode-glyph fallback gwae
# also decodes (Ú, Ô, ©, ƒ, ÷) is deliberately NOT used here: over remote
# control those bytes reach the focused pane as ordinary text and get typed
# into the agent's prompt instead of steering the grid.
set -euo pipefail

BIN=${BIN:-$HOME/.cargo/bin/gwae}
OUT=${OUT:-$HOME/Desktop/gwae-demo.mp4}
SOCK=${SOCK:-/tmp/gwae-demo-rc}
FONT=${FONT:-13}
PACE=${PACE:-0.42}                       # beat multiplier while recording
SPEED=${SPEED:-2.6}                      # post-hoc retime of the captured file
RAW=${RAW:-${TMPDIR:-/tmp}/gwae-demo-raw.mov}
# The demo prompts point four agents at real coursework, and an agent asked to
# "work this problem" will helpfully *edit the note*. A recording is not worth
# a surprise commit in someone's vault, so the take runs against a throwaway
# copy by default; set NOTES to the vault itself only if you want that.
SRC_NOTES=${SRC_NOTES:-$HOME/Documents/obsidian/MIT 5.111}
NOTES=${NOTES:-${TMPDIR:-/tmp}/gwae-demo-notes/MIT 5.111}
KITTY=/Applications/kitty.app/Contents/MacOS/kitty
KITTEN=/Applications/kitty.app/Contents/MacOS/kitten
DRY=${DRY:-0}

# --- key DSL ---------------------------------------------------------------
# Every wait in the demo goes through `nap`, so PACE is the single dial for how
# hurried the take is. The floor is not decoration: below ~90ms gwae and the
# pane's own program stop being visibly distinguishable, and the frame the
# viewer needs to see never gets painted.
nap()   { awk -v s="$1" -v p="$PACE" 'BEGIN{d=s*p; print (d<0.09?0.09:d)}' | xargs sleep; }
raw()   { printf '%b' "$1" | "$KITTEN" @ --to "unix:$SOCK" send-text --stdin; }
# gwae decodes a chord as ESC immediately followed by the key, and treats a
# lone ESC as a chord preamble that expires. Sending one right behind a Return
# races that window: the ESC gets consumed as the tail of the previous burst
# and the bare key is typed into the pane, which is how a prompt ends up
# reading ":Compare Kp and Kc...". A quiet gap before the chord removes the
# race; it is wall-clock rather than PACE-scaled because it is a property of
# the decoder's timeout, and 120ms was still short enough to lose occasionally.
chord() { sleep 0.30; raw "\033$1"; nap "${2:-0.35}"; }   # ⌥+key
# No trailing newline: a here-string would append one, submitting the line a
# beat before `enter` does and sending a *second* Return into whatever the
# command just launched (that stray Return is how yazi ends up opening a file
# in vim mid-take).
lit()   { printf '%s' "$1" | "$KITTEN" @ --to "unix:$SOCK" send-text --stdin; }
enter() { raw '\015'; nap "${1:-0.4}"; }
beat()  { nap "${1:-0.8}"; }
# Goes through `chord`, not `raw`: a burst is the *most* ESC-race-prone thing
# in the script (many chords back to back), and skipping the quiet gap is how a
# run of ⌥+[ leaves a trail of literal `[[[` in a pane's prompt.
burst() { local key=$1 n=$2 gap=${3:-0.12}; for _ in $(seq "$n"); do chord "$key" "$gap"; done; }
# A freshly spawned pane needs its shell (or agent TUI) to finish drawing
# before it will accept a line: type too early and the first characters land
# in the previous pane or get eaten by the redraw, which is exactly how a take
# ends up with `resubtm` on screen. Settle, then type, then submit.
# The settle is deliberately NOT scaled by PACE: it is the one wait that is a
# property of the machine rather than of the edit, and shrinking it is how a
# take ends up with `resubtm` on screen instead of `btm`.
# Ctrl+U first: the chord that spawned this pane is decoded by gwae, but on
# a bad roll the terminal delivers ESC and the key far enough apart that the
# key also reaches the new pane, leaving a stray ':' in front of the prompt.
# Timing tweaks only make that rarer; clearing the line makes it impossible.
ask()   { settle "${3:-1.2}"; raw '\025'; lit "$1"; nap 0.6; enter "${2:-0.6}"; }

# `settle` is wall-clock and deliberately immune to PACE: it covers the time a
# new pane's program needs to claim the PTY and paint. PACE is an editorial
# dial over an already-correct take, and scaling these waits with it is not a
# faster demo but a broken one -- the prompt gets typed into whichever pane
# still has focus, which is how a take grows a stray `Overwrite file?` dialog
# in yazi and a chemistry note gets edited on camera.
settle() { sleep "$1"; }

AGENT_ROW=':'   # ⌥+Shift+;  agent on a new strip below
AGENT=';'       # ⌥+;        agent right of focus
CARRY_DOWN='J'  # ⌥+Shift+j  carry pane to the strip below
JUMP='g'        # ⌥+g        smart-jump
FULL='f'        # ⌥+f        full width toggle
HUD='/'         # ⌥+/        cheat-sheet

cleanup() { pkill -f "instance-group gwae-demo" 2>/dev/null || true; rm -f "$SOCK"; }

# --- stage -----------------------------------------------------------------
if [ "$NOTES" != "$SRC_NOTES" ]; then
  rm -rf "$(dirname "$NOTES")"
  mkdir -p "$(dirname "$NOTES")"
  cp -R "$SRC_NOTES" "$NOTES"
fi
pkill -f "instance-group gwae-demo" 2>/dev/null || true; sleep 0.5
trap cleanup EXIT
nohup "$KITTY" -o allow_remote_control=yes --listen-on "unix:$SOCK" \
  --instance-group gwae-demo --title GWAE-DEMO -o font_size="$FONT" \
  --start-as fullscreen "$BIN" >/tmp/gwae-demo-kitty.log 2>&1 &
sleep 4
"$KITTEN" @ --to "unix:$SOCK" ls >/dev/null    # fail fast if RC never came up

if [ "$DRY" = 0 ]; then
  rm -f "$OUT" "$RAW"
  screencapture -v -C -x "$RAW" &
  REC=$!
  trap 'kill -INT $REC 2>/dev/null || true; sleep 2; cleanup' EXIT
  sleep 2
fi

# === STRIP 1 — the workbench: btm, yazi, width that never shrinks ==========
beat 1.2
chord '\015' 0.2; settle 0.8     # ⌥+Enter: new column
ask 'btm'                        # system monitor next to the agent
beat 2.0
chord '\015' 0.2; settle 0.8     # ⌥+Enter: new column
ask "yazi \"$NOTES\"" 1.0        # the 5.111 vault, browsable
beat 1.4
burst 'j' 4 0.20                 # yazi owns its own keys — gwae forwards
burst 'k' 2 0.20
beat 0.5

# --- ⌥+r: width cycling at speed, no reflow, no wobble --------------------
chord 'h' 0.45                   # back to btm
chord 'r' 0.6                    # 1/4 -> 1/3
chord 'r' 0.6                    # 1/3 -> 1/2
chord 'r' 0.6                    # 1/2 -> 1/4
chord 'r' 0.30; chord 'r' 0.30; chord 'r' 0.30   # again, fast: it is free
beat 0.6

# --- ⌥+s: vertical split under the monitor -------------------------------
chord 's' 0.2; settle 1.0
ask "while true; date +%T; uptime; sleep 1; clear; end" 0.8   # fish: a poor man's watch(1); macOS has none
beat 1.4
chord 'k' 0.4                    # up to btm
chord 'j' 0.4                    # down to the split
chord 'l' 0.4                    # over to yazi
beat 0.6

# === STRIP 2 — a study swarm on 5.111 ====================================
chord "$AGENT_ROW" 0.2; settle 2.0   # ⌥+Shift+; : agent on a NEW strip (strip 2)
ask "Read '$NOTES/Lessons/Gibbs Free-Energy Change.md' and teach me the sign conventions in 5 lines. Do not edit any file."
beat 2.5

chord "$AGENT" 0.2; settle 2.0   # ⌥+; : second agent on this strip
ask "From '$NOTES/5.111 Chemistry Map.md', quiz me on Kp vs Kc, one question at a time. Do not edit any file."
beat 2.2

chord "$AGENT" 0.2; settle 2.0   # third agent
ask "Work '$NOTES/Exam Question - Thermodynamics and Equilibrium.md' in chat and show every step. Do not edit any file."
beat 2.2

chord 's' 0.2; settle 1.0        # ⌥+s: split this strip's column too
ask 'jcode' 1.6
ask "Summarize standard state in thermodynamics from my 5.111 notes. Do not edit any file."
beat 2.2

# --- fast panning: the row is wider than the screen ----------------------
burst ']' 6 0.10                 # viewport slides right, focus untouched
beat 0.5
burst '[' 6 0.10                 # and back
beat 0.5
burst 'l' 4 0.13                 # follow-focus, quantized stops
burst 'h' 4 0.13
beat 0.5

# === STRIP 3 — reference + agent, then the payoff ========================
chord "$AGENT_ROW" 0.2; settle 2.0   # strip 3
ask "Compare Kp and Kc with a worked example from '$NOTES/Lessons'. Do not edit any file."
beat 1.8
chord '\015' 0.6
ask "yazi \"$NOTES/Lessons\"" 1.0
beat 1.2
chord 's' 0.2; settle 1.0        # ⌥+s again, on strip three
ask 'btm --basic' 0.8
beat 1.2
chord 'h' 0.4
chord 'r' 0.5; chord 'r' 0.5     # ⌥+r on strip three as well
chord "$FULL" 1.0                # ⌥+f: full width, read the answer
beat 1.8
chord "$FULL" 0.8                # back to 1/4

# --- three strips, one glance -------------------------------------------
chord "$CARRY_DOWN" 0.9          # ⌥+Shift+j: carry a pane between strips
beat 0.7
chord 'k' 0.45                   # walk up through the strips...
chord 'k' 0.45
chord 'j' 0.45                   # ...and back down
chord 'j' 0.45
beat 0.6
chord "$JUMP" 1.8                # ⌥+g smart-jump: the agent that needs you
chord "$JUMP" 1.8
beat 1.0
chord "$HUD" 3.0                 # ⌥+/ cheat-sheet as the closing card
beat 2.0

if [ "$DRY" = 0 ]; then
  kill -INT "$REC" 2>/dev/null || true
  sleep 2
  # `screencapture` writes a variable-frame-rate file; retiming it without
  # re-rasterising to a constant rate leaves the retimed copy stuttering, so
  # normalise to 60fps on the way through.
  if command -v ffmpeg >/dev/null && [ "$SPEED" != 1 ]; then
    ffmpeg -y -loglevel error -i "$RAW" \
      -filter:v "setpts=PTS/$SPEED,fps=60" -an \
      -c:v libx264 -pix_fmt yuv420p -crf 20 "$OUT"
  else
    cp "$RAW" "$OUT"
  fi
  # ffprobe, not mdls: Spotlight has no metadata for a file this young (and
  # none at all for the h264 mp4), so mdls reports a bare `(null)`.
  dur() { ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$1"; }
  printf 'recorded -> %s (%.1fs, retimed %sx from a %.1fs take)\n' \
    "$OUT" "$(dur "$OUT")" "$SPEED" "$(dur "$RAW")"
fi
