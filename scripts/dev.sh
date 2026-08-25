#!/usr/bin/env bash
# One-command dev loop: build and run with tracing to a file.
#   STRIMUX_LOG=debug ./scripts/dev.sh
set -euo pipefail

cd "$(dirname "$0")/.."
export RUST_LOG="${STRIMUX_LOG:-debug}"

cargo build --workspace
echo "strimux doctor:"
RUST_LOG=info cargo run -q -- doctor
