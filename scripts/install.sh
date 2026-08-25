#!/usr/bin/env bash
# gwae installer: downloads the latest release binary to a bin dir on PATH.
#   curl -fsSL https://raw.githubusercontent.com/hongnoul/gwae/main/scripts/install.sh | bash
set -euo pipefail

REPO="hongnoul/gwae"
INSTALL_DIR="${GWAE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '\033[1;36mgwae:\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mgwae:\033[0m %s\n' "$*" >&2; exit 1; }

# --- platform ---------------------------------------------------------------
case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux)  os=unknown-linux-musl ;;
  *) die "unsupported OS $(uname -s). On Windows, download gwae-x86_64-pc-windows-msvc.zip from https://github.com/${REPO}/releases/latest — or build from source: cargo install --git https://github.com/${REPO} gwae" ;;
esac

case "$(uname -m)" in
  x86_64)          arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported architecture $(uname -m). Build from source: cargo install --git https://github.com/${REPO} gwae" ;;
esac
target="${arch}-${os}"
artifact="gwae-${target}"

# --- download ----------------------------------------------------------------
# The /releases/latest/download/ redirect avoids api.github.com rate limits
# (60/hr per IP unauthenticated), which bite on shared networks.
url="https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "downloading ${artifact} (latest release)..."
final_url=$(curl -fsSL -o "$tmp/pkg.tar.gz" -w '%{url_effective}' "$url") \
  || die "download failed: $url"

# Recover the tag from the resolved asset URL for the install message. The
# redirect may land on release-assets.githubusercontent.com, so fall back
# gracefully when no tag segment is present.
tag=$(printf '%s\n' "$final_url" | grep -o '/releases/download/[^/]*/' | cut -d/ -f4 || true)
[ -n "$tag" ] || tag="latest"

# --- checksum -----------------------------------------------------------------
sha_tool="shasum -a 256"
command -v shasum >/dev/null 2>&1 || sha_tool="sha256sum"
if curl -fsSL "https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz.sha256" \
    -o "$tmp/pkg.sha256" 2>/dev/null; then
  expected=$(awk '{print $1}' "$tmp/pkg.sha256")
  actual=$($sha_tool "$tmp/pkg.tar.gz" | awk '{print $1}')
  [ "$expected" = "$actual" ] || die "checksum verification failed"
fi

# --- install ------------------------------------------------------------------
tar xzf "$tmp/pkg.tar.gz" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m755 "$tmp/gwae" "$INSTALL_DIR/gwae"

say "installed gwae ${tag} to ${INSTALL_DIR}/gwae"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    say "${INSTALL_DIR} is not on your PATH. Add it to your shell profile:"
    say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac

say "run 'gwae' to start, or 'gwae init' for the guided setup."
