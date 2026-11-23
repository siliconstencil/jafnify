#!/bin/bash
# Jafnify away lab (run as root). The laptop's Wi-Fi to the phone's hotspot
# goes away for more than five minutes and comes back.
#
# What it proves:
#   A1  the laptop is connected before the absence
#   A2  the absence is recorded as an away event once it is longer than the
#       two-minute edge of the core, not as a drop
#   A3  the absence is one record: one drop line and one away line
#   A4  the laptop is connected again with no user action
#   A5  the absence is closed with a back line
#   A6  the daemon counts no drop for the absence
#   A7  the connection log says how long the phone was away
#
# Same namespace layout as e2e-resilience.sh: jl-jo is the laptop, jl-sh is
# the phone (the hotspot) and jl-net is the internet at 203.0.113.1:8080.
# The laptop runs the tray daemon, so the daemon owns the flags and the drop
# history: the drop history switch is on in the private flags.toml of the lab.
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
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-away-return
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
ST=$OUT/state
CODE=4321
LAN=192.168.77
# How long the Wi-Fi of the laptop to the hotspot stays away. It must be
# longer than the two-minute edge of the core; the reference behaviour waits
# minutes, so the default is five and a half.
AWAY_SECONDS=${AWAY_SECONDS:-330}
export OUT ST JOIN SHARE CODE LAN
mkdir -p "$OUT"; rm -rf "$OUT"/*; mkdir -p "$ST"
lab_init "$OUT"

NS="jl-jo jl-sh jl-net"
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
ip link add jl-hot type veth peer name jl-lan
ip link set jl-hot netns jl-sh; ip link set jl-lan netns jl-jo
ip -n jl-sh addr add $LAN.1/24 dev jl-hot; ip -n jl-sh link set jl-hot up
ip -n jl-jo addr add $LAN.2/24 dev jl-lan; ip -n jl-jo link set jl-lan up
ip -n jl-jo route add default via $LAN.1
# Ubuntu (systemd) lets every user ping; a fresh namespace does not. The app's
# liveness probe is an ICMP echo through jafn0, so mirror the host default.
ip netns exec jl-jo sysctl -qw net.ipv4.ping_group_range="0 2147483647"
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
# wait_for CMD SECONDS: poll every 0.5 s; returns 0 when CMD succeeds.
wait_for() { local end=$(( $(date +%s) + $2 )); while [ "$(date +%s)" -lt "$end" ]; do $1 && return 0; sleep 0.5; done; $1; }
elapsed_since() { local ms=$(( ($(date +%s%N) - $1) / 1000000 )); echo "$2: $(( ms / 1000 )).$(printf %03d $(( ms % 1000 ))) s" | tee -a "$OUT/results.txt"; }

# The drop file of the laptop, in the private config dir of the lab.
DROPS="$ST/jo-away/jafnify/drops.tsv"
# The away line of the absence.
away_recorded() { grep -q " away at=" "$DROPS" 2>/dev/null; }
# One record for the absence: one drop line and one away line.
one_away_line()  { [ "$(grep -c " away at=" "$DROPS" 2>/dev/null)" = 1 ]; }
one_drop_line()  { [ "$(grep -c " drop at=" "$DROPS" 2>/dev/null)" = 1 ]; }
# The back line closes the absence.
back_recorded() { grep -q " back at=" "$DROPS" 2>/dev/null; }
# The count the user sees: the CLI asks the daemon over its socket.
daemon_drops() {
  joiner_sandbox jl-jo bash -c 'XDG_RUNTIME_DIR="$1" "$2" drops' _ "$ST/run-away" "$JOIN" 2>&1
}
daemon_counts_no_drop() { daemon_drops | grep -q "Drops in the last 24 h: 0"; }
# The connection log of the laptop says how long the phone was away.
away_in_the_log() { grep -q "Phone away" "$ST/state-away/jafnify/connection.log" 2>/dev/null; }
tray_up_away() { [ -f "$OUT/tray-up-away" ]; }

# sharer_start LABEL: the phone's sharer. It keeps running during the absence:
# the phone is there, the laptop's Wi-Fi to it is not.
sharer_start() {
  local l=$1
  mkdir -p "$ST/sh-$l"
  ip netns exec jl-sh "$SHARE" --code $CODE --state-dir "$ST/sh-$l" --iface-ip $LAN.1 --iface-ip 10.77.0.1 > "$OUT/sharer-$l.log" 2>&1 &
}
# joiner_tray_block LABEL: the laptop app in a private mount ns. It pairs with
# the code, then the tray daemon takes over: the daemon owns the flags and the
# drop history. Runs until $OUT/kill-tray-LABEL exists.
joiner_tray_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    export XDG_RUNTIME_DIR="$ST/run-$1"
    # The connection log lives in the state dir, not in the config dir.
    export XDG_STATE_HOME="$ST/state-$1"
    mkdir -p "$XDG_CONFIG_HOME/jafnify" "$XDG_RUNTIME_DIR"
    chmod 0700 "$XDG_RUNTIME_DIR"
    export PATH="$2:$PATH"
    # The drop history switch is on: the daemon owns the flags.
    printf "drop_history = true\n" > "$XDG_CONFIG_HOME/jafnify/flags.toml"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-pair.log" 2>&1 &
    JP=$!
    n=0
    while [ $n -lt 120 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then break; fi
      sleep 0.5; n=$((n+1))
    done
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    "$JOIN" tray > "$OUT/joiner-$1-tray.log" 2>&1 &
    TP=$!
    n=0
    while [ $n -lt 240 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then
        : > "$OUT/tray-up-$1"
        break
      fi
      sleep 0.5; n=$((n+1))
    done
    while [ ! -f "$OUT/kill-tray-$1" ]; do sleep 0.5; done
    kill $TP 2>/dev/null || true
    wait $TP 2>/dev/null || true
  ' _ "$1" "$bin" &
}

# ---- the laptop is connected before the absence ------------------------------------
sharer_start away; sleep 1
joiner_tray_block away; BAWAY=$!
wait_for tray_up_away 240
res "A1 before the absence: the laptop is connected" internet_ok

# ---- the Wi-Fi of the laptop to the hotspot goes away --------------------------------
T0=$(date +%s)
ip netns exec jl-jo ip link set jl-lan down
# The absence is longer than the edge of the core, so the core writes the away.
if wait_for away_recorded 200; then
  res "A2 the absence is recorded as an away event, not as a drop" true
else
  res "A2 the absence is recorded as an away event, not as a drop" false
fi
res "A3 the absence is one record: one drop line and one away line" one_drop_line
res "A3b the absence is one record: one drop line and one away line" one_away_line

# ---- the absence lasts longer than five minutes --------------------------------------
SPENT=$(( $(date +%s) - T0 ))
if [ "$SPENT" -lt "$AWAY_SECONDS" ]; then sleep $(( AWAY_SECONDS - SPENT )); fi

# ---- the Wi-Fi of the laptop to the hotspot comes back --------------------------------
ip netns exec jl-jo ip link set jl-lan up
# Link down drops the default route; DHCP/NetworkManager restore it on a real laptop.
ip -n jl-jo route replace default via $LAN.1
# The measured length of the absence: from the link going down to the link
# coming back. The check is on the measured length, not on the constant.
MEASURED=$(( $(date +%s) - T0 ))
res "A3c the absence lasted longer than the edge of the core" \
  sh -c '[ "$1" -ge 120 ]' _ "$MEASURED"
T1=$(date +%s%N)
if wait_for internet_ok 120; then
  elapsed_since $T1 "A4 time from the link coming back to the internet reachable"
  res "A4 after the absence: connected again with no user action (within 120 s)" true
else
  res "A4 after the absence: connected again with no user action (within 120 s)" false
fi
wait_for back_recorded 60
res "A5 the absence is closed with a back line" back_recorded
res "A6 the daemon counts no drop for the absence" daemon_counts_no_drop
res "A7 the connection log says how long the phone was away" away_in_the_log

touch "$OUT/kill-tray-away"; wait $BAWAY
lab_assert_no_resolvectl "$OUT" "$OUT/joiner-away-pair.log" "$OUT/joiner-away-tray.log"

echo "---- the drop file of the laptop ----" | tee -a "$OUT/results.txt"
cat "$DROPS" 2>/dev/null | tee -a "$OUT/results.txt"
echo "---- the connection log of the laptop ----" | tee -a "$OUT/results.txt"
tail -20 "$ST/state-away/jafnify/connection.log" 2>/dev/null | tee -a "$OUT/results.txt"

if [ "$FAILS" -eq 0 ]; then
  echo "RESULT: PASS" | tee -a "$OUT/results.txt"
else
  echo "RESULT: FAIL ($FAILS)" | tee -a "$OUT/results.txt"
fi

# ---- the host must be untouched ------------------------------------------------------
ip -br addr show wg0 > "$OUT/host-wg0-after" 2>&1
diff "$OUT/host-wg0-before" "$OUT/host-wg0-after" && echo "wg0 unchanged"
ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"

lab_finish
echo "=== $FAILS failed ==="
exit $FAILS
