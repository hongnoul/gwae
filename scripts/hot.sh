#!/usr/bin/env bash
# Hot-reload dev loop: rebuild on save, and the running gwae swaps itself in
# place without losing a single pane.
#
#   ./scripts/hot.sh          # in one terminal: watch + rebuild
#   GWAE_DEV_RELOAD=1 gwae    # in another: the session that reloads itself
#
# gwae is daemon-free, so this is not a server: the running process notices
# that its own binary changed, hands its PTYs to a new image of itself via
# execve, and keeps the same pid. Panes never learn it happened.
#
# Requires `cargo-watch` (cargo install cargo-watch) or falls back to a poll.
set -euo pipefail

cd "$(dirname "$0")/.."

# Where the running gwae is executing from. Rebuilding into the *same* path is
# what triggers the reload, so this must match how the session was launched.
TARGET="${GWAE_BIN:-$(command -v gwae || echo target/release/gwae)}"

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
  echo "[hot] building..."
  if cargo build --release 2>&1 | grep -E '^(error|warning: unused)' ; then
    echo "[hot] build failed; the running session is untouched"
    return 0
  fi
  install_atomically target/release/gwae "$TARGET"
}

# Re-entrant call from cargo-watch: build, install, and get out of the way.
# Checked before the banner so the watch loop's output stays readable.
if [ "${1:-}" = "--build-once" ]; then
  build_once
  exit 0
fi

echo "[hot] watching for changes; running gwae sessions with"
echo "      GWAE_DEV_RELOAD=1 will swap themselves in place."
echo "[hot] target: $TARGET"

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
