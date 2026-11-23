#!/bin/sh
# Fail before a build when the release version strings disagree.
# POSIX sh; reads files only, uses no new tools.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
LINUX_TOML="$ROOT/linux/Cargo.toml"
ANDROID_GRADLE="$ROOT/app/app/build.gradle"
CHANGELOG="$ROOT/CHANGELOG.md"
README="$ROOT/README.md"

die() {
    printf 'check-version: %s\n' "$*" >&2
    exit 1
}

version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$LINUX_TOML" | head -n 1)
[ -n "$version" ] || die "no version in $LINUX_TOML"
case "$version" in
    *[!0-9.]*) die "version '$version' in $LINUX_TOML is not numeric" ;;
esac

android=$(sed -n 's/^[[:space:]]*versionName "\([^"]*\)".*/\1/p' "$ANDROID_GRADLE" | head -n 1)
[ "$android" = "$version" ] ||
    die "Android versionName '$android' != '$version'"

changelog=$(sed -n 's/^## \[\([^]]*\)\].*/\1/p' "$CHANGELOG" | head -n 1)
[ "$changelog" = "$version" ] ||
    die "CHANGELOG top entry '$changelog' != '$version'"

for name in \
    "jafnify-$version-arm64.apk" \
    "jafnify-$version-linux-x86_64.tar.gz" \
    "jafnify_${version}_amd64.deb"
do
    grep -Fq "$name" "$README" ||
        die "README does not name $name"
done

printf 'check-version: %s ok\n' "$version"
