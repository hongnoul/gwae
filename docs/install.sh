#!/usr/bin/env bash
# gwae installer: downloads the latest release binary to a bin dir and adds it
# to PATH, so `gwae` works in a fresh terminal right after install.
#   curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
# Fallback: https://raw.githubusercontent.com/hongnoul/gwae/main/scripts/install.sh
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

# The tag is reported after install by running the binary itself (see below):
# the redirect usually lands on release-assets.githubusercontent.com, which
# carries no tag segment, so parsing $final_url printed a useless "latest".
: "${final_url:=}"

# --- checksum -----------------------------------------------------------------
sha_tool="shasum -a 256"
command -v shasum >/dev/null 2>&1 || sha_tool="sha256sum"
if curl -fsSL "https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz.sha256" \
    -o "$tmp/pkg.sha256" 2>/dev/null; then
  expected=$(awk '{print $1}' "$tmp/pkg.sha256")
  actual=$($sha_tool "$tmp/pkg.tar.gz" | awk '{print $1}')
  [ "$expected" = "$actual" ] || die "checksum verification failed"
  say "checksum verified"
else
  # Never fail the install over a missing .sha256, but never pretend either:
  # a silent skip is indistinguishable from a verified download.
  say "warning: could not fetch ${artifact}.tar.gz.sha256; skipping verification"
fi

# --- install ------------------------------------------------------------------
tar xzf "$tmp/pkg.tar.gz" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m755 "$tmp/gwae" "$INSTALL_DIR/gwae"

# Report the version by asking the installed binary, which also proves it
# executes on this machine before the user ever runs it.
version=$("$INSTALL_DIR/gwae" --version 2>/dev/null) \
  || die "installed binary at ${INSTALL_DIR}/gwae does not run on this machine"
say "installed ${version} to ${INSTALL_DIR}/gwae"

# --- receipt -------------------------------------------------------------------
# Record *how* gwae got here, so `gwae upgrade` knows the route instead of
# guessing it from the install path. The path is genuinely ambiguous:
# ~/.local/bin is also where people drop hand-built binaries, and
# /usr/local/bin belongs to Homebrew on Intel macOS and to a distro package
# manager on Linux. Guessing wrong there means telling someone to run a
# command that would fight their package manager.
#
# State, not config: this is machine-written bookkeeping and `gwae` treats a
# missing or stale receipt as "detect from the path", so deleting it is safe.
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/gwae"
if mkdir -p "$state_dir" 2>/dev/null; then
  cat > "$state_dir/install.toml" <<EOF
# Written by gwae's install.sh; read by \`gwae upgrade\`. Safe to delete.
source = "install.sh"
dir = "${INSTALL_DIR}"
version = "${version##* }"
EOF
fi

# --- PATH ----------------------------------------------------------------------
# The install dir means nothing until a new shell can find it, so add it to
# the shell's startup file now instead of printing an export line to copy by
# hand. `GWAE_NO_MODIFY_PATH=1` opts out (CI, scripted setups) and restores
# the old print-the-instructions behavior.
#
# The snippet is guarded on `$PATH`, so sourcing it twice (login + interactive
# files both loading, reinstalls, `gwae upgrade` re-running this script) never
# stacks duplicate entries. Reinstalls rewrite only lines we own (marked
# "added by gwae installer"), so a moved install dir relocates instead of
# duplicating, and anything another tool or a hand edit wrote is left alone.
add_to_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac
  if [ -n "${GWAE_NO_MODIFY_PATH:-}" ]; then
    say "${INSTALL_DIR} is not on your PATH. Add it to your shell profile:"
    say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    return 0
  fi

  own_mark="added by gwae installer"
  # Escape for embedding inside a double-quoted shell string.
  esc_dir="$(printf '%s' "$INSTALL_DIR" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\$/\\$/g' -e 's/`/\\`/g')"
  snippet="case \":\$PATH:\" in *\":$esc_dir:\"*) ;; *) export PATH=\"$esc_dir:\$PATH\" ;; esac  # $own_mark"

  ensure_block() {
    f="$1"
    [ -n "$f" ] && [ -d "$(dirname "$f")" ] || return 0
    touch "$f" 2>/dev/null || return 0
    # Drop lines from a previous run of this installer; keep everything else.
    if grep -q "$own_mark" "$f" 2>/dev/null; then
      grep -v "$own_mark" "$f" > "$f.gwae-tmp" 2>/dev/null && mv "$f.gwae-tmp" "$f"
    fi
    # Already covered another way (brew/cargo shim, hand edit): leave it.
    if grep -qF "$INSTALL_DIR" "$f" 2>/dev/null; then
      touched="$touched $f(already)"
      return 0
    fi
    printf '\n# %s\n%s\n' "$own_mark" "$snippet" >> "$f"
    touched="$touched $f"
  }

  touched=""
  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    *fish*) : ;;
    *zsh*) ensure_block "$HOME/.zshrc" ;;
    *bash*)
      # Bash's split personality needs both: login shells read .bash_profile,
      # interactive non-login shells read .bashrc. The guard makes both safe.
      ensure_block "$HOME/.bashrc"
      ensure_block "$HOME/.bash_profile"
      ;;
    *) ensure_block "$HOME/.profile" ;;
  esac
  # fish_add_path is idempotent and this is our own file in conf.d (never
  # config.fish itself). Written whenever fish is the current shell or is
  # installed — a fish user installing from bash, or switching shells later,
  # is covered — but never creating fish config dirs for users without fish.
  fish_shell=0
  case "$shell_name" in *fish*) fish_shell=1 ;; esac
  if [ "$fish_shell" = 1 ] || command -v fish >/dev/null 2>&1; then
    if mkdir -p "$HOME/.config/fish/conf.d" 2>/dev/null; then
      printf '# %s\nfish_add_path "%s"\n' "$own_mark" "$INSTALL_DIR" \
        > "$HOME/.config/fish/conf.d/gwae.fish"
      touched="$touched $HOME/.config/fish/conf.d/gwae.fish"
    fi
  fi

  export PATH="$INSTALL_DIR:$PATH"
  if [ -n "$touched" ]; then
    say "added ${INSTALL_DIR} to PATH in:${touched} — restart your terminal to use it"
    say "(this shell already has it for now)"
  else
    say "${INSTALL_DIR} is not on your PATH and no shell profile was writable. Add it by hand:"
    say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi
}

add_to_path

say "run 'gwae' to start, or 'gwae init' for the guided setup."
say "later: 'gwae upgrade' moves you to the next release the same way."
