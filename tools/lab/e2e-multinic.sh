#!/bin/bash
# Jafnify multi-NIC lab (run as root). The laptop has two networks: the
# phone's hotspot and a second link (Ethernet, docker0, a VM NIC) that has its
# own address and carries the default route while the hotspot is away.
#
# What it proves (F5):
#   M1  the joiner has two networks before the absence
#   M2  the joiner is connected through the hotspot
#   M3  the absence lasts longer than the 35 s no-data edge
#   M4  the search follows the hotspot and connects again within 30 s
#       without a restart (the same process)
#   M6  the known connect after the killed joiner is under 10 s: the phone is
#       still in the code track and must answer the joiner's Hello at once
#
# The red run is the same script with the old build: the search pins the
# second network, hears its own multicast echo and never follows the hotspot
# back.
#
# Layout: jl-jo is the laptop, jl-sh is the phone (the hotspot) with the
# internet at 203.0.113.1:8080 in jl-net, and jl-x is the second link's peer.
# The joiner runs in a private mount namespace (tmpfs /etc/wireguard, private
# HOME and XDG), so the host's /etc and config are never touched.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
LAB_HELPER=${LAB_HELPER:-$ROOT/linux/target/release/jafnify-helper}
export LAB_HELPER
LAB_ROUTE=${LAB_ROUTE:-$ROOT/linux/target/release/jafnify-route}
export LAB_ROUTE
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-multinic
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"
ST=$OUT/state
CODE=4321
LAN=192.168.77
SEC=10.55.0
# The hotspot stays away longer than the 35 s no-data edge of the core.
AWAY_SECONDS=${AWAY_SECONDS:-60}
# After the hotspot returns, the joiner must connect without a restart.
RECONNECT_SECONDS=${RECONNECT_SECONDS:-30}
export OUT ST JOIN SHARE CODE LAN
mkdir -p "$OUT"; rm -rf "$OUT"/*; mkdir -p "$ST"
lab_init "$OUT"

NS="jl-jo jl-sh jl-net jl-x"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
trap 'lab_finish; cleanup; rm -rf "$ST"' EXIT
cleanup
ip route show default | head -2 > "$OUT/host-before"
ip -br addr show wg0 > "$OUT/host-wg0-before" 2>&1

for n in $NS; do ip netns add $n; ip -n $n link set lo up; done
# The hotspot link: jl-a on the laptop to the phone.
ip link add jl-a type veth peer name jl-hot
ip link set jl-hot netns jl-sh; ip link set jl-a netns jl-jo
ip -n jl-sh addr add $LAN.1/24 dev jl-hot; ip -n jl-sh link set jl-hot up
ip -n jl-jo addr add $LAN.2/24 dev jl-a; ip -n jl-jo link set jl-a up
ip -n jl-jo route add default via $LAN.1
# The second network: jl-n on the laptop, no route to the phone.
ip link add jl-n type veth peer name jl-nb
ip link set jl-nb netns jl-x; ip link set jl-n netns jl-jo
ip -n jl-x addr add $SEC.1/24 dev jl-nb; ip -n jl-x link set jl-nb up
ip -n jl-jo addr add $SEC.2/24 dev jl-n; ip -n jl-jo link set jl-n up
# Ubuntu (systemd) lets every user ping; a fresh namespace does not.
ip netns exec jl-jo sysctl -qw net.ipv4.ping_group_range="0 2147483647"
# The phone's way to the internet.
ip link add jl-cell type veth peer name jl-cellb
ip link set jl-cell netns jl-sh; ip link set jl-cellb netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-cell; ip -n jl-sh link set jl-cell up
ip -n jl-net addr add 10.200.0.1/24 dev jl-cellb; ip -n jl-net link set jl-cellb up
ip -n jl-net addr add 203.0.113.1/24 dev jl-cellb
ip -n jl-sh route add default via 10.200.0.1
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0
mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
sleep 1

FAILS=0
res() { # NAME cmd...
  if "${@:2}"; then echo "PASS  $1" | tee -a "$OUT/results.txt"
  else echo "FAIL  $1" | tee -a "$OUT/results.txt"; FAILS=$((FAILS+1)); fi
}
internet_ok()  { [ "$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; }
wait_for() { local end=$(( $(date +%s) + $2 )); while [ "$(date +%s)" -lt "$end" ]; do $1 && return 0; sleep 0.5; done; $1; }
elapsed_since() { local ms=$(( ($(date +%s%N) - $1) / 1000000 )); echo "$2: $(( ms / 1000 )).$(printf %03d $(( ms % 1000 ))) s" | tee -a "$OUT/results.txt"; }
has_addr() { ip -n jl-jo -4 -o addr show | grep -q " $1/"; }
# Internet alone is not proof: a kept jafn0 still carries traffic while the app
# is searching. A reconnect needs a NEW "-> Connected" line in the joiner log.
connected_count() { grep -c -- '-> Connected' "$OUT/joiner-known.log" 2>/dev/null || true; }
C0=0
connected_again() { [ "$(connected_count)" -gt "$C0" ] && internet_ok; }
# ABSENCE=carrier (default, like the VM run: the phone side of the link goes
# down, the joiner keeps its address with no carrier) or admin (the joiner's
# own link is set down and its address leaves the up list).
ABSENCE=${ABSENCE:-carrier}
hotspot_away() { if [ "$ABSENCE" = admin ]; then ip -n jl-jo link set jl-a down; else ip -n jl-sh link set jl-hot down; fi; }
hotspot_back() { if [ "$ABSENCE" = admin ]; then ip -n jl-jo link set jl-a up; else ip -n jl-sh link set jl-hot up; fi; }

# sharer_start LABEL: the phone keeps sharing during the absence.
sharer_start() {
  local l=$1
  mkdir -p "$ST/sh-$l"
  ip netns exec jl-sh "$SHARE" --code $CODE --state-dir "$ST/sh-$l" --iface-ip $LAN.1 --iface-ip 10.77.0.1 > "$OUT/sharer-$l.log" 2>&1 &
}
# joiner_start LABEL EXTRA...: the laptop app in its private mount namespace.
joiner_start() {
  local l=$1; shift
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$1" XDG_RUNTIME_DIR="$2"
    export XDG_STATE_HOME="$3"
    mkdir -p "$XDG_CONFIG_HOME/jafnify" "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME"
    chmod 0700 "$XDG_RUNTIME_DIR"
    shift 3
    exec "$@"
  ' _ "$ST/jo" "$ST/run-$l" "$ST/state" "$JOIN" join --accept-terms "$@" > "$OUT/joiner-$l.log" 2>&1 &
  JPID=$!
}

# ---- pair once so the second run is the known-peer search -------------------------
sharer_start multi; sleep 1
joiner_start pair --code $CODE
if wait_for internet_ok 120; then
  res "M0 the first run pairs and connects" true
else
  res "M0 the first run pairs and connects" false
fi
kill $JPID 2>/dev/null || true; wait $JPID 2>/dev/null || true
# Each joiner run has its own tmpfs /etc/wireguard, so the pair run's kept
# jafn0 cannot be adopted by the next run: remove it, as e2e-known.sh does.
ip -n jl-jo link del jafn0 2>/dev/null || true
sleep 1

# ---- the known-peer run has two networks and is connected --------------------------
joiner_start known
KPID=$!
if wait_for connected_again 120; then
  res "M2 before the absence: the joiner is connected through the hotspot" true
else
  res "M2 before the absence: the joiner is connected through the hotspot" false
fi
res "M1 the joiner has two networks before the absence" has_addr $LAN.2
res "M1b the joiner has two networks before the absence" has_addr $SEC.2

# ---- the comeback run is the restart case: the phone is still in the code
# track (the same sharer process as the pair run). The joiner came back on the
# known track, so the phone had to answer that Hello at once: the known
# connect must not wait for the 35 s no-data edge and the retry.
KNOWN_S=$(lab_known_connect_seconds "$OUT/joiner-known.log")
if [ -n "$KNOWN_S" ] && [ "$KNOWN_S" -lt 10 ]; then
  echo "PASS  first-known-connect-after-killed-joiner: ${KNOWN_S}s (under 10s)" | tee -a "$OUT/results.txt"
else
  echo "FAIL  first-known-connect-after-killed-joiner: ${KNOWN_S:-no known connect}s (want under 10s)" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS+1))
fi

# ---- the hotspot goes away; the second network takes the default route -------------
T0=$(date +%s%N)
hotspot_away
ip -n jl-jo route replace default via $SEC.1
res "M1c the second network carries the default route during the absence" \
  sh -c 'ip -n jl-jo route show default | grep -q "via $1"' _ "$SEC.1"
SPENT=$(( ($(date +%s%N) - T0) / 1000000000 ))
if [ "$SPENT" -lt "$AWAY_SECONDS" ]; then sleep $(( AWAY_SECONDS - SPENT )); fi
MEASURED=$(( ($(date +%s%N) - T0) / 1000000000 ))
res "M3 the absence lasted longer than the 35 s no-data edge" \
  sh -c '[ "$1" -ge 36 ]' _ "$MEASURED"

# ---- the hotspot comes back --------------------------------------------------------
C0=$(connected_count)
hotspot_back
ip -n jl-jo route replace default via $LAN.1
T1=$(date +%s%N)
if wait_for connected_again "$RECONNECT_SECONDS"; then
  elapsed_since $T1 "M4 time from the hotspot coming back to the internet reachable"
  res "M4 the search follows the hotspot and connects within ${RECONNECT_SECONDS} s" true
else
  res "M4 the search follows the hotspot and connects within ${RECONNECT_SECONDS} s" false
fi
res "M5 no restart: the same join process is still running" kill -0 "$KPID"

kill $KPID 2>/dev/null || true; wait $KPID 2>/dev/null || true
lab_assert_no_resolvectl "$OUT" "$OUT/joiner-pair.log" "$OUT/joiner-known.log"

echo "---- the joiner log ----" | tee -a "$OUT/results.txt"
tail -40 "$OUT/joiner-known.log" 2>/dev/null | tee -a "$OUT/results.txt"

if [ "$FAILS" -eq 0 ]; then
  echo "RESULT: PASS" | tee -a "$OUT/results.txt"
else
  echo "RESULT: FAIL ($FAILS)" | tee -a "$OUT/results.txt"
fi

# ---- the host must be untouched ----------------------------------------------------
ip -br addr show wg0 > "$OUT/host-wg0-after" 2>&1
diff "$OUT/host-wg0-before" "$OUT/host-wg0-after" && echo "wg0 unchanged"
ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"

lab_finish
echo "=== $FAILS failed ==="
exit $FAILS
