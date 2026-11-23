# Contributing to Jafnify

Thanks for helping. Keep changes small and focused, and include a test for a
bug before the fix.

## Requirements

- Rust stable. The Linux app builds for x86_64 Linux; the Android library
  builds for `aarch64-linux-android`.
- JDK 17 and the Android SDK (compileSdk 35, NDK 26.1.10909125) for the app.
- `wireguard-tools` and `polkit` to run the Linux app.

## Language and style

- English only, ASCII only in tracked files. No typographic dashes, curly
  quotes or ellipses.
- Keep user-visible text factual and neutral. The wording test in the Android
  unit tests is the guard.
- Match the style of the module you touch: naming, comment density and error
  types.

## Tests

Run the three Rust suites, in this order:

    cargo test --manifest-path jafnify-core/Cargo.toml
    cargo test --manifest-path linux/Cargo.toml
    cargo test --manifest-path app/wgserv/Cargo.toml

Android:

    app/gradlew -p app :app:testDebugUnitTest
    app/gradlew -p app :app:lintDebug
    app/gradlew -p app assembleDebug

Lint warnings are errors, and the debug build verifies that the APK holds the
library that was just built.

Clippy must report zero warnings:

    cargo clippy --all-targets --manifest-path jafnify-core/Cargo.toml -- -D warnings
    cargo clippy --all-targets --manifest-path linux/Cargo.toml -- -D warnings
    cargo clippy --all-targets --manifest-path app/wgserv/Cargo.toml -- -D warnings

The network-namespace lab needs root and real network namespaces. It is
described in `tools/lab/README.md`; it is the proof that a routing or
discovery change works end to end.

## Version strings

A version bump touches `jafnify-core/Cargo.toml`, `linux/Cargo.toml`,
`app/wgserv/Cargo.toml`, the Android `versionName` and `versionCode`, and the
top `CHANGELOG.md` entry. Then run:

    sh tools/release/check-version.sh

## Secrets

Never commit a private key, a state file, a keystore or a diagnostics export.
The rules and the accepted risks are in SECURITY.md.

## Licence

By contributing you agree that your work is released under the MIT License,
the same as the rest of the project. Keep the upstream wgslirpy notice in
`libwgslirpy-fork/LICENSE-MIT` when you change that directory.
