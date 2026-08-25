#!/usr/bin/env bash
# Record the gwae/strimux "fire demo": fast panning navigation, btm + yazi and
# a swarm of jcode agents studying MIT 5.111 chemistry, across three strips.
#
# The whole take is driven through kitty remote control (`kitten @ send-text`),
# so it is deterministic and repeatable: nobody touches the keyboard while
# `screencapture -v` rolls. Re-run until a take is good; tweak the beats, not
# your fingers.
#
#   scripts/demo_record.sh              # -> ~/Desktop/gwae-demo.mov
#   OUT=/tmp/take2.mov scripts/demo_record.sh
#   DRY=1 scripts/demo_record.sh        # drive the UI, record nothing
#
# Chord encoding: every strimux chord goes out as Option-as-Meta, i.e. ESC +
# key, and shifted chords as ESC + the *shifted* character (ESC ':' for
# ⌥+Shift+;, ESC 'J' for ⌥+Shift+j). The macOS Unicode-glyph fallback strimux
# also decodes (Ú, Ô, ©, ƒ, ÷) is deliberately NOT used here: over remote
# control those bytes reach the focused pane as ordinary text and get typed
# into the agent's prompt instead of steering the grid.
set -euo pipefail

BIN=${BIN:-$HOME/.cargo/bin/strimux}
OUT=${OUT:-$HOME/Desktop/gwae-demo.mov}
SOCK=${SOCK:-/tmp/gwae-demo-rc}
FONT=${FONT:-13}
NOTES=${NOTES:-$HOME/Documents/obsidian/MIT 5.111}
KITTY=/Applications/kitty.app/Contents/MacOS/kitty
KITTEN=/Applications/kitty.app/Contents/MacOS/kitten
DRY=${DRY:-0}

# --- key DSL ---------------------------------------------------------------
raw()   { printf '%b' "$1" | "$KITTEN" @ --to "unix:$SOCK" send-text --stdin; }
chord() { raw "\033$1"; sleep "${2:-0.35}"; }          # ⌥+key
# No trailing newline: a here-string would append one, submitting the line a
# beat before `enter` does and sending a *second* Return into whatever the
# command just launched (that stray Return is how yazi ends up opening a file
# in vim mid-take).
lit()   { printf '%s' "$1" | "$KITTEN" @ --to "unix:$SOCK" send-text --stdin; }
enter() { raw '\015'; sleep "${1:-0.4}"; }
beat()  { sleep "${1:-0.8}"; }
burst() { local key=$1 n=$2 gap=${3:-0.12}; for _ in $(seq "$n"); do raw "\033$key"; sleep "$gap"; done; }
# A freshly spawned pane needs its shell (or agent TUI) to finish drawing
# before it will accept a line: type too early and the first characters land
# in the previous pane or get eaten by the redraw, which is exactly how a take
# ends up with `resubtm` on screen. Settle, then type, then submit.
ask()   { sleep "${3:-1.2}"; lit "$1"; sleep 0.6; enter "${2:-0.6}"; }

AGENT_ROW=':'   # ⌥+Shift+;  agent on a new strip below
AGENT=';'       # ⌥+;        agent right of focus
CARRY_DOWN='J'  # ⌥+Shift+j  carry pane to the strip below
JUMP='g'        # ⌥+g        smart-jump
FULL='f'        # ⌥+f        full width toggle
HUD='/'         # ⌥+/        cheat-sheet

cleanup() { pkill -f "instance-group gwae-demo" 2>/dev/null || true; rm -f "$SOCK"; }

# --- stage -----------------------------------------------------------------
pkill -f "instance-group gwae-demo" 2>/dev/null || true; sleep 0.5
trap cleanup EXIT
nohup "$KITTY" -o allow_remote_control=yes --listen-on "unix:$SOCK" \
  --instance-group gwae-demo --title GWAE-DEMO -o font_size="$FONT" \
  --start-as fullscreen "$BIN" >/tmp/gwae-demo-kitty.log 2>&1 &
sleep 4
"$KITTEN" @ --to "unix:$SOCK" ls >/dev/null    # fail fast if RC never came up

if [ "$DRY" = 0 ]; then
  rm -f "$OUT"
  screencapture -v -C -x "$OUT" &
  REC=$!
  trap 'kill -INT $REC 2>/dev/null || true; sleep 2; cleanup' EXIT
  sleep 2
fi

# === STRIP 1 — the workbench: btm, yazi, width that never shrinks ==========
beat 1.2
chord '\015' 0.6                 # ⌥+Enter: new column
ask 'btm'                        # system monitor next to the agent
beat 2.0
chord '\015' 0.6                 # ⌥+Enter: new column
ask "yazi \"$NOTES\"" 1.0        # the 5.111 vault, browsable
beat 1.4
burst 'j' 4 0.20                 # yazi owns its own keys — strimux forwards
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
chord 's' 0.9
ask "while true; date +%T; uptime; sleep 1; clear; end" 0.8   # fish: a poor man's watch(1); macOS has none
beat 1.4
chord 'k' 0.4                    # up to btm
chord 'j' 0.4                    # down to the split
chord 'l' 0.4                    # over to yazi
beat 0.6

# === STRIP 2 — a study swarm on 5.111 ====================================
chord "$AGENT_ROW" 1.6           # ⌥+Shift+; : agent on a NEW strip (strip 2)
ask "Read '$NOTES/Lessons/Gibbs Free-Energy Change.md' and teach me the sign conventions in 5 lines."
beat 2.5

chord "$AGENT" 1.4               # ⌥+; : second agent on this strip
ask "From '$NOTES/5.111 Chemistry Map.md', quiz me on Kp vs Kc, one question at a time."
beat 2.2

chord "$AGENT" 1.4               # third agent
ask "Work '$NOTES/Exam Question - Thermodynamics and Equilibrium.md' and show every step."
beat 2.2

chord 's' 0.9                    # ⌥+s: split this strip's column too
ask 'jcode' 1.6
ask "Summarize standard state in thermodynamics from my 5.111 notes."
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
chord "$AGENT_ROW" 1.6           # strip 3
ask "Compare Kp and Kc with a worked example from '$NOTES/Lessons'."
beat 1.8
chord '\015' 0.6
ask "yazi \"$NOTES/Lessons\"" 1.0
beat 1.2
chord 's' 0.9                    # ⌥+s again, on strip three
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
  echo "recorded -> $OUT"
fi
