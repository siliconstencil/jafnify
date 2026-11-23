#!/bin/bash
# Shared lab prelude. Every lab script sources this file:
#   . "$(dirname "$0")/lib.sh"
# and calls lab_init "$OUT" after creating OUT. The EXIT trap calls lab_finish.
#
# The joiner sandbox is not optional: a joiner may write /etc/wireguard and
# wg-quick's DNS step can reach the host systemd-resolved over D-Bus. This
# prelude gives every joiner its own mount namespace with a tmpfs /etc/wireguard,
# a no-op resolvconf and a tmpfs over /run/systemd/resolve. It also snapshots
# the host resolvectl status and the host tunnel/polkit files, and fails the run
# if either changed.

: "${FAILS:=0}"

# Every lab runs the freshly built helper and the freshly built route binary
# unless the caller names another one: an installed copy from an older build
# must never decide a lab result. The swap itself happens in the joiner
# sandbox, which needs the binaries to exist.
LAB_ROOT=${LAB_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}
: "${LAB_HELPER:=$LAB_ROOT/linux/target/release/jafnify-helper}"
: "${LAB_ROUTE:=$LAB_ROOT/linux/target/release/jafnify-route}"
export LAB_HELPER LAB_ROUTE

# host_sensitive_digest: hash the host files the lab must never touch.
host_sensitive_digest() {
  find /etc/wireguard /etc/polkit-1/rules.d -maxdepth 1 -type f \
    -exec sha256sum {} + 2>/dev/null | sort
}

lab_init() {
  LAB_OUT="$1"
  mkdir -p "$LAB_OUT"
  # Every lab says which helper it runs: a lab result is only about the
  # binary it names.
  echo "lab: helper $LAB_HELPER"
  echo "lab: route  $LAB_ROUTE"
  if [ ! -x "$LAB_HELPER" ]; then
    echo "lab: helper $LAB_HELPER is missing: build it before the lab" >&2
  fi
  if command -v resolvectl >/dev/null 2>&1; then
    resolvectl status > "$LAB_OUT/host-resolved-before" 2>&1 || true
  else
    : > "$LAB_OUT/host-resolved-before"
  fi
  host_sensitive_digest > "$LAB_OUT/host-sensitive-before"
  # The caller's EXIT trap runs this after cleanup.
  LAB_INITIALIZED=1
}

lab_finish() {
  [ "${LAB_INITIALIZED:-0}" = 1 ] || return 0
  [ "${LAB_FINISHED:-0}" = 1 ] && return 0
  LAB_FINISHED=1
  [ -n "${LAB_OUT:-}" ] || return 0
  local bad=0
  if command -v resolvectl >/dev/null 2>&1; then
    if ! resolvectl status 2>&1 | diff -q - "$LAB_OUT/host-resolved-before" >/dev/null; then
      echo "FAIL  host resolvectl status unchanged" | tee -a "$LAB_OUT/results.txt"
      FAILS=$((FAILS + 1))
      bad=1
    else
      echo "PASS  host resolvectl status unchanged" | tee -a "$LAB_OUT/results.txt"
    fi
  fi
  host_sensitive_digest > "$LAB_OUT/host-sensitive-after"
  if ! diff -q "$LAB_OUT/host-sensitive-before" "$LAB_OUT/host-sensitive-after" >/dev/null; then
    echo "FAIL  host sensitive files unchanged" | tee -a "$LAB_OUT/results.txt"
    FAILS=$((FAILS + 1))
    bad=1
  else
    echo "PASS  host sensitive files unchanged" | tee -a "$LAB_OUT/results.txt"
  fi
  [ "$bad" = 0 ] || return 1
}

# lab_assert_no_resolvectl OUT LOG...
# The joiner sandbox must never reach the host resolver. A log line naming
# resolvectl means a hook ran there despite the sandbox.
lab_assert_no_resolvectl() {
  local out="$1"
  shift
  local bad=0
  local f
  for f in "$@"; do
    if [ -f "$f" ] && grep -q "resolvectl" "$f"; then
      echo "FAIL  $f contains resolvectl" | tee -a "$out/results.txt"
      FAILS=$((FAILS + 1))
      bad=1
    fi
  done
  if [ "$bad" = 0 ]; then
    echo "PASS  no joiner log contains resolvectl" | tee -a "$out/results.txt"
  fi
}

