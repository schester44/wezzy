#!/usr/bin/env bash
# Cut a release: build, sign, notarize, tag, and upload a draft GitHub release.
#
#   1. bump `version` in Cargo.toml, commit, push
#   2. scripts/release.sh
#   3. install the .dmg from the draft, check it opens cleanly, then publish
#      the draft. Installed copies update once it's published (drafts are
#      invisible to the updater).
#
# Same env as scripts/bundle.sh --dist (NOTARY_PROFILE, SIGN_IDENTITY, RELEASE_REPO).
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
TAG=v$VERSION
RELEASE_REPO=${RELEASE_REPO:-schester44/wezzy}
export RELEASE_REPO

fail() { echo "error: $*" >&2; exit 1; }

command -v gh >/dev/null || fail "needs the gh CLI"
[[ -z $(git status --porcelain) ]] || fail "working tree isn't clean"
git fetch -q origin
[[ $(git rev-parse HEAD) == $(git rev-parse '@{u}' 2>/dev/null) ]] \
  || fail "HEAD isn't pushed to its upstream"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && fail "$TAG already exists; bump the version in Cargo.toml"
gh release view "$TAG" -R "$RELEASE_REPO" >/dev/null 2>&1 && fail "release $TAG already exists on $RELEASE_REPO"

scripts/bundle.sh --dist

DIR=target/release/dist
# stable name, so https://github.com/<repo>/releases/latest/download/Wezzy.dmg always works
cp "$DIR/Wezzy-$VERSION.dmg" "$DIR/Wezzy.dmg"

echo "==> tagging $TAG"
git tag -a "$TAG" -m "Wezzy $VERSION"
git push -q origin "$TAG"

echo "==> draft release $TAG on $RELEASE_REPO"
gh release create "$TAG" -R "$RELEASE_REPO" --draft --verify-tag \
  --title "Wezzy $VERSION" --generate-notes \
  "$DIR/Wezzy-$VERSION.dmg" "$DIR/Wezzy.dmg" "$DIR/Wezzy-$VERSION.zip" "$DIR/latest.json"

echo "==> drafted. Test $DIR/Wezzy-$VERSION.dmg, then publish:"
echo "    gh release edit $TAG -R $RELEASE_REPO --draft=false"
