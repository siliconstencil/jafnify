# Release build scripts

Run these from any directory; each script finds the repository root itself.
Nothing here runs in the normal test loop: the tree must be clean and HEAD must
be the `v<VERSION>` tag, which you create before a release build.

## Android (signed APK)

Environment at build time (never written to disk, never printed):

- `JAFNIFY_KEYSTORE` -- absolute path to the keystore file
- `JAFNIFY_KEYSTORE_PASSWORD`
- `JAFNIFY_KEY_ALIAS`
- `JAFNIFY_KEY_PASSWORD`

    bash tools/release/android.sh

The init script `android-signing.init.gradle` reads the four variables and
fails the Gradle build when any is missing. Output:
`tmp/release/jafnify-<version>-arm64.apk` (arm64 only, matching the cargo
targets in `app/app/build.gradle`).

Before publishing, verify the signature with the SDK's `apksigner`:

    apksigner verify --print-certs tmp/release/jafnify-<version>-arm64.apk

## Linux (tarball and .deb)

    bash tools/release/linux.sh

Builds the three release binaries, stages the tarball under
`tmp/release/jafnify-<version>-linux-x86_64.tar.gz`, and, when `dpkg-deb` is
installed, a `tmp/release/jafnify_<version>_amd64.deb`. The `.deb` installs
`jafnify` and `jafnify-gui` under `/usr/bin`, the helper under
`/usr/libexec/jafnify`, and its own desktop entry that points at
`/usr/bin/jafnify-gui`.

## Checksums

    bash tools/release/sums.sh

Writes `tmp/release/SHA256SUMS` over the `.apk`, `.tar.gz` and `.deb` files.
Publish the file next to the artifacts. Users verify with:

    sha256sum -c SHA256SUMS

## Guard rails

- No secret is stored in the repository or in a script. Passwords exist only in
  the build environment for the duration of the build.
- A tracked edit makes `require_clean_tag` fail. Ignored paths (`tmp/`,
  `target/`, build output) are allowed.
- HEAD must be the exact tag `v<VERSION>`; `<VERSION>` is read from
  `linux/Cargo.toml`. Bump that first, commit, then tag.
- Before any build, `tools/release/check-version.sh` checks that
  `linux/Cargo.toml`, the Android `versionName`, the top `CHANGELOG.md` entry
  and the README artifact names all carry the same version.
