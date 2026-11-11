# Releases from CI

Releases are cut by hand today ([Releasing](../development/releasing.md)).
This is how to move that onto GitHub Actions: push a `v*` tag, and a macOS
runner builds, signs, notarizes and drafts the release. You still test the
draft and publish it yourself.

Nothing here is built yet. The workflow below is a starting point and hasn't
been run.

## How it differs from `release.sh`

| | By hand | CI |
|---|---|---|
| Trigger | `scripts/release.sh` | `git push origin v<version>` |
| Tag | created by the script | created by you; the workflow builds it |
| Certificate | your login keychain | a `.p12` secret, imported into a temporary keychain |
| Notarization | the `dottt` keychain profile (Apple ID) | an App Store Connect API key, stored as a profile named `dottt` in the temporary keychain |
| Upload | `gh`, logged in as you | `gh` with the workflow's `GITHUB_TOKEN` |

The workflow runs `scripts/bundle.sh --dist` unchanged. It doesn't run
`release.sh`, which would refuse because the tag already exists; it does the
same upload itself.

## 1. Export the signing certificate

On the Mac that has it:

1. Keychain Access → login → My Certificates.
2. Right-click **Developer ID Application: … (827TBA8ZC3)** → Export. Save as
   `.p12` with a strong password. The certificate must show a disclosure
   arrow with its private key under it; without the key, CI can't sign.
3. Store it and the password as repository secrets:

```sh
base64 -i DeveloperID.p12 | gh secret set DEVELOPER_ID_P12 -R schester44/wezzy
gh secret set DEVELOPER_ID_P12_PASSWORD -R schester44/wezzy   # prompts
rm DeveloperID.p12
```

## 2. Create a notarization API key

Notarizing with an Apple ID needs an app-specific password and can trip
two-factor prompts; an API key doesn't.

1. appstoreconnect.apple.com → Users and Access → Integrations → App Store
   Connect API → Team Keys → **+**. Name it "Wezzy CI", access **Developer**.
2. Download `AuthKey_<KEYID>.p8`. Apple only lets you download it once.
3. Note the **Key ID** (on the key's row) and the **Issuer ID** (above the
   table).

```sh
base64 -i AuthKey_XXXXXXXXXX.p8 | gh secret set NOTARY_KEY_P8 -R schester44/wezzy
gh secret set NOTARY_KEY_ID -R schester44/wezzy      # prompts
gh secret set NOTARY_ISSUER_ID -R schester44/wezzy   # prompts
```

Keep the `.p8` somewhere safe (a password manager), then delete the download.

## 3. Protect the secrets

The repo is public. Secrets aren't passed to workflows run from forks, and
this workflow only runs on tag pushes, which need write access. To also require
your approval before anything gets signed:

1. Settings → Environments → New environment `release`.
2. Add yourself under **Required reviewers**.
3. Move the five secrets into that environment (`gh secret set … --env
   release`) instead of the repository.

The workflow below already names `environment: release`; with no reviewers set,
it runs straight away.

## 4. Add the workflow

`.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write   # create the release and upload assets

jobs:
  release:
    runs-on: macos-14   # Apple Silicon, like the hand-built releases
    environment: release
    env:
      RELEASE_REPO: ${{ github.repository }}
      KEYCHAIN: ${{ runner.temp }}/release.keychain-db
    steps:
      - uses: actions/checkout@v4

      - name: Tag matches Cargo.toml
        run: |
          v=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
          [[ "v$v" == "$GITHUB_REF_NAME" ]] || { echo "tag $GITHUB_REF_NAME != Cargo.toml $v"; exit 1; }

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Cache wezterm-mux-server
        uses: actions/cache@v4
        with:
          path: target/mux-server
          key: mux-server-${{ hashFiles('Cargo.toml') }}

      - name: Signing keychain
        env:
          P12: ${{ secrets.DEVELOPER_ID_P12 }}
          P12_PASSWORD: ${{ secrets.DEVELOPER_ID_P12_PASSWORD }}
          NOTARY_KEY_P8: ${{ secrets.NOTARY_KEY_P8 }}
          NOTARY_KEY_ID: ${{ secrets.NOTARY_KEY_ID }}
          NOTARY_ISSUER_ID: ${{ secrets.NOTARY_ISSUER_ID }}
        run: |
          pw=$(openssl rand -hex 24)
          security create-keychain -p "$pw" "$KEYCHAIN"
          security set-keychain-settings -lut 21600 "$KEYCHAIN"
          security unlock-keychain -p "$pw" "$KEYCHAIN"
          echo "$P12" | base64 -d > "$RUNNER_TEMP/cert.p12"
          security import "$RUNNER_TEMP/cert.p12" -k "$KEYCHAIN" -P "$P12_PASSWORD" \
            -T /usr/bin/codesign -T /usr/bin/security
          security set-key-partition-list -S apple-tool:,apple: -k "$pw" "$KEYCHAIN"
          # put it first in the search list so codesign and notarytool find it
          security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')
          echo "$NOTARY_KEY_P8" | base64 -d > "$RUNNER_TEMP/key.p8"
          xcrun notarytool store-credentials dottt --keychain "$KEYCHAIN" \
            --key "$RUNNER_TEMP/key.p8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID"
          rm "$RUNNER_TEMP/cert.p12" "$RUNNER_TEMP/key.p8"

      - name: Build, sign, notarize
        run: scripts/bundle.sh --dist

      - name: Draft release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          v=${GITHUB_REF_NAME#v}
          d=target/release/dist
          cp "$d/Wezzy-$v.dmg" "$d/Wezzy.dmg"
          gh release create "$GITHUB_REF_NAME" -R "$RELEASE_REPO" --draft --verify-tag \
            --title "Wezzy $v" --generate-notes \
            "$d/Wezzy-$v.dmg" "$d/Wezzy.dmg" "$d/Wezzy-$v.zip" "$d/latest.json"

      - name: Remove keychain
        if: always()
        run: security delete-keychain "$KEYCHAIN" || true
```

## 5. Cutting a release

```sh
# bump version in Cargo.toml
git commit -am "Release 0.2.0" && git push
git tag -a v0.2.0 -m "Wezzy 0.2.0" && git push origin v0.2.0
```

Watch it with `gh run watch`. When it finishes, test and publish the draft as
in [Releasing](../development/releasing.md#checking-the-draft).

If a run fails, fix the cause, delete the draft if one was made, and re-run
the workflow from the Actions tab (it builds the same tag). If the fix needs a
new commit, move the tag: `git tag -fa v0.2.0 -m "Wezzy 0.2.0" && git push -f
origin v0.2.0`.

## Things to check on the first run

- **Notarization profile name.** `bundle.sh` defaults `NOTARY_PROFILE` to
  `dottt`, so the workflow stores the API key under that name. Rename both
  together.
- **Mux server build time.** The first run builds `wezterm-mux-server` from
  source; later runs restore it from the cache. The cache key changes when
  `Cargo.toml` does. Only the WezTerm rev in it matters, so a narrower key is
  possible.
- **Architecture.** `macos-14` is Apple Silicon, so the app is arm64 only, as
  hand-built releases from an Apple Silicon Mac are now. Intel support would
  need a universal build in `bundle.sh` (both targets, joined with `lipo`).
- **Certificate expiry.** Developer ID certificates last five years. When it's
  renewed, export and replace `DEVELOPER_ID_P12`.
- **Updater identity.** The updater only accepts releases signed by the same
  team (`827TBA8ZC3`) with the bundle id `dev.wezzy.app`. CI uses the same
  certificate, so CI releases update hand-built installs and the other way
  round.
