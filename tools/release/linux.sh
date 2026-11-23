#!/usr/bin/env bash
# Build the Linux release tarball, and a .deb when dpkg-deb is available.
#
# The tree must be clean and HEAD must be the v<VERSION> tag. Output:
#   tmp/release/jafnify-<version>-linux-x86_64.tar.gz
#   tmp/release/jafnify_<version>_amd64.deb        (when dpkg-deb exists)
set -euo pipefail
source "$(dirname "$0")/common.sh"

require_clean_tag
ensure_out
sh "$ROOT/tools/release/check-version.sh"
[ "$(uname -m)" = "x86_64" ] || die "release packaging is defined for x86_64 only"

# Panic messages embed source paths; map the build machine's directories to
# neutral names so no local path or user name ships in a binary.
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$CARGO_DIR=/cargo --remap-path-prefix=$ROOT=/jafnify"
cargo build --release --manifest-path "$ROOT/linux/Cargo.toml"
for bin in jafnify jafnify-gui jafnify-helper jafnify-route; do
    if strings "$ROOT/linux/target/release/$bin" | grep -q -e "$HOME" -e "$ROOT"; then
        die "release binary $bin still holds a local path"
    fi
done
for bin in jafnify jafnify-gui jafnify-helper jafnify-route; do
    [ -x "$ROOT/linux/target/release/$bin" ] || die "missing release binary $bin"
done

name="jafnify-$VERSION-linux-x86_64"
stage="$OUT/$name"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/polkit-1/actions" \
    "$stage/share/applications" \
    "$stage/share/icons/hicolor/scalable/apps"
# Third-party notices, generated offline from both packaged target graphs.
notices="$OUT/THIRD_PARTY_NOTICES.txt"
sh "$ROOT/tools/release/notices.sh" "$notices"
install -m 0755 "$ROOT/linux/target/release/jafnify" \
    "$ROOT/linux/target/release/jafnify-gui" \
    "$ROOT/linux/target/release/jafnify-helper" \
    "$ROOT/linux/target/release/jafnify-route" "$stage/bin/"
install -m 0644 "$ROOT/assets/polkit/org.jafnify.policy" \
    "$stage/share/polkit-1/actions/"
install -m 0644 "$ROOT/assets/linux/jafnify.desktop" \
    "$stage/share/applications/"
install -m 0644 "$ROOT/assets/icon/jafnify.svg" \
    "$ROOT/assets/icon/jafnify-symbolic.svg" \
    "$stage/share/icons/hicolor/scalable/apps/"
install -m 0644 "$ROOT/README.md" "$ROOT/CHANGELOG.md" "$ROOT/LICENSE" \
    "$notices" "$stage/"

tar -C "$OUT" --sort=name --owner=0 --group=0 --numeric-owner \
    -czf "$OUT/$name.tar.gz" "$name"
printf 'tarball: %s\n' "$OUT/$name.tar.gz"
printf 'sha256: %s\n' "$(sha256_of "$OUT/$name.tar.gz")"

if command -v dpkg-deb >/dev/null 2>&1; then
    deb="$OUT/deb-root"
    rm -rf "$deb"
    mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/libexec/jafnify" \
        "$deb/usr/share/polkit-1/actions" "$deb/usr/share/applications" \
        "$deb/usr/share/icons/hicolor/scalable/apps" \
        "$deb/usr/share/doc/jafnify"
    install -m 0755 "$ROOT/linux/target/release/jafnify" \
        "$ROOT/linux/target/release/jafnify-gui" "$deb/usr/bin/"
    install -m 0755 "$ROOT/linux/target/release/jafnify-helper" \
        "$ROOT/linux/target/release/jafnify-route" \
        "$deb/usr/libexec/jafnify/"
    install -m 0644 "$ROOT/assets/polkit/org.jafnify.policy" \
        "$deb/usr/share/polkit-1/actions/"
    install -m 0644 "$ROOT/assets/icon/jafnify.svg" \
        "$ROOT/assets/icon/jafnify-symbolic.svg" \
        "$deb/usr/share/icons/hicolor/scalable/apps/"
    # The package installs to /usr/bin, so the desktop entry points there.
    sed 's#/usr/local/bin/jafnify-gui#/usr/bin/jafnify-gui#' \
        "$ROOT/assets/linux/jafnify.desktop" \
        > "$deb/usr/share/applications/jafnify.desktop"
    install -m 0644 "$ROOT/README.md" "$ROOT/CHANGELOG.md" \
        "$deb/usr/share/doc/jafnify/"
    install -m 0644 "$ROOT/LICENSE" "$deb/usr/share/doc/jafnify/copyright"
    install -m 0644 "$notices" "$deb/usr/share/doc/jafnify/THIRD_PARTY_NOTICES.txt"

    # Maintainer scripts: take the tunnel down on remove, drop the key and rule on purge.
    install -m 0755 "$ROOT/assets/linux/deb/prerm" "$ROOT/assets/linux/deb/postrm" "$deb/DEBIAN/"
    cat > "$deb/DEBIAN/control" <<EOF
Package: jafnify
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Depends: wireguard-tools, pkexec
Recommends: nftables | iptables, systemd-resolved
Maintainer: Jafnify maintainers <maintainers@jafnify.invalid>
Description: Share a phone's internet with a laptop over WireGuard
 Jafnify pairs an Android phone that shares its connection with a Linux
 computer over an encrypted WireGuard tunnel. The phone provides the hotspot;
 the computer joins it. No account and no cloud service.
EOF

    debfile="$OUT/jafnify_${VERSION}_amd64.deb"
    dpkg-deb --build --root-owner-group "$deb" "$debfile"
    rm -rf "$deb"
    printf 'deb: %s\n' "$debfile"
    printf 'sha256: %s\n' "$(sha256_of "$debfile")"
else
    printf 'dpkg-deb not found: .deb skipped (tarball only)\n'
fi
