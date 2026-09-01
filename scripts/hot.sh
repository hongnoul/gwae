#!/usr/bin/env bash
# Hot-reload dev loop: rebuild on save, and the running gwae swaps itself in
# place without losing a single pane.
#
#   ./scripts/hot.sh              # watch only (legacy: 2 terminals)
#   ./scripts/hot.sh --run        # npm run dev: watch bg + run fg with hot reload
#   GWAE_DEV_RELOAD=1 gwae        # in another: the session that reloads itself
#   GWAE_PROFILE=debug ./scripts/hot.sh --run   # fast debug build (default for --run)
#   GWAE_PROFILE=release ./scripts/hot.sh --run # prod-like release build
#   make dev                      # one-command alias for --run (debug)
#
# gwae is daemon-free, so this is not a server: the running process notices
# that its own binary changed, hands its PTYs to a new image of itself via
# execve, and keeps the same pid. Panes never learn it happened.
#
# Requires `cargo-watch` (cargo install cargo-watch) or falls back to a poll.
set -euo pipefail

cd "$(dirname "$0")/.."

# Profile-aware build: debug is ~5s, release is ~60s.
# For plain watch mode keep release as default (backward compat).
# For --run (npm run dev) default to debug unless GWAE_PROFILE is set.
resolve_profile() {
  local p="${GWAE_PROFILE:-release}"
  if [ "$p" = "dev" ]; then p="debug"; fi
  echo "$p"
}

cargo_args_for() {
  case "$1" in
    debug) echo "" ;;
    *) echo "--release" ;;
  esac
}

src_for() {
  case "$1" in
    debug) echo "target/debug/gwae" ;;
    *) echo "target/release/gwae" ;;
  esac
}

# Where the running gwae is executing from. Rebuilding into the *same* path is
# what triggers the reload, so this must match how the session was launched.
# For --run this is forced to the build output (so cargo run path == watch target).
DEFAULT_TARGET="$(command -v gwae 2>/dev/null || echo target/release/gwae)"
TARGET="${GWAE_BIN:-$DEFAULT_TARGET}"

# Install atomically: write a new file, sign it, rename it into place.
#
# The rename is not a style choice. On macOS, overwriting a Mach-O in place
# invalidates its code signature, and the kernel then SIGKILLs the process
# mid-execve rather than failing cleanly, which would take the whole session
# down. A fresh inode plus an ad-hoc signature avoids that entirely.
install_atomically() {
  local src="$1" dst="$2"
  cp "$src" "$dst.new"
  chmod 755 "$dst.new"
  if command -v codesign >/dev/null 2>&1; then
    codesign -f -s - "$dst.new" >/dev/null 2>&1 || true
  fi
  mv -f "$dst.new" "$dst"
  echo "  -> installed $dst"
}

build_once() {
  local profile args src
  profile="$(resolve_profile)"
  args="$(cargo_args_for "$profile")"
  src="$(src_for "$profile")"
  echo "[hot] building ($profile)..."
  # pipe through tee but preserve cargo's exit status via PIPESTATUS
  set +e
  cargo build $args 2>&1 | tee /tmp/gwae-hot-build.log
  local st=${PIPESTATUS[0]}
  set -e
  if [ "$st" -ne 0 ]; then
    echo "[hot] build failed (exit $st); the running session is untouched"
    return 0
  fi
  if grep -qE '^error' /tmp/gwae-hot-build.log; then
    echo "[hot] build failed; the running session is untouched"
    return 0
  fi
  if [ ! -f "$src" ]; then
    echo "[hot] build failed (no $src); the running session is untouched"
    return 0
  fi
  install_atomically "$src" "$TARGET"
}

# Re-entrant call from cargo-watch: build, install, and get out of the way.
# Checked before the banner so the watch loop's output stays readable.
if [ "${1:-}" = "--build-once" ]; then
  build_once
  exit 0
fi

# npm run dev equivalent: build once, watch in background, run gwae in foreground.
# Single terminal, async: save -> rebuild -> execve in place, jcode pane keeps same pid.
if [ "${1:-}" = "--run" ]; then
  shift
  # For --run default to debug for speed if caller didn't choose.
  if [ -z "${GWAE_PROFILE:-}" ]; then
    export GWAE_PROFILE=debug
  fi
  profile="$(resolve_profile)"
  src="$(src_for "$profile")"
  # Force TARGET to the build output so watcher and runner agree, unless user
  # explicitly set GWAE_BIN to an external path (e.g. ~/.local/bin/gwae).
  if [ -z "${GWAE_BIN:-}" ]; then
    TARGET="$src"
    export GWAE_BIN="$TARGET"
  fi
  export GWAE_BIN TARGET
  echo "[hot] npm run dev: profile=$profile src=$src target=$TARGET"
  build_once
  echo "[hot] starting watcher in background..."
  # Start watcher in background; it will call --build-once on changes.
  (
    if command -v cargo-watch >/dev/null 2>&1; then
      exec cargo watch -w crates -s "$0 --build-once"
    else
      echo "[hot] cargo-watch not found; polling every 2s (cargo install cargo-watch for instant)"
      last=""
      while true; do
        now=$(find crates -name '*.rs' -newer "$TARGET" -print -quit 2>/dev/null || true)
        if [ -n "$now" ] && [ "$now" != "$last" ]; then
          "$0" --build-once
          last="$now"
        fi
        sleep 2
      done
    fi
  ) &
  WATCH_PID=$!
  echo "[hot] watcher pid $WATCH_PID; launching gwae with GWAE_DEV_RELOAD=1"
  echo "[hot] save a file -> rebuild -> gwae swaps in place (same pid, panes alive)"
  # Must not exec (which would discard the trap and orphan the watcher);
  # run gwae as a child and reap the watcher when it exits.
  cleanup() {
    echo "[hot] stopping watcher $WATCH_PID"
    kill "$WATCH_PID" 2>/dev/null || true
    wait "$WATCH_PID" 2>/dev/null || true
  }
  trap cleanup INT TERM EXIT
  # Give watcher a moment to settle before exec so initial mtime is stable
  sleep 0.2
  set +e
  GWAE_DEV_RELOAD=1 "$TARGET" "$@"
  STATUS=$?
  set -e
  trap - INT TERM EXIT
  cleanup
  exit $STATUS
fi

echo "[hot] watching for changes; running gwae sessions with"
echo "      GWAE_DEV_RELOAD=1 will swap themselves in place."
echo "[hot] target: $TARGET (GWAE_PROFILE=${GWAE_PROFILE:-release})"

if command -v cargo-watch >/dev/null 2>&1; then
  # `-s` so the whole install step runs, not just cargo.
  exec cargo watch -w crates -s "$0 --build-once"
fi

echo "[hot] cargo-watch not found; polling every 2s (cargo install cargo-watch)"
last=""
while true; do
  now=$(find crates -name '*.rs' -newer "$TARGET" -print -quit 2>/dev/null || true)
  if [ -n "$now" ] && [ "$now" != "$last" ]; then
    build_once
    last="$now"
  fi
  sleep 2
done
