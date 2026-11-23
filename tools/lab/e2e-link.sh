#!/bin/bash
# Jafnify link-quality lab (run as root). Same isolated 3-namespace layout as
# e2e-loss.sh; host wg0/routes untouched. Scenarios:
#   N1: netem loss 5%, delay 30ms +/- 20ms, reorder 2% on both
#      LAN veth ends; 20 sequential 1 MiB HTTP GETs, at least 19 intact;
#      jafn0 sampled every second and never torn down.
#   N2 harsher: loss 15%, delay 80ms +/- 40ms; tunnel stays up for 60 s of idle
#      plus 5 GETs (at least 3 intact); after removing netem a GET succeeds
#      within 10 s.
#   N3: MTU 1280 on both LAN veth ends and the jl-sh -> jl-net
#      link; known reconnect; 4 MiB download and 1 MiB upload intact; record
#      the jafn0 MTU.
#   N4 path-MTU black hole: MTU 1280 on jl-sh -> jl-net only, ICMP
#      fragmentation-needed dropped in jl-sh only; 4 MiB download intact.
#   N5 D2 "Test the link now": 10 probes under netem delay 80ms +/- 20ms on
#      both LAN ends, average within 128-192 ms, then 100% loss says
#      "No reply"; the tunnel recovers after netem is removed.
# PASS/FAIL lines match the other lab scripts; exits non-zero on any FAIL.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-link
mkdir -p "$OUT"; rm -rf "$OUT"/* "$OUT"/.tmp
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
CODE=${CODE:-4321}
ST=$OUT/state
mkdir -p "$ST/jo" "$ST/sh"
lab_init "$OUT"
FAILS=0

NS="jl-jo jl-sh jl-net"
cleanup() {
  if [ -n "${JOINER_WRAPPER:-}" ]; then kill "$JOINER_WRAPPER" 2>/dev/null; fi
  if [ -n "${SHARER_PID:-}" ]; then kill "$SHARER_PID" 2>/dev/null; fi
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
trap 'lab_finish; cleanup; rm -rf "$ST"' EXIT
cleanup

pass() { echo "PASS  $*" | tee -a "$OUT/results.txt"; }
fail() { echo "FAIL  $*" | tee -a "$OUT/results.txt"; FAILS=$((FAILS + 1)); }
note() { echo "NOTE  $*" | tee -a "$OUT/results.txt"; }

http_ok() {
  ip netns exec jl-jo curl -sS -m 3 -o /dev/null -w "%{http_code}" http://203.0.113.1:8080/index.html 2>/dev/null | grep -q 200
}
http_ok_fast() {
  ip netns exec jl-jo curl -sS -m 1 -o /dev/null -w "%{http_code}" http://203.0.113.1:8080/index.html 2>/dev/null | grep -q 200
}
wait_http() {
  local n=$1 i=0
  while [ "$i" -lt "$n" ]; do
    if http_ok; then return 0; fi
    i=$((i + 1)); sleep 1
  done
  return 1
}
wait_connected() {
  local n=$1 i=0
  while [ "$i" -lt "$n" ]; do
    if grep -q Connected "$OUT/joiner.log" 2>/dev/null; then return 0; fi
    i=$((i + 1)); sleep 1
  done
  return 1
}
link_up() { ip netns exec jl-jo ip link show jafn0 >/dev/null 2>&1; }
wait_link() {
  local n=$1 i=0
  while [ "$i" -lt "$n" ]; do
    if link_up; then return 0; fi
    i=$((i + 1)); sleep 1
  done
  return 1
}

start_sampler() {
  local phase=$1
  : > "$OUT/$phase-link.txt"
  rm -f "$OUT/.$phase-stop"
  (
    while [ ! -e "$OUT/.$phase-stop" ]; do
      if link_up; then echo up >> "$OUT/$phase-link.txt"; else echo down >> "$OUT/$phase-link.txt"; fi
      sleep 1
    done
  ) &
  SAMPLER_PID=$!
}
stop_sampler() {
  local phase=$1
  touch "$OUT/.$phase-stop"
  wait "$SAMPLER_PID" 2>/dev/null
}
check_samples() {
  local phase=$1 up down total
  up=$(grep -cx up "$OUT/$phase-link.txt" 2>/dev/null || true)
  down=$(grep -cx down "$OUT/$phase-link.txt" 2>/dev/null || true)
  total=$((up + down))
  if [ "$total" -eq 0 ]; then
    fail "$phase jafn0 was not sampled"
  elif [ "$down" -eq 0 ]; then
    pass "$phase jafn0 stayed up ($up samples)"
  else
    fail "$phase jafn0 disappeared ($down down samples of $total)"
  fi
}

netem() {
  local ns=$1 dev=$2 loss=$3 delay=$4 reorder=${5:-}
  local base="loss $loss delay $delay"
  local with_dist="$base distribution normal"
  local plain="$base"
  if [ -n "$reorder" ]; then
    with_dist="$with_dist reorder $reorder"
    plain="$plain reorder $reorder"
  fi
  if ip netns exec "$ns" tc qdisc replace dev "$dev" root netem $with_dist 2>/dev/null; then
    echo "tc $ns $dev: $with_dist" >> "$OUT/tc.txt"
  elif ip netns exec "$ns" tc qdisc replace dev "$dev" root netem $plain 2>>"$OUT/tc.err"; then
    echo "tc $ns $dev: $plain" >> "$OUT/tc.txt"
  else
    fail "netem on $dev ($ns) could not be installed"
  fi
}
del_netem() {
  local ns=$1 dev=$2
  ip netns exec "$ns" tc qdisc del dev "$dev" root 2>/dev/null || true
  echo "tc $ns $dev: del root" >> "$OUT/tc.txt"
}

SHARER_N=0
SHARER_PID=
start_sharer() {
  SHARER_N=$((SHARER_N + 1))
  SHARER_LOG="$OUT/sharer-$SHARER_N.log"
  ip netns exec jl-sh "$SHARE" --iface-ip 192.168.77.1 --state-dir "$ST/sh" "$@" > "$SHARER_LOG" 2>&1 &
  SHARER_PID=$!
  echo "start sharer $SHARER_N args=$* pid=$SHARER_PID" >> "$OUT/events.txt"
}
stop_sharer() {
  if [ -n "${SHARER_PID:-}" ]; then kill "$SHARER_PID" 2>/dev/null; wait "$SHARER_PID" 2>/dev/null; fi
  for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p 2>/dev/null; done
  sleep 1
  SHARER_PID=
}

JOINER_WRAPPER=
start_joiner() {
  local args=$1
  JOIN_ARGS=$args
  export OUT JOIN ST CODE JOIN_ARGS
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d -p "$ST") XDG_CONFIG_HOME="$ST/jo"
    timeout 1800 "$JOIN" join --accept-terms $JOIN_ARGS > "$OUT/joiner.log" 2>&1 &
    echo $! > "$OUT/joiner.pid"
    wait
  ' > "$OUT/joiner-wrapper.log" 2>&1 &
  JOINER_WRAPPER=$!
}
wait_for_joiner_pid() {
  local i=0
  while [ "$i" -lt 20 ]; do
    if [ -s "$OUT/joiner.pid" ]; then return 0; fi
    i=$((i + 1)); sleep 1
  done
  return 1
}

echo "=== host before ==="
ip -br addr show wg0 | tee "$OUT/host-wg0-before"
ip route show default | head -2 > "$OUT/host-before"
cat "$OUT/host-before"

for n in $NS; do ip netns add $n; ip -n $n link set lo up; done
# A new namespace disables unprivileged ping; desktop Ubuntu allows it. N5 needs it.
ip netns exec jl-jo sysctl -qw net.ipv4.ping_group_range="0 2147483647"
ip link add jl-a type veth peer name jl-b
ip link set jl-a netns jl-jo; ip link set jl-b netns jl-sh
ip -n jl-jo addr add 192.168.77.2/24 dev jl-a; ip -n jl-jo link set jl-a up
ip -n jl-sh addr add 192.168.77.1/24 dev jl-b; ip -n jl-sh link set jl-b up
ip -n jl-jo route add default via 192.168.77.1
ip link add jl-c type veth peer name jl-d
ip link set jl-c netns jl-sh; ip link set jl-d netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-c; ip -n jl-sh link set jl-c up
ip -n jl-net addr add 10.200.0.1/24 dev jl-d; ip -n jl-net link set jl-d up
ip -n jl-net addr add 203.0.113.1/24 dev jl-d
ip -n jl-sh route add default via 10.200.0.1
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0

mkdir -p "$OUT/www"
echo "JAFNIFY-LINK-OK" > "$OUT/www/index.html"
head -c 1048576 /dev/urandom > "$OUT/www/one.bin"
head -c 4194304 /dev/urandom > "$OUT/www/four.bin"
head -c 1048576 /dev/urandom > "$OUT/upload-src.bin"
ONE_SHA=$(sha256sum "$OUT/www/one.bin" | cut -d" " -f1)
FOUR_SHA=$(sha256sum "$OUT/www/four.bin" | cut -d" " -f1)
UP_SHA=$(sha256sum "$OUT/upload-src.bin" | cut -d" " -f1)

ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
ip netns exec jl-net python3 -I -c '
import http.server, hashlib, sys
class Upload(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        with open(sys.argv[1], "wb") as f:
            f.write(body)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(hashlib.sha256(body).hexdigest().encode())
    def log_message(self, *args):
        pass
http.server.HTTPServer((sys.argv[2], 8081), Upload).serve_forever()
' "$OUT/www/uploaded.bin" 203.0.113.1 > "$OUT/upload.log" 2>&1 &
sleep 1

start_sharer --code "$CODE"
sleep 1
start_joiner "--code $CODE"
if ! wait_for_joiner_pid; then
  fail "pairing: joiner process did not start"
  echo "=== joiner wrapper log ==="; cat "$OUT/joiner-wrapper.log" 2>/dev/null || true
  exit $FAILS
fi
if ! wait_http 90; then
  fail "pairing: tunnel did not come up within 90 s"
  echo "=== joiner log ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner.log" 2>/dev/null | tail -30 || true
  exit $FAILS
fi
if wait_connected 90; then
  pass "pairing: joiner reports Connected"
else
  fail "pairing: joiner log has no Connected within 90 s"
fi

echo "=== N1: loss 5%, delay 30ms +/- 20ms, reorder 2% ===" | tee -a "$OUT/results.txt"
netem jl-jo jl-a "5%" "30ms 20ms" "2%"
netem jl-sh jl-b "5%" "30ms 20ms" "2%"
if wait_link 10; then
  pass "N1 jafn0 present before downloads"
else
  fail "N1 jafn0 missing before downloads"
fi
start_sampler n1
: > "$OUT/n1-times.txt"; : > "$OUT/n1-fail.txt"; N1_OK=0
for i in $(seq 1 20); do
  RC=0
  T=$(ip netns exec jl-jo curl -sS -m 45 -o "$OUT/n1-$i.bin" -w "%{time_total}" http://203.0.113.1:8080/one.bin 2>>"$OUT/n1-curl.err") || RC=$?
  GOT=$(sha256sum "$OUT/n1-$i.bin" 2>/dev/null | cut -d" " -f1)
  if [ "$RC" -eq 0 ] && [ "$GOT" = "$ONE_SHA" ]; then
    N1_OK=$((N1_OK + 1)); echo "$T" >> "$OUT/n1-times.txt"
  else
    echo "get $i rc=$RC time=$T sha=$GOT" >> "$OUT/n1-fail.txt"
  fi
done
sleep 1
stop_sampler n1
check_samples n1
if [ "$N1_OK" -ge 19 ]; then
  pass "N1 20 sequential 1 MiB GETs: $N1_OK/20 intact"
else
  fail "N1 20 sequential 1 MiB GETs: only $N1_OK/20 intact"
fi
N1_MEDIAN=$(sort -n "$OUT/n1-times.txt" | awk '{a[NR]=$1} END {if (NR==0) {print "n/a"; exit} if (NR%2) printf "%.3f", a[(NR+1)/2]; else printf "%.3f", (a[NR/2]+a[NR/2+1])/2}')
N1_MAX=$(sort -n "$OUT/n1-times.txt" | tail -1)
note "N1 transfer times: median=${N1_MEDIAN}s max=${N1_MAX:-n/a}s"

echo "=== N2: loss 15%, delay 80ms +/- 40ms ===" | tee -a "$OUT/results.txt"
netem jl-jo jl-a "15%" "80ms 40ms"
netem jl-sh jl-b "15%" "80ms 40ms"
start_sampler n2
sleep 60
N2_OK=0; : > "$OUT/n2-fail.txt"
for i in $(seq 1 5); do
  RC=0
  ip netns exec jl-jo curl -sS -m 45 -o "$OUT/n2-$i.bin" http://203.0.113.1:8080/one.bin 2>>"$OUT/n2-curl.err" || RC=$?
  GOT=$(sha256sum "$OUT/n2-$i.bin" 2>/dev/null | cut -d" " -f1)
  if [ "$RC" -eq 0 ] && [ "$GOT" = "$ONE_SHA" ]; then
    N2_OK=$((N2_OK + 1))
  else
    echo "get $i rc=$RC sha=$GOT" >> "$OUT/n2-fail.txt"
  fi
done
stop_sampler n2
check_samples n2
if [ "$N2_OK" -ge 3 ]; then
  pass "N2 5 GETs after 60 s idle: $N2_OK/5 intact"
else
  fail "N2 5 GETs after 60 s idle: only $N2_OK/5 intact"
fi
del_netem jl-jo jl-a
del_netem jl-sh jl-b
DEADLINE=$(( $(date +%s) + 10 ))
RECOVER=0
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  if http_ok_fast; then RECOVER=1; break; fi
  sleep 1
done
if [ "$RECOVER" -eq 1 ]; then
  pass "N2 GET succeeds within 10 s after netem removal"
else
  fail "N2 GET did not succeed within 10 s after netem removal"
fi

echo "=== N5: link test under netem delay 80ms +/- 20ms ===" | tee -a "$OUT/results.txt"
LINK_HOME="$ST/link-home"; mkdir -p "$LINK_HOME" "$ST/link-run"
link_test() {
  joiner_sandbox jl-jo env XDG_RUNTIME_DIR="$ST/link-run" \
    XDG_CONFIG_HOME="$ST/jo" HOME="$LINK_HOME" "$JOIN" link-test 2>>"$OUT/link-test.err"
}
netem jl-jo jl-a "0%" "80ms 20ms"
netem jl-sh jl-b "0%" "80ms 20ms"
if wait_link 10; then
  pass "N5 jafn0 present before the link test"
else
  fail "N5 jafn0 missing before the link test"
fi
LINK_OUT=$(link_test) || true
echo "$LINK_OUT" | tee -a "$OUT/results.txt"
if echo "$LINK_OUT" | grep -q "0% loss"; then
  pass "N5 link test reports 0% loss"
else
  fail "N5 link test did not report 0% loss: $LINK_OUT"
fi
LINK_AVG=$(echo "$LINK_OUT" | sed -n 's/.*average \([0-9][0-9]*\) ms.*/\1/p')
if [ -n "$LINK_AVG" ] && [ "$LINK_AVG" -ge 128 ] && [ "$LINK_AVG" -le 192 ]; then
  pass "N5 average ${LINK_AVG} ms is within 20 percent of 160 ms"
