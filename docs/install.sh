#!/usr/bin/env bash
# gwae installer (curl fallback; Homebrew is the primary install).
#   curl -fsSL https://hongnoul.github.io/gwae/install.sh | bash
# Fallback: https://raw.githubusercontent.com/hongnoul/gwae/main/scripts/install.sh
#
# Installs into the first writable `*bin` dir already on PATH (preferring
# `~/.local/bin`), so `gwae` works in this terminal and every fresh one with
# no profile change and no paste. Only when no PATH dir is writable does it
# fall back to `~/.local/bin` and persist a guarded PATH snippet to your
# shell profile, printing the one-line export that activates it here.
# Override with GWAE_INSTALL_DIR. GWAE_NO_MODIFY_PATH=1
# opts out of profile writes (CI, scripted setups) and prints instructions.
set -euo pipefail

REPO="hongnoul/gwae"

# Styling, gated the standard way (clig.dev / no-color.org): plain output
# when stdout is not a tty, when NO_COLOR is set and non-empty, or when
# TERM=dumb. `curl | bash` keeps stdout on the terminal, so an interactive
# install gets color while `bash install.sh > log` stays grep-clean.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
  C_BOLD=$'\033[1m' C_DIM=$'\033[2m' C_GREEN=$'\033[32m' C_RED=$'\033[31m' C_RESET=$'\033[0m'
else
  C_BOLD='' C_DIM='' C_GREEN='' C_RED='' C_RESET=''
fi

