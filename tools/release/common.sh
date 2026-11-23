#!/usr/bin/env bash
# Shared release helpers. Nothing here reads keystore material or passwords.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/tmp/release"
VERSION="${JAFNIFY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/linux/Cargo.toml" | head -n 1)}"

die() {
    printf 'release: %s\n' "$*" >&2
    exit 1
}

need_env() {
    local name
    for name in "$@"; do
        [ -n "${!name:-}" ] || die "missing environment variable $name"
    done
}

# The GO requires a build from a clean, tagged tree. A tracked edit blocks the
# build; ignored files (tmp/, target/) do not count.
require_clean_tag() {
    local dirty tag
    dirty="$(git -C "$ROOT" status --porcelain --untracked-files=no)"
    [ -z "$dirty" ] || die "tracked files are not clean; commit them before the release build"
    tag="$(git -C "$ROOT" describe --tags --exact-match 2>/dev/null || true)"
    [ "$tag" = "v$VERSION" ] || die "HEAD is not the v$VERSION tag (found '${tag:-no tag}')"
}

ensure_out() {
    mkdir -p "$OUT"
}

sha256_of() {
    sha256sum "$1" | cut -d' ' -f1
}