else
  fail "N5 average ${LINK_AVG:-missing} ms is outside 128-192 ms"
fi
netem jl-jo jl-a "100%" "0ms"
netem jl-sh jl-b "100%" "0ms"
LINK_LOSS=$(link_test) || true
echo "$LINK_LOSS" | tee -a "$OUT/results.txt"
if echo "$LINK_LOSS" | grep -q "No reply"; then
  pass "N5 100% loss reports No reply"
else
  fail "N5 100% loss did not report No reply: $LINK_LOSS"
fi
del_netem jl-jo jl-a
del_netem jl-sh jl-b
if wait_http 40; then
  pass "N5 tunnel recovers after the loss case"
else
  fail "N5 tunnel did not recover after the loss case"
fi

echo "=== N3: MTU 1280 on both LAN ends and jl-sh -> jl-net ===" | tee -a "$OUT/results.txt"
ip -n jl-jo link set jl-a mtu 1280
ip -n jl-sh link set jl-b mtu 1280
ip -n jl-sh link set jl-c mtu 1280
ip -n jl-net link set jl-d mtu 1280
for spec in "jl-jo jl-a" "jl-sh jl-b" "jl-sh jl-c" "jl-net jl-d"; do
  set -- $spec
  ip -n "$1" link show "$2" | sed -n "s/.* mtu \([0-9]*\).*/$1 $2 mtu \1/p" | head -1
