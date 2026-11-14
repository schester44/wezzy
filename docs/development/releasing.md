# Releasing

Releases are cut by hand from a Mac with the signing setup below. Pushing to
`main` never releases anything. Moving this to GitHub Actions is written up in
[Releases from CI](../roadmap/ci-releases.md).

## Checklist

```sh
# 1. version: bump `version` in Cargo.toml (skip for the first, untagged 0.1.0)
cargo build && cargo test          # warning-free, green
git add -A && git commit -m "Release 0.2.0"
git push

# 2. build, sign, notarize, tag, draft
scripts/release.sh

# 3. install target/release/dist/Wezzy-<v>.dmg, check it opens cleanly, then
gh release edit v<version> --draft=false
```

Notarization runs twice, once for the app and once for the DMG, and waits for
Apple each time. The first release on a machine also builds
`wezterm-mux-server` from source into `target/mux-server/<rev>/`. Set
`MUX_FROM_WEZTERM=1` to copy it from `/Applications/WezTerm.app` instead, but
only if that WezTerm is the pinned commit.

### What `scripts/release.sh` does

It stops before building if:

- the working tree isn't clean (commit or stash everything, untracked files
  included);
- `HEAD` isn't pushed to its upstream;
- tag `v<version>` or a GitHub release for it already exists (bump the
  version).

Then it:

1. runs `scripts/bundle.sh --dist`: release build, the bundle assembled with
   `wezterm-mux-server`, both signed with the Developer ID certificate, the
   hardened runtime and `assets/macos/entitlements.plist`; the app notarized
   and stapled, then a signed, notarized, stapled DMG, checked with `spctl`;
2. writes `target/release/dist/`: `Wezzy-<v>.dmg`, `Wezzy.dmg` (a stable name
   for the download link), `Wezzy-<v>.zip` (what the updater installs) and
   `latest.json`;
3. tags `v<version>` and pushes the tag;
4. creates a **draft** GitHub release with those four files and generated
   notes.

### Checking the draft

Download the DMG from the draft rather than using the local copy, so you test
what people get:

```sh
gh release download v<version> -p Wezzy.dmg
```

Open it, drag Wezzy to Applications and launch it. It must open without a
Gatekeeper warning. Then publish:

```sh
gh release edit v<version> --draft=false
```

Installed copies only see published releases: the feed URL is
`releases/latest/download/latest.json`, and drafts are never "latest".

### When it fails

| Error | Fix |
|---|---|
| `working tree isn't clean` | `git status`; commit or stash |
| `HEAD isn't pushed` | `git push` |
| `v<v> already exists` | bump `version` in `Cargo.toml` |
| `no 'Developer ID Application' certificate` | install the certificate (below) |
| `notarytool profile '…' doesn't work` | create the profile, or set `NOTARY_PROFILE` |
| `notarization failed` | `xcrun notarytool log <id> --keychain-profile dottt` shows why |

If it fails after tagging (the `gh release create` step), the tag is already
pushed. Either create the release by hand from `target/release/dist/` with the
same `gh release create` line from the script, or delete the tag
(`git push origin :v<v> && git tag -d v<v>`) and run it again.

### A signed build without releasing

`scripts/bundle.sh --dist` signs and notarizes into `target/release/dist/`
without tagging or uploading. Plain `scripts/bundle.sh` makes an ad-hoc signed
`target/release/Wezzy.app` that runs only on this Mac and never auto-updates.
`scripts/rebuild.sh` builds that, replaces `/Applications/Wezzy.app` with it,
and restarts the app. Your shells and agents keep running in the session server.

## Signing setup

One-time, on the machine that releases. Team `827TBA8ZC3`.

| | Check |
|---|---|
| Developer ID Application certificate + key | `security find-identity -v -p codesigning` |
| notarytool keychain profile | `xcrun notarytool history --keychain-profile dottt` |
| `gh`, logged in with push access | `gh auth status` |

The certificate comes from developer.apple.com → Certificates (type
"Developer ID Application"); install it with its private key into the login
keychain.

The notary profile defaults to `dottt`. To create one, with an app-specific
password from appleid.apple.com → Sign-In and Security:

```sh
xcrun notarytool store-credentials dottt --apple-id <apple id> --team-id 827TBA8ZC3
```

Any profile for the same team works; point at another with
`NOTARY_PROFILE=<name>`. `SIGN_IDENTITY` overrides the certificate (default:
the first Developer ID Application identity), and `RELEASE_REPO` the GitHub
repo (default `schester44/wezzy`, which must match `FEED` in `src/update.rs`).

## The update feed

`latest.json`:

```json
{
  "version": "0.2.0",
  "wezterm": "<the pinned WezTerm commit>",
  "url": "https://github.com/schester44/wezzy/releases/download/v0.2.0/Wezzy-0.2.0.zip",
  "sha256": "<of the zip>"
}
```

`src/update.rs` polls it (30 s after launch, then every 6 h, while the
`auto_update` setting is on) and on ⌘K → Check for Updates. To install, it:

1. downloads the zip into `.wezzy-update/` next to the running bundle, so the
   swap is a rename on one volume;
2. checks the SHA-256, then `codesign --verify --deep --strict` against a
   requirement of Apple-anchored, identifier `dev.wezzy.app`, leaf OU = the
   running app's own Team ID, then `spctl --assess`, then that the bundle's
   version matches the feed;
3. renames the old bundle aside, moves the new one in, and deletes the old one.

The running process keeps its mapped files, and the session server keeps
running the binary it was started from, so nothing notices until the next
launch. `restart.rs` relaunches the bundle by path, which is the new one.

Only a Developer ID-signed bundle updates: dev builds (`restart::source_dir()`),
bare binaries and ad-hoc `bundle.sh` builds don't, because there's no Team ID to
pin the update's signature to.

### WezTerm bumps

The mux protocol must match the running session server. If the feed's
`wezterm` differs from this build's pinned commit, the updater doesn't install
it; it says so once per version. Releases that bump WezTerm need a note telling
people to stop the server (`kill $(cat ~/.local/share/wezzy/pid)`) and install
by hand.

## Server binaries for remote hosts

Setting a host up sends the host a Linux build of `wezzy` over ssh
(`scripts/set-up-host.sh`), so the host needs no internet. The builds ship in
the app, `Wezzy.app/Contents/Resources/server/wezzy-linux-{x86_64,aarch64}.tar.gz`:
`bundle.sh` runs `scripts/build-server.sh` (Docker, `rust:1-bullseye`, glibc
2.31, OpenSSL built in), which skips a build that's newer than everything in
`cli/`. A release bundle (`--dist`) fails without Docker; a dev bundle uses
whatever builds `target/server` has.

A host with no matching build falls back to `install-server.sh`'s download from
a release tagged `server-<pin>` (one per WezTerm pin), then to building from
source. That release is optional; to publish it:

```sh
gh release create server-e68a8ec target/server/* -R schester44/wezzy \
  --title "Wezzy server (wezterm e68a8ec)" --notes "Linux binaries for Set up"
```
