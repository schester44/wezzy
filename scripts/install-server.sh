#!/bin/sh
# Install Wezzy's server on this machine (a Linux box you ssh into).
#
#   curl -fsSL https://raw.githubusercontent.com/schester44/wezzy/main/scripts/install-server.sh | sh
#
# or from the Mac app: ⌘K → "Set up <host>", which pipes this script over ssh.
#
# Downloads `wezzy` for this machine's CPU (x86-64 or ARM64, glibc 2.31+) from
# the `server-<pin>` GitHub release into ~/.local/share/wezzy/server/<pin>/bin,
# links it as ~/.local/bin/wezzy, and starts the server: as a systemd user
# service when there is one (so it survives logout), else in the background.
# With no prebuilt binary for this machine, it builds one from source if Rust
# is installed. Run it again with --force to reinstall.
#
# <pin> is the WezTerm commit the mux protocol comes from; it has to match the
# app's. Keep it in step with cli/src/paths.rs.
set -eu

PIN=e68a8ec
REPO=${WEZZY_REPO:-https://github.com/schester44/wezzy}
BRANCH=${WEZZY_BRANCH:-main}
ROOT="$HOME/.local/share/wezzy/server/$PIN"
BIN="$ROOT/bin/wezzy"

say() { printf '==> %s\n' "$*"; }

RELEASES=${WEZZY_RELEASES:-$REPO/releases/download/server-$PIN}

fetch() { # url file
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
  else return 1; fi
}

download() {
  [ "$(uname -s)" = Linux ] || return 1
  case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) return 1 ;;
  esac
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  name=wezzy-linux-$arch.tar.gz
  say "downloading $name"
  fetch "$RELEASES/$name" "$tmp/$name" || return 1
  fetch "$RELEASES/$name.sha256" "$tmp/$name.sha256" || return 1
  want=$(cut -d' ' -f1 <"$tmp/$name.sha256")
  got=$( (sha256sum "$tmp/$name" 2>/dev/null || shasum -a 256 "$tmp/$name") | cut -d' ' -f1)
  if [ "$want" != "$got" ]; then
    echo "error: $name doesn't match its checksum" >&2
    exit 1
  fi
  mkdir -p "$ROOT/bin"
  tar -xzf "$tmp/$name" -C "$tmp"
  mv -f "$tmp/wezzy" "$BIN"
  chmod +x "$BIN"
}

build() {
  if ! command -v cargo >/dev/null 2>&1; then
    [ -x "$HOME/.cargo/bin/cargo" ] && PATH="$HOME/.cargo/bin:$PATH"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: no prebuilt wezzy for $(uname -sm), and no Rust to build it (https://rustup.rs)" >&2
    exit 1
  fi
  say "building wezzy from source (a few minutes)"
  cargo install --locked --git "$REPO" --branch "$BRANCH" --root "$ROOT" wezzy-cli
}

if [ -x "$BIN" ] && [ "${1:-}" != "--force" ]; then
  say "wezzy is already installed"
else
  download || build
fi

if ! version=$("$BIN" version); then
  echo "error: wezzy doesn't run on this machine; rerun with --force to reinstall" >&2
  exit 1
fi
mkdir -p "$HOME/.local/bin"
ln -sf "$BIN" "$HOME/.local/bin/wezzy"
say "$version"

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  "$BIN" server install
else
  "$BIN" server start
  say "started; without systemd it runs until this machine restarts"
fi
say "done: connect from Wezzy with ⌘K → Connect <host>"
