# Releasing

The macOS alpha is built by `.github/workflows/release.yml`. The workflow is
deliberately fail-closed: it will not publish an unsigned build, an unstapled
DMG, an updater archive whose signature does not match `latest.json`, or a feed
that cannot be reached without GitHub credentials.

## One-time release setup

The updater has no authenticated GitHub client. Its feed and every asset named
by that feed therefore have to live in a **public** repository. The workflow
checks repository visibility before it handles any signing material.

At the time this automation was added, `MedSlimane/Aviary` was private. Before
the first automated release, either make it public or move both the versioned
release assets and the `updater-alpha/latest.json` channel to a dedicated public
release repository, then update the endpoint in `aviary/src-tauri/tauri.conf.json`
and the repository checks in the workflow and verifier together.

Configure a protected GitHub `release` environment with these secrets:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting that `.p12` |
| `APPLE_ID` | Apple account used by `notarytool` |
| `APPLE_PASSWORD` | App-specific password for that Apple account |
| `APPLE_TEAM_ID` | Apple Developer team identifier |
| `TAURI_SIGNING_PRIVATE_KEY` | Entire private updater-signing key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Key password, only if one was set |

The matching updater public key is embedded in `tauri.conf.json`. Existing
installations trust that exact key, so do not rotate it as part of an ordinary
release. The private key must never enter the repository; keep an offline backup
in addition to the protected GitHub secret. The current release workstation's
copy is `~/.aviary/keys/aviary-updater.key`, with mode `0600`.

## Cut a release

1. Update the version in all three sources:

   - `aviary/src-tauri/tauri.conf.json`
   - `aviary/package.json`
   - `aviary/src-tauri/Cargo.toml`

2. Verify locally:

   ```bash
   cd aviary
   bun install --frozen-lockfile
   bunx tsc --noEmit
   cargo test --manifest-path src-tauri/Cargo.toml --lib
   cargo build --manifest-path src-tauri/Cargo.toml --bins
   ```

   Then run the app and drive every affected flow. Compilation is only the
   floor for a release.

3. Commit the version, create a tag whose numeric prefix exactly matches it,
   and push the tag. For example, version `0.1.1` may use
   `v0.1.1-alpha.1`.

   A tag push starts the release workflow. `workflow_dispatch` can rebuild an
   existing tag, but cannot release an untagged revision.

4. Watch the `Release macOS alpha` job. It:

   - validates the tag and all version sources;
   - runs the frontend and Rust tests;
   - builds both architectures of `aviary-media`, `aviary-library`, and
     `aviary-launch`, then combines each helper into a universal binary;
   - imports an ephemeral Developer ID keychain;
   - signs and notarizes the universal app and signs the updater archive;
   - separately notarizes and staples the final DMG, which is created after the
     app notarization step;
   - downloads the draft assets back from GitHub and runs
     `aviary/scripts/verify-release.sh` against those downloaded bytes;
   - publishes the versioned prerelease only after every verification passes;
   - promotes that verified manifest to the fixed `updater-alpha` channel and
     compares the public download byte-for-byte.

The action implementations are pinned to full commit SHAs. Update those pins as
an explicit dependency review, never incidentally while cutting a release.

## What verification proves

`verify-release.sh` rejects the release unless:

- the app and all three bundled helpers each contain arm64 and x86_64 slices;
- the app, every helper, and DMG have Developer ID signatures and secure timestamps;
- hardened runtime is enabled;
- Apple stapler validates the app, archive copy and DMG;
- Gatekeeper accepts a quarantined copy as a downloaded user would receive it;
- the updater archive passes minisign verification with the embedded public key;
- `latest.json` names the application version and carries the exact generated
  archive signature for both macOS architectures.

This is stricter than inspecting the local bundle: the final verification uses
the archive, signature and post-stapling DMG downloaded from the draft release.

## First updater release

`v0.1.0-alpha.1` does not contain an updater, so it cannot prove the updater
path. The first Developer ID build is a manual bootstrap install. Keep it
available, install it on a clean macOS account, then publish the next patch and
verify the complete in-app flow:

1. the installed build discovers the newer version on launch and on a manual
   check;
2. the prompt shows the version and notes;
3. the signed archive downloads and installs;
4. Aviary relaunches on the new version without opening GitHub.

P1's updater and notarisation items are only complete after that real
old-version-to-new-version test and a downloaded DMG passes Gatekeeper. Having
the workflow code in the repository is necessary, but is not that proof.

## Release notes

State every known gap plainly. Include the tested macOS versions, the sha256 of
the DMG, and any migration or bootstrap instructions. Once notarised releases
begin, do not carry forward the old right-click/`xattr` workaround: needing it
means verification failed and the release must not be published.
