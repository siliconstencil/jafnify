#!/usr/bin/env bash
# Regenerate tmp/release/SHA256SUMS over the release artifacts.
set -euo pipefail
source "$(dirname "$0")/common.sh"

[ -d "$OUT" ] || die "no release directory: run android.sh and/or linux.sh first"
cd "$OUT"
files="$(find . -maxdepth 1 -type f \
    \( -name '*.apk' -o -name '*.tar.gz' -o -name '*.deb' \) \
    -printf '%P\n' | sort)"
[ -n "$files" ] || die "no release artifacts in $OUT"
printf '%s\n' "$files" | xargs sha256sum > SHA256SUMS
cat SHA256SUMS
