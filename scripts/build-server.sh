#!/bin/bash
# Build `wezzy` (cli/) for Linux hosts, x86-64 and ARM64, in Docker.
#
#   scripts/build-server.sh            # both
#   scripts/build-server.sh aarch64    # one
#   FORCE=1 scripts/build-server.sh    # even if up to date
#
# Writes target/server/wezzy-linux-<arch>.tar.gz (+ .sha256), which is what
# scripts/install-server.sh downloads from the `server-<pin>` GitHub release.
# Publish them with:
#
#   gh release create server-<pin> target/server/* --title "Wezzy server (wezterm <pin>)"
#
# Built on Debian bullseye (glibc 2.31): runs on Ubuntu 20.04+, Debian 11+ and
# anything newer. OpenSSL is built in (cli/Cargo.toml), so glibc is all it
# needs; the build fails if a system libssl sneaks in.
set -euo pipefail
cd "$(dirname "$0")/.."

PIN=$(grep -m1 -o 'WEZTERM_REV: &str = "[0-9a-f]*"' cli/src/paths.rs | cut -d'"' -f2)
IMAGE=${IMAGE:-rust:1-bullseye}
OUT=target/server
ARCHES=("${@:-x86_64 aarch64}")
mkdir -p "$OUT"

for arch in ${ARCHES[@]}; do
  case $arch in
    x86_64) platform=linux/amd64 ;;
    aarch64) platform=linux/arm64 ;;
    *) echo "unknown arch $arch" >&2; exit 1 ;;
  esac
  tarball=$OUT/wezzy-linux-$arch.tar.gz
  # fresh: nothing in cli/ (or the pinned deps) changed since it was built
  if [[ -f $tarball && -z ${FORCE:-} && -z $(find cli -path cli/target -prune -o -newer "$tarball" -type f -print -quit) ]]; then
    echo "==> linux-$arch is up to date"
    continue
  fi
  echo "==> wezzy for linux-$arch (wezterm $PIN)"
  docker run --rm --platform "$platform" \
    -v "$PWD":/src -w /src \
    -v "wezzy-cargo-$arch:/usr/local/cargo/registry" \
    -v "wezzy-cargo-git-$arch:/usr/local/cargo/git" \
    -e CARGO_TARGET_DIR="/src/target/linux-$arch" \
    "$IMAGE" \
    sh -c 'apt-get purge -y -qq libssl-dev >/dev/null 2>&1; cargo build --release --locked --manifest-path cli/Cargo.toml'
  # OpenSSL must be built in (no system libssl on the build image to fall back to)
  if docker run --rm --platform "$platform" -v "$PWD/target/linux-$arch/release":/b:ro "$IMAGE" \
    ldd /b/wezzy | grep -E 'libssl|libcrypto'; then
    echo "error: wezzy links a system OpenSSL" >&2
    exit 1
  fi
  bin=target/linux-$arch/release/wezzy
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$OUT/wezzy-linux-$arch.tar.gz" -C "$(dirname "$bin")" wezzy
  (cd "$OUT" && shasum -a 256 "wezzy-linux-$arch.tar.gz" > "wezzy-linux-$arch.tar.gz.sha256")
  echo "==> $OUT/wezzy-linux-$arch.tar.gz"
done
