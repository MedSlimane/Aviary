# Releasing

Cutting a macOS build. Two steps here are non-obvious and both produced a broken
artefact the first time — they are called out below rather than buried.

---

## Prerequisites

```bash
brew install create-dmg
rustup target add x86_64-apple-darwin aarch64-apple-darwin
gh auth status
```

---

## 1. Verify before you build

```bash
cd aviary
bunx tsc --noEmit
cd src-tauri && cargo test --lib      # expect 28 passing
```

Then run the app and drive the surfaces you changed. A green suite is the floor.

---

## 2. Bump the version

`aviary/src-tauri/tauri.conf.json` → `version`. The DMG script reads it, so the
filename and volume name follow automatically.

---

## 3. Build universal

```bash
cd aviary
bun run tauri build --target universal-apple-darwin
```

> **This fails the first time on a clean target directory.** Tauri lipos only
> the *main* binary. Aviary ships two — the app and `aviary-media` — and the
> bundler aborts looking for a universal copy of the second:
>
> ```
> Failed to copy binary from ".../universal-apple-darwin/release/aviary-media"
> ```
>
> Merge it yourself, then re-run the same command:
>
> ```bash
> cd src-tauri
> lipo -create -output target/universal-apple-darwin/release/aviary-media \
>   target/aarch64-apple-darwin/release/aviary-media \
>   target/x86_64-apple-darwin/release/aviary-media
> ```

Confirm both binaries are fat:

```bash
A=src-tauri/target/universal-apple-darwin/release/bundle/macos/aviary.app
lipo -archs "$A/Contents/MacOS/aviary"        # x86_64 arm64
lipo -archs "$A/Contents/MacOS/aviary-media"  # x86_64 arm64
```

---

## 4. Package

```bash
./scripts/make-dmg.sh
```

Produces `aviary/release/Aviary_<version>_universal.dmg` and prints its sha256.

> **`lipo` strips the linker's ad-hoc signature.** A universal bundle arrives
> *completely unsigned*, and macOS reports an unsigned app as **damaged** — not
> merely untrusted. No amount of right-click → Open recovers it. The script
> re-signs ad-hoc and verifies with `codesign --verify --deep --strict` before
> packaging. Do not skip it.

With a Developer ID certificate:

```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" ./scripts/make-dmg.sh
```

---

## 5. Verify the artefact as a downloader receives it

Not as you built it — with the quarantine flag a browser attaches.

```bash
cd /tmp && rm -rf vtest && mkdir vtest
M=$(hdiutil attach ~/personalAi/aviary/release/Aviary_*_universal.dmg \
      -nobrowse -readonly | grep -o '/Volumes/.*$')
cp -R "$M/aviary.app" vtest/
hdiutil detach "$M"

codesign --verify --deep --strict vtest/aviary.app   # must pass
xattr -w com.apple.quarantine "0081;0;Safari;" vtest/aviary.app
open vtest/aviary.app                                 # must launch
```

If `codesign --verify` fails, the DMG is broken — do not upload it.

---

## 6. Publish

```bash
cd ~/personalAi
git push origin main

gh release create v<version>-alpha.<n> \
  aviary/release/Aviary_<version>_universal.dmg \
  --title "Aviary <version>-alpha.<n>" \
  --notes-file <notes>.md \
  --prerelease
```

Then confirm the published bytes match what you tested:

```bash
gh release download v<version>-alpha.<n> -p "*.dmg" -O /tmp/published.dmg
shasum -a 256 /tmp/published.dmg
```

---

## Release notes

State plainly what does not work. The alpha notes list the missing updater,
non-persistent chat sessions, absent bundles and unmeasured MCP tokens — an
alpha tester who discovers a gap you hid stops reporting the ones you did not.

Always include:

- **Install steps for an unnotarised app** — drag to Applications, right-click →
  Open, and the `xattr -dr com.apple.quarantine` fallback.
- **The sha256.**
- **Known gaps**, in plain language.

---

## Current signing status

Ad-hoc signed, **not notarised** — there is no Apple Developer certificate on
the build machine. The app verifies and launches, but Gatekeeper cannot confirm
who built it, so first launch needs the right-click path.

Notarisation is [P1 on the roadmap](ROADMAP.md). The script already accepts
`SIGN_IDENTITY`; what remains is `notarytool submit --wait` and `stapler staple`
in CI.