say()  { printf '  %s\n' "$*"; }
ok()   { printf '  %s✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
die()  { printf '  %s✗ %s%s\n' "$C_RED" "$*" "$C_RESET" >&2; exit 1; }

banner() {
  printf '\n%s  ▄▄▄▄ ▄ ▄%s\n%s   ▄ █ █▄█%s   %sgwae installer%s\n%s   █ █ █ █%s   %spanes that never shrink%s\n%s   █ ▀ █ █%s   %sgithub.com/hongnoul/gwae%s\n%s  ▀▀▀▀ ▀ ▀%s\n\n' \
    "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET" \
    "$C_BOLD" "$C_RESET" "$C_DIM" "$C_RESET" \
    "$C_BOLD" "$C_RESET" "$C_DIM" "$C_RESET" \
    "$C_BOLD" "$C_RESET"
}

# Where to put the binary. Explicit `GWAE_INSTALL_DIR` always wins (upgrades
# pin it so a re-run cannot relocate the binary). Otherwise pick the first
# writable `*bin` dir already on PATH, so `gwae` works in this terminal and
# every fresh one with no profile change and no paste. Only when no PATH dir
# is writable do we fall back to `~/.local/bin` and set up PATH below.
if [ -n "${GWAE_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$GWAE_INSTALL_DIR"
else
  INSTALL_DIR=""
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) INSTALL_DIR="$HOME/.local/bin" ;;
  esac
  if [ -z "$INSTALL_DIR" ]; then
    _rest="$PATH"
    while [ -n "$_rest" ]; do
      _d="${_rest%%:*}"
      if [ "$_rest" = "$_d" ]; then _rest=""; else _rest="${_rest#*:}"; fi
      case "$_d" in *bin) ;; *) continue ;; esac
      # Never claim system locations even when writable (sudo): they belong
      # to the OS or a package manager, and overwriting there fights it.
      case "$_d" in /usr/bin|/bin|/usr/sbin|/sbin|/System/*) continue ;; esac
      if [ -d "$_d" ] && [ -w "$_d" ]; then INSTALL_DIR="$_d"; break; fi
    done
    unset _rest _d
  fi
  [ -n "$INSTALL_DIR" ] || INSTALL_DIR="$HOME/.local/bin"
fi

# --- platform (macOS only) ----------------------------------------------------
banner
case "$(uname -s)" in
  Darwin) os=macos ;;
  *) die "gwae is macOS-only. On a Mac: brew install hongnoul/tap/gwae" ;;
esac

case "$(uname -m)" in
  x86_64)          arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac
ok "detected ${os}/${arch}"
target="${arch}-apple-darwin"
artifact="gwae-${target}"

# --- download -----------------------------------------------------------------
# The /releases/latest/download/ redirect avoids api.github.com rate limits
# (60/hr per IP unauthenticated), which bite on shared networks.
url="https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "fetching latest release manifest..."
manifest_ver=$(curl -fsSL "https://github.com/${REPO}/releases/latest" -o /dev/null -w '%{url_effective}' 2>/dev/null | grep -o '[^/]*$' || true)
[ -n "${manifest_ver:-}" ] && say "downloading ${manifest_ver}..."
# On a tty, show curl's own progress bar (the X-of-Y KB display users
# expect from a download); piped or logged runs stay silent as before.
if [ -t 1 ]; then curl_progress="--progress-bar"; else curl_progress="-s"; fi
curl -fSL $curl_progress -o "$tmp/pkg.tar.gz" "$url" \
  || die "download failed: $url"
ok "downloaded ${manifest_ver:-latest release}"

# --- checksum (required: every release ships a .sha256) ------------------------
sha_tool="shasum -a 256"
command -v shasum >/dev/null 2>&1 || sha_tool="sha256sum"
if curl -fsSL "https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz.sha256" \
    -o "$tmp/pkg.sha256" 2>/dev/null; then
  expected=$(awk '{print $1}' "$tmp/pkg.sha256")
  actual=$($sha_tool "$tmp/pkg.tar.gz" | awk '{print $1}')
  [ "$expected" = "$actual" ] || die "checksum verification failed"
  ok "checksum verified"
else
  die "could not fetch ${artifact}.tar.gz.sha256; refusing to install without verification"
fi

# --- install (atomic: tmp file -> ad-hoc sign -> rename) -----------------------
# The staging name is per-process: two concurrent installs (two terminals,
# an upgrade racing a reinstall) must not share one `gwae.new`.
tar xzf "$tmp/pkg.tar.gz" -C "$tmp"
[ -f "$tmp/gwae" ] || die "archive did not contain a gwae binary"
mkdir -p "$INSTALL_DIR"
tmp_bin="$INSTALL_DIR/gwae.new.$$"
trap "rm -rf \"\$tmp\" \"$tmp_bin\"" EXIT
cp "$tmp/gwae" "$tmp_bin"
chmod 755 "$tmp_bin"
if command -v codesign >/dev/null 2>&1; then
  codesign -f -s - "$tmp_bin" >/dev/null 2>&1 || true
fi
mv -f "$tmp_bin" "$INSTALL_DIR/gwae"
trap 'rm -rf "$tmp"' EXIT

# Report the version by asking the installed binary, which also proves it
# executes on this machine before the user ever runs it.
version=$("$INSTALL_DIR/gwae" --version 2>/dev/null) \
  || die "installed binary at ${INSTALL_DIR}/gwae does not run on this machine"
ok "installed ${version##* } to ${INSTALL_DIR}/gwae"

# A Homebrew gwae elsewhere on PATH would now be shadowed (or shadow this
# install): say which one wins so `gwae --version` never surprises anyone.
if command -v brew >/dev/null 2>&1 && brew list gwae >/dev/null 2>&1; then
  case ":$PATH:" in
    *":$INSTALL_DIR:"*)
      first_gwae="$(command -v gwae 2>/dev/null || true)"
      if [ -n "$first_gwae" ] && [ "$first_gwae" != "$INSTALL_DIR/gwae" ]; then
        say "note: Homebrew also provides gwae, and ${first_gwae} wins on your current PATH."
        say "this install (${INSTALL_DIR}/gwae) takes effect in terminals where ${INSTALL_DIR} comes first."
      fi
      ;;
  esac
fi

# --- receipt ------------------------------------------------------------------
# Record *how* gwae got here, so `gwae upgrade` knows the route instead of
# guessing it from the install path. ~/.local/bin is genuinely ambiguous
# (people drop hand-built binaries there too), and guessing wrong means
# telling someone to run a command that fights their package manager.
#
# State, not config: machine-written bookkeeping; `gwae` treats a missing or
# stale receipt as "detect from the path", so deleting it is safe.
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/gwae"
if mkdir -p "$state_dir" 2>/dev/null; then
  cat > "$state_dir/install.toml" <<EOF
# Written by gwae's install.sh; read by \`gwae upgrade\`. Safe to delete.
source = "install.sh"
dir = "${INSTALL_DIR}"
version = "${version##* }"
EOF
fi

# --- PATH ---------------------------------------------------------------------
# A piped `curl | bash` runs in a child shell, so `export PATH` here cannot
# reach the user's terminal. What *does* reach it: the profile files below
# (fresh terminals) plus the exact export line we print (this terminal, one
# paste). When the dir is already on PATH there is nothing to do and `gwae`
# just works.
#
# The snippet is guarded on `$PATH`, so sourcing it twice (login +
# interactive files both loading, reinstalls) never stacks duplicates.
# Reinstalls rewrite only lines we own (marked "added by gwae installer"), so
# a moved install dir relocates instead of duplicating, and anything another
# tool or a hand edit wrote is left alone.
add_to_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*)
      ok "${INSTALL_DIR} is already on your PATH"
      return 0
      ;;
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

  touched=""
  ensure_block() {
    f="$1"
    [ -n "$f" ] && [ -d "$(dirname "$f")" ] || return 0
    touch "$f" 2>/dev/null || return 0
    # Drop lines from a previous run of this installer; keep everything else.
    if grep -q "$own_mark" "$f" 2>/dev/null; then
      grep -v "$own_mark" "$f" > "$f.gwae-tmp" 2>/dev/null && mv "$f.gwae-tmp" "$f"
    fi
    # Already covered another way (hand edit, another tool): leave it.
    if grep -qF "$INSTALL_DIR" "$f" 2>/dev/null; then
      touched="$touched $f(already)"
      return 0
    fi
    printf '\n# %s\n%s\n' "$own_mark" "$snippet" >> "$f"
    touched="$touched $f"
  }

  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    *fish*) : ;;
    *zsh*) ensure_block "$HOME/.zshrc" ;;
    *bash*)
      # Bash reads .bash_profile for login shells, .bashrc for interactive
      # non-login shells; POSIX sh reads .profile, which bash login shells
      # also fall back to when no bash-specific file exists. The guard makes
      # writing all three safe.
      ensure_block "$HOME/.bashrc"
      ensure_block "$HOME/.bash_profile"
      ensure_block "$HOME/.profile"
      ;;
    *) ensure_block "$HOME/.profile" ;;
  esac
  # fish_add_path is idempotent and this is our own file in conf.d (never
  # config.fish itself). Written whenever fish is the current shell or is
  # installed, but never creating fish config dirs for users without fish.
  fish_shell=0
  case "$shell_name" in *fish*) fish_shell=1 ;; esac
  if [ "$fish_shell" = 1 ] || command -v fish >/dev/null 2>&1; then
    if mkdir -p "$HOME/.config/fish/conf.d" 2>/dev/null; then
      printf '# %s\nfish_add_path "%s"\n' "$own_mark" "$INSTALL_DIR" \
        > "$HOME/.config/fish/conf.d/gwae.fish"
      touched="$touched $HOME/.config/fish/conf.d/gwae.fish"
    fi
  fi

  if [ -n "$touched" ]; then
    ok "added ${INSTALL_DIR} to PATH in:${touched}"
    say "use it in this terminal now with:"
    say "  ${C_BOLD}export PATH=\"${INSTALL_DIR}:\$PATH\"${C_RESET}"
    say "${C_DIM}(fresh terminals pick it up automatically)${C_RESET}"
  else
    say "${INSTALL_DIR} is not on your PATH and no shell profile was writable. Add it by hand:"
    say "  ${C_BOLD}export PATH=\"${INSTALL_DIR}:\$PATH\"${C_RESET}"
  fi
}

add_to_path

printf '\n  %s✓ welcome to gwae.%s type %sgwae%s and you'"'"'re in.\n\n' "$C_GREEN" "$C_RESET" "$C_BOLD" "$C_RESET"