done | tee "$OUT/n3-mtus.txt"
stop_sharer
sleep 1
start_sharer
if wait_http 40; then
  pass "N3 known reconnect after the MTU change"
else
  fail "N3 known reconnect after the MTU change"
fi
RC=0
ip netns exec jl-jo curl -sS -m 180 -o "$OUT/n3-four.bin" http://203.0.113.1:8080/four.bin 2>>"$OUT/n3-curl.err" || RC=$?
GOT=$(sha256sum "$OUT/n3-four.bin" 2>/dev/null | cut -d" " -f1)
if [ "$RC" -eq 0 ] && [ "$GOT" = "$FOUR_SHA" ]; then
  pass "N3 4 MiB download intact"
else
  fail "N3 4 MiB download rc=$RC sha=$GOT"
fi
RC=0
HTTP=$(ip netns exec jl-jo curl -sS -m 180 -o "$OUT/n3-upload-response.txt" -w "%{http_code}" --data-binary @"$OUT/upload-src.bin" http://203.0.113.1:8081/upload 2>>"$OUT/n3-curl.err") || RC=$?
GOT=$(sha256sum "$OUT/www/uploaded.bin" 2>/dev/null | cut -d" " -f1)
if [ "$RC" -eq 0 ] && [ "$HTTP" = "200" ] && [ "$GOT" = "$UP_SHA" ]; then
  pass "N3 1 MiB upload intact"
else
  fail "N3 1 MiB upload rc=$RC http=$HTTP sha=$GOT"
fi
JAFN0_MTU=$(ip netns exec jl-jo ip link show jafn0 | sed -n "s/.* mtu \([0-9]*\).*/\1/p" | head -1)
if [ -n "$JAFN0_MTU" ] && [ "$JAFN0_MTU" -le 1200 ]; then
  pass "N3 jafn0 MTU $JAFN0_MTU <= 1200"
else
  note "N3 jafn0 MTU ${JAFN0_MTU:-unknown} > 1200; product sets 1280; download and upload are the evidence"
fi
note "N3 ping -M do is not used as a pass criterion (ICMP to the internet is not forwarded)"

echo "=== N4: path-MTU black hole on jl-sh -> jl-net ===" | tee -a "$OUT/results.txt"
ip -n jl-jo link set jl-a mtu 1500
ip -n jl-sh link set jl-b mtu 1500
ip -n jl-sh link set jl-c mtu 1280
ip -n jl-net link set jl-d mtu 1280
ip netns exec jl-sh nft delete table inet jllink 2>/dev/null || true
if ! ip netns exec jl-sh nft -f - <<'NFT'
table inet jllink {
  chain input {
    type filter hook input priority 0; policy accept;
    icmp type destination-unreachable icmp code frag-needed drop
  }
}
NFT
then
  fail "N4 nft ICMP fragmentation-needed drop could not be installed"
else
  pass "N4 nft ICMP fragmentation-needed drop installed in jl-sh"
  ip netns exec jl-sh nft list table inet jllink > "$OUT/n4-nft.txt"
fi
if ! wait_http 20; then
  stop_sharer
  sleep 1
  start_sharer
  if ! wait_http 40; then
    fail "N4 tunnel did not come back after the black hole was installed"
  fi
fi
RC=0
ip netns exec jl-jo curl -sS -m 180 -o "$OUT/n4-four.bin" http://203.0.113.1:8080/four.bin 2>>"$OUT/n4-curl.err" || RC=$?
GOT=$(sha256sum "$OUT/n4-four.bin" 2>/dev/null | cut -d" " -f1)
if [ "$RC" -eq 0 ] && [ "$GOT" = "$FOUR_SHA" ]; then
  pass "N4 4 MiB download intact with the PMTU black hole"
else
  fail "N4 4 MiB download rc=$RC sha=$GOT"
fi

echo "=== host after ==="
ip -br addr show wg0 | tee "$OUT/host-wg0-after"
if diff -q "$OUT/host-wg0-before" "$OUT/host-wg0-after" >/dev/null; then
  pass "host wg0 unchanged"
else
  fail "host wg0 changed"
fi
if ip route show default | head -2 | diff -q "$OUT/host-before" - >/dev/null; then
  pass "host default route unchanged"
else
  fail "host default route changed"
fi
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner*.log

echo "=== netem/tc commands used ==="; cat "$OUT/tc.txt" 2>/dev/null || true
echo "=== N3 link MTUs ==="; cat "$OUT/n3-mtus.txt" 2>/dev/null || true
echo "=== N4 nft table ==="; cat "$OUT/n4-nft.txt" 2>/dev/null || true
echo "=== joiner log (tail) ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner.log" 2>/dev/null | tail -20 || true
echo "=== sharer log (tail) ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/sharer-$SHARER_N.log" 2>/dev/null | tail -12 || true

lab_finish
echo "=== results ==="; cat "$OUT/results.txt"
exit $FAILS
