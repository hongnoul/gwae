#!/usr/bin/env bash
# Develop strimux with hot module reload.
#
# Run this in one pane and `strimux-hmr` in another (both inside strimux).
# Every time a file under crates/strimux-core/src changes, this rebuilds just
# the core dylib; the running strimux-hmr notices the new dylib and hot-swaps
# it in while keeping host-owned session state (focus, layout) intact.
#
# No external watcher binary required: a cheap `find -newer` poll drives it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO="${CARGO:-cargo}"
CORE_SRC="$ROOT/crates/strimux-core/src"
PROFILE="${PROFILE:-debug}"
DYLIB="$ROOT/target/$PROFILE/libstrimux_core.dylib"

echo "[hmr] watching $CORE_SRC -> $DYLIB (Ctrl-C to stop)"
while true; do
  if [ -n "$(find "$CORE_SRC" -type f -newer "$DYLIB" 2>/dev/null | head -1)" ]; then
    echo "[hmr] source changed, rebuilding core ..."
    if out="$("$CARGO" build -p strimux-core 2>&1)"; then
      echo "$out" | tail -2
      echo "[hmr] rebuilt; strimux-hmr will hot-reload on next tick"
    else
      echo "$out" | tail -5
      echo "[hmr] build failed; keeping the running core"
    fi
  fi
  sleep 0.7
done
