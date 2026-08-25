#!/usr/bin/env bash
# One-command dev loop: build and run with tracing to a file.
#   GWAE_LOG=debug ./scripts/dev.sh
set -euo pipefail

cd "$(dirname "$0")/.."
export RUST_LOG="${GWAE_LOG:-debug}"

cargo build --workspace
echo "gwae doctor:"
RUST_LOG=info cargo run -q -- doctor
