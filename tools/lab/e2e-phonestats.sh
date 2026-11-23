#!/bin/bash
# Jafnify phone measurements lab (run as root). Isolated namespaces; host
# wg0/routes untouched. State dirs hold throwaway lab keys only, never printed,
# deleted at the end.
#   run 1: pair with code 4321 and read the lab fake summary over the tunnel
#   run 2: a third namespace on the hotspot LAN cannot reach the responder at
#          the gateway address or at the sharer's LAN address
#   run 3: ten requests in one second get at most three replies
#   run 4: a sharer with no provider leaves the paired joiner with no value
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-phonestats
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
ST=$OUT/state
mkdir -p "$OUT"; rm -rf "$OUT"/*; mkdir -p "$ST/jo" "$ST/sh" "$ST/x" "$ST/jo-home"
lab_init "$OUT"

FAKE='JFS1 1 summary v=1 net=5g sig=3 rtt=45 up=123456 down=654321 batt=80 temp=315 used=123456789 allow=10737418240 reset=12 month=24323'

NS="jl-jo jl-sh jl-net jl-x"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
stop_joiner() {
  if [ -n "${JP:-}" ]; then
    kill "$JP" 2>/dev/null
    wait "$JP" 2>/dev/null
  fi
  ip -n jl-jo link del jafn0 2>/dev/null
  JP=
}
stop_sharer() {
  if [ -n "${SP:-}" ]; then
    kill "$SP" 2>/dev/null
    wait "$SP" 2>/dev/null
  fi
  for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p; done
  sleep 1
  SP=
}
trap 'stop_joiner; stop_sharer; cleanup; lab_finish; [ -z "${KEEP:-}" ] && rm -rf "$ST"' EXIT
cleanup
ip route show default | head -2 > "$OUT/host-before"

# The LAN is a bridge inside jl-sh; joiner (jl-jo) and stranger (jl-x) hang off it.
for n in $NS; do ip netns add $n; ip -n $n link set lo up; done
ip -n jl-sh link add jl-br type bridge; ip -n jl-sh addr add 192.168.77.1/24 dev jl-br; ip -n jl-sh link set jl-br up
for spec in "jl-jo 192.168.77.2 a" "jl-x 192.168.77.3 x"; do
  set -- $spec
  ip link add jl-$3 type veth peer name jl-$3b
  ip link set jl-$3 netns $1; ip link set jl-$3b netns jl-sh
  ip -n $1 addr add $2/24 dev jl-$3; ip -n $1 link set jl-$3 up
  ip -n jl-sh link set jl-$3b master jl-br; ip -n jl-sh link set jl-$3b up
  ip -n $1 route add default via 192.168.77.1
done
ip link add jl-c type veth peer name jl-d
ip link set jl-c netns jl-sh; ip link set jl-d netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-c; ip -n jl-sh link set jl-c up
ip -n jl-net addr add 10.200.0.1/24 dev jl-d; ip -n jl-net link set jl-d up
ip -n jl-net addr add 203.0.113.1/24 dev jl-d
ip -n jl-sh route add default via 10.200.0.1
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0
mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &

sharer() { ip netns exec jl-sh "$SHARE" --iface-ip 192.168.77.1 --state-dir "$ST/sh" "$@" > "$OUT/sharer-$L.log" 2>&1 & SP=$!; }
start_joiner() {
  joiner_sandbox jl-jo bash -c '
    export HOME='"$ST"'/jo-home XDG_CONFIG_HOME='"$ST"'/jo
    exec timeout 180 "'"$JOIN"'" join --accept-terms '"$1"' > "'"$OUT"'/joiner.log" 2>&1
  ' &
  JP=$!
}
wait_tunnel() {
  for i in $(seq 1 "$1"); do
    if ip netns exec jl-jo ip link show jafn0 >/dev/null 2>&1; then
      C=$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || true)
      [ "$C" = JAFNIFY-E2E-OK ] && return 0
    fi
    sleep 1
  done
  return 1
}
phone_stats() {
  joiner_sandbox jl-jo bash -c '
    export HOME='"$ST"'/jo-home XDG_CONFIG_HOME='"$ST"'/jo
    "'"$JOIN"'" phone-stats
  '
}
udp_query() { # namespace target count timeout -> number of replies
  ip netns exec "$1" python3 -I - "$2" "$3" "$4" <<'PY'
import socket, sys
target, count, timeout = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(timeout)
# Send the whole burst first, then count what comes back, so a rate limit is
# measured as a burst and not as one request per timeout.
for _ in range(count):
    sock.sendto(b"JFS1 1 summary", (target, 9798))
replies = 0
while True:
    try:
        sock.recvfrom(1024)
        replies += 1
    except OSError:
        break
print(replies)
PY
}
check() {
  if [ "$2" = 1 ]; then
    echo "PASS  $1" | tee -a "$OUT/results.txt"
  else
    echo "FAIL  $1" | tee -a "$OUT/results.txt"
    FAILS=$((FAILS + 1))
  fi
}

# --- run 1: the paired joiner reads the lab fake summary ---------------------
L=fake; sharer --code 4321 --stats-fake-line "$FAKE"; sleep 1
start_joiner "--code 4321"
if wait_tunnel 40; then
  PHONE=$(phone_stats)
  echo "phone-stats: $PHONE" >> "$OUT/summary.txt"
  case "$PHONE" in
    *"net=5g"*"sig=3"*) check "paired joiner reads the fake summary" 1 ;;
    *) check "paired joiner reads the fake summary" 0 ;;
  esac
else
  echo "joiner never reached the tunnel" >> "$OUT/summary.txt"
  check "paired joiner reads the fake summary" 0
fi

# --- run 2: a LAN stranger cannot reach the responder ------------------------
X_GW=$(udp_query jl-x 10.233.0.1 1 1)
X_LAN=$(udp_query jl-x 192.168.77.1 1 1)
echo "stranger replies: gateway=$X_GW lan=$X_LAN" >> "$OUT/summary.txt"
if [ "$X_GW" = 0 ] && [ "$X_LAN" = 0 ]; then
  check "stranger replies" 1
else
  check "stranger replies" 0
fi

# --- run 3: the responder admits at most two or three requests per second ---
RATE=$(udp_query jl-jo 10.233.0.1 10 1)
echo "rate replies: $RATE of 10" >> "$OUT/summary.txt"
if [ "$RATE" -le 3 ]; then
  check "rate replies" 1
else
  check "rate replies" 0
fi

# --- run 4: a sharer with no provider answers nothing ------------------------
stop_joiner
stop_sharer
L=nofake; sharer
start_joiner ""
if wait_tunnel 40; then
  PHONE=$(phone_stats)
  echo "phone-stats (no provider): $PHONE" >> "$OUT/summary.txt"
  case "$PHONE" in
    *unavailable*) check "no provider" 1 ;;
    *) check "no provider" 0 ;;
  esac
else
  echo "joiner never reconnected without a provider" >> "$OUT/summary.txt"
  check "no provider" 0
fi
stop_joiner

echo "=== summary ==="; cat "$OUT/summary.txt"
echo "=== results ==="; cat "$OUT/results.txt"
[ "$FAILS" = 0 ] && exit 0 || exit 1