# lab_known_connect_seconds LOG
# The seconds from the joiner's first "search started (known peer)" line to
# the first "-> Connected" line after it. Empty when the log has no such pair.
# This is the time a paired device that came back needs to reach the phone.
lab_known_connect_seconds() {
  python3 -I - "$1" <<'PY'
import re, sys, datetime
stamp = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})Z")
start = None
for line in open(sys.argv[1], errors="replace"):
    m = stamp.match(line)
    if not m:
        continue
    ts = datetime.datetime.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S")
    if start is None:
        if "search started (known peer)" in line:
            start = ts
        continue
    if "-> Connected" in line:
        print(int((ts - start).total_seconds()))
        break
PY
}

# joiner_sandbox NETNS COMMAND...
# The command is normally `bash -c '...'`. Only this function may create the
# joiner mount namespace; no lab block starts a joiner without it.
joiner_sandbox() {
  local ns="$1"
  shift
  ip netns exec "$ns" unshare -m --propagation private bash -c '
    set -e
    mkdir -p /etc/wireguard
    mount -t tmpfs tmpfs /etc/wireguard
    # The helper installs a per-user polkit rule. Keep it inside the lab mount
    # namespace so the host /etc/polkit-1 is never written.
    if [ -d /etc/polkit-1 ]; then
      mount -t tmpfs tmpfs /etc/polkit-1
    fi
    # Use the freshly built helper and route binary when the run requests it,
    # so the lab never exercises an older installed copy. The swap needs the
    # install directory to exist; a clean machine without `install-helper`
    # otherwise falls back silently to the pkexec /bin/sh path.
    if [ -n "${LAB_HELPER:-}" ] && [ -x "$LAB_HELPER" ]; then
      if [ -d /usr/libexec/jafnify ]; then
        mount -t tmpfs tmpfs /usr/libexec/jafnify
        cp "$LAB_HELPER" /usr/libexec/jafnify/jafnify-helper
        chmod 0755 /usr/libexec/jafnify/jafnify-helper
        if [ -n "${LAB_ROUTE:-}" ] && [ -x "$LAB_ROUTE" ]; then
          cp "$LAB_ROUTE" /usr/libexec/jafnify/jafnify-route
          chmod 0755 /usr/libexec/jafnify/jafnify-route
        fi
      else
        echo "lab: LAB_HELPER not swapped in: /usr/libexec/jafnify is missing;" \
             "run jafnify install-helper once on the host" >&2
      fi
    fi
    if [ -d /run/systemd/resolve ]; then
      mount -t tmpfs tmpfs /run/systemd/resolve
    fi
    # The product must see a stopped resolver here: the directory alone is
    # not proof, the varlink socket is.
    if [ -e /run/systemd/resolve/io.systemd.Resolve ]; then
      echo "FAIL  sandbox still sees the resolved varlink socket" >&2
      exit 1
    fi
    rc=$(command -v resolvconf || true)
    if [ -n "$rc" ]; then
      printf "#!/bin/sh\nexit 0\n" > /etc/wireguard/resolvconf-noop
      chmod +x /etc/wireguard/resolvconf-noop
      mount --bind /etc/wireguard/resolvconf-noop "$rc"
    fi
    # Defence in depth: even a future product regression cannot reach the host
    # resolver through the client name.
    mkdir -p /etc/wireguard/bin
    printf "#!/bin/sh\nexit 0\n" > /etc/wireguard/bin/resolvectl
    chmod +x /etc/wireguard/bin/resolvectl
    export PATH="/etc/wireguard/bin:$PATH"
    # pkexec resets PATH, so the root route binary would run the real
    # resolvectl and reach the host resolver over D-Bus. Cover the real path.
    for real in /usr/bin/resolvectl /bin/resolvectl; do
      if [ -e "$real" ] && [ ! -L "$real" ]; then
        mount --bind /etc/wireguard/bin/resolvectl "$real"
      fi
    done
    exec "$@"
  ' sh "$@"
}
