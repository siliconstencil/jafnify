#!/bin/bash
# Jafnify end-to-end lab (run as root). Fully isolated, host wg0/routes untouched:
#   jl-jo  (joiner, Linux app)   192.168.77.2 --veth-- 192.168.77.1  jl-sh (sharer harness)
#   jl-sh  10.200.0.2 --veth-- 10.200.0.1 jl-net ("internet": HTTP on 203.0.113.1:8080)
# jl-sh does NOT forward IP, so jl-jo can reach 203.0.113.1 only through the
# WireGuard tunnel into the sharer's userspace gateway (libwgslirpy).
# Inside jl-jo a private mount ns puts tmpfs over /etc/wireguard and HOME and
# shadows resolvconf with a no-op so no host DNS state can change.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-e2e
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
CODE=${CODE:-4321}
mkdir -p "$OUT"; rm -rf "$OUT"/*
lab_init "$OUT"

NS="jl-jo jl-sh jl-net"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
trap 'lab_finish; cleanup' EXIT
cleanup

echo "=== host before ==="; ip -br addr show wg0; ip route show default | head -2 > "$OUT/host-before"; cat "$OUT/host-before"

for n in $NS; do ip netns add $n; ip -n $n link set lo up; done
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

mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
ip netns exec jl-jo tcpdump -i jl-a -U -w "$OUT/lan.pcap" 'udp' >/dev/null 2>&1 &
sleep 1

echo "=== control: joiner -> 203.0.113.1 without tunnel (must fail) ==="
ip netns exec jl-jo curl -s -m 3 http://203.0.113.1:8080/ && echo "UNEXPECTED: reachable" || echo "unreachable (good)"

ip netns exec jl-sh timeout 70 "$SHARE" --code "$CODE" --iface-ip 192.168.77.1 > "$OUT/sharer.log" 2>&1 &
sleep 1

joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d) XDG_CONFIG_HOME=
  timeout 60 '"$JOIN"' join --accept-terms --code '"$CODE"' > '"$OUT"'/joiner.log 2>&1 &
  JP=$!
  for i in $(seq 1 40); do ip link show jafn0 >/dev/null 2>&1 && break; sleep 1; done
  echo "jafn0 after ${i}s:"; ip -br addr show jafn0 || echo "no jafn0"
  sleep 2
  echo "=== joiner -> 203.0.113.1 THROUGH tunnel ==="
  curl -s -m 8 http://203.0.113.1:8080/ | tee '"$OUT"'/through.txt || echo "curl failed"
  echo "=== wg show (keys redacted) ==="
  wg show jafn0 2>&1 | sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g"
  echo "=== route to 203.0.113.1 ==="; ip route get 203.0.113.1
  echo "--- generated conf (keys redacted) ---"
  sed -E "s#(=\s*)[A-Za-z0-9+/]{43}=#\1<REDACTED>#" /etc/wireguard/*.conf 2>/dev/null
  kill $JP 2>/dev/null; wait $JP 2>/dev/null
  wg-quick down jafn0 >/dev/null 2>&1
' 2>&1
sleep 1
for p in $(ip netns pids jl-jo); do kill $p 2>/dev/null; done; sleep 0.5

echo "=== joiner log ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner.log"
echo "=== sharer log ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/sharer.log" | tail -30
echo "=== http server log ==="; cat "$OUT/http.log"
echo "=== LAN udp (src, sport, dport, first byte) ==="
python3 -I - "$OUT/lan.pcap" <<'PY'
import sys, struct, collections
d = open(sys.argv[1], 'rb').read()
off = 24; c = collections.Counter()
while off + 16 <= len(d):
    incl = struct.unpack('<I', d[off+8:off+12])[0]
    pkt = d[off+16:off+16+incl]; off += 16 + incl
    ihl = (pkt[14] & 0x0f) * 4
    sport, dport = struct.unpack('>HH', pkt[14+ihl:14+ihl+4])
    src = '.'.join(map(str, pkt[26:30])); payload = pkt[14+ihl+8:]
    c[(src, sport, dport, '0x%02x' % payload[0] if payload else '-')] += 1
for k, v in sorted(c.items()): print(k, v)
PY
echo "=== host after ==="; ip -br addr show wg0; ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner*.log
if grep -q JAFNIFY-E2E-OK "$OUT/through.txt" 2>/dev/null; then echo "PASS  data flows through the paired tunnel"; else echo "FAIL  no data through the tunnel after pairing"; FAILS=$((FAILS + 1)); fi
lab_finish
exit $FAILS
