#!/usr/bin/env bash
# Build the signed release APK.
#
# Required environment (never written to disk, never logged):
#   JAFNIFY_KEYSTORE           path to the keystore file
#   JAFNIFY_KEYSTORE_PASSWORD  keystore password
#   JAFNIFY_KEY_ALIAS          key alias
#   JAFNIFY_KEY_PASSWORD       key password
#
# The tree must be clean and HEAD must be the v<VERSION> tag. Output:
#   tmp/release/jafnify-<version>-arm64.apk
set -euo pipefail
source "$(dirname "$0")/common.sh"

need_env JAFNIFY_KEYSTORE JAFNIFY_KEYSTORE_PASSWORD JAFNIFY_KEY_ALIAS JAFNIFY_KEY_PASSWORD
[ -f "$JAFNIFY_KEYSTORE" ] || die "the keystore named by JAFNIFY_KEYSTORE does not exist"
[ -r "$JAFNIFY_KEYSTORE" ] || die "the keystore named by JAFNIFY_KEYSTORE is not readable"
require_clean_tag
ensure_out
sh "$ROOT/tools/release/check-version.sh"

# The About screen opens the notices asset. Generate it offline from the same
# two crate graphs the release build ships, and put it where Gradle looks.
notices="$OUT/THIRD_PARTY_NOTICES.txt"
sh "$ROOT/tools/release/notices.sh" "$notices"
mkdir -p "$ROOT/app/app/src/main/assets"
install -m 0644 "$notices" "$ROOT/app/app/src/main/assets/THIRD_PARTY_NOTICES.txt"

"$ROOT/app/gradlew" -p "$ROOT/app" clean lintDebug assembleRelease \
    --init-script "$ROOT/tools/release/android-signing.init.gradle"

apk="$ROOT/app/app/build/outputs/apk/release/app-release.apk"
[ -f "$apk" ] || die "assembleRelease did not produce app-release.apk"

dest="$OUT/jafnify-$VERSION-arm64.apk"
install -m 0644 "$apk" "$dest"
printf 'apk: %s\n' "$dest"
printf 'sha256: %s\n' "$(sha256_of "$dest")"
