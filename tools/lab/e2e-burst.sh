#!/bin/bash
# Burst-load lab (run as root): reproduces a tunnel freeze under burst load, where a
# burst of downstream UDP (browser QUIC, several tabs at once) plus TCP stalled
# the sharer gateway for good and both sides reported "no data for 35 s".
# Topology as e2e-pair.sh. jl-net runs a UDP blaster (each request datagram is
# answered with BURST datagrams of 1200 bytes, sent as fast as possible) and an
# HTTP server with a 20 MB file. The joiner runs ROUNDS rounds of CLIENTS
# parallel UDP bursts plus 4 parallel TCP downloads, and after every round
# checks that a small HTTP request still gets through within 5 s.
# PASS: every liveness check succeeds. FAIL: the tunnel stops carrying traffic.
#   SHARE=/path/to/jafnify-sharer-host sudo -E bash tools/lab/e2e-burst.sh
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-burst
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
CODE=${CODE:-4321}
ROUNDS=${ROUNDS:-6}; CLIENTS=${CLIENTS:-9}; BURST=${BURST:-3000}
mkdir -p "$OUT"; rm -rf "$OUT"/*
lab_init "$OUT"

NS="jl-jo jl-sh jl-net"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
trap 'cleanup' EXIT
cleanup

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
# Hotspot-like link: limit the sharer->joiner direction (RATE, default 40mbit).
ip netns exec jl-sh tc qdisc add dev jl-b root netem rate ${RATE:-40mbit} delay 3ms limit 200

mkdir -p "$OUT/www" "$OUT/py"
echo "ALIVE" > "$OUT/www/index.html"
head -c 20000000 /dev/urandom > "$OUT/www/big.bin"
cat > "$OUT/py/blaster.py" <<'PY'
import socket, sys
burst = int(sys.argv[1]); s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("203.0.113.1", 4433)); data = b"x" * 1200
while True:
    msg, addr = s.recvfrom(2048)
    if msg != b"go": continue
    for _ in range(burst):
        try: s.sendto(data, addr)
        except OSError: pass
PY
cat > "$OUT/py/client.py" <<'PY'
import socket, sys, time, threading
# Downstream burst plus continuous upstream datagrams (QUIC-like ACK/upload
# pressure from the joiner) at the same time.
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(2)
dst = ("203.0.113.1", 4433); stop = time.time() + 8
def up():
    d = b"u" * 1200
    while time.time() < stop:
        try: s.sendto(d, dst)
        except OSError: pass
s.sendto(b"go", dst); threading.Thread(target=up, daemon=True).start(); n = 0
while time.time() < stop:
    try: s.recv(2048); n += 1
    except socket.timeout: break
print(n)
PY
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
ip netns exec jl-net python3 -I "$OUT/py/blaster.py" "$BURST" > "$OUT/blaster.log" 2>&1 &
sleep 1

ip netns exec jl-sh timeout 600 "$SHARE" --code "$CODE" --iface-ip 192.168.77.1 > "$OUT/sharer.log" 2>&1 &
sleep 1

joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d) XDG_CONFIG_HOME=
  timeout 580 '"$JOIN"' join --accept-terms --code '"$CODE"' > '"$OUT"'/joiner.log 2>&1 &
  JP=$!
  for i in $(seq 1 40); do ip link show jafn0 >/dev/null 2>&1 && break; sleep 1; done
  sleep 2
  curl -s -m 8 http://203.0.113.1:8080/ | grep -q ALIVE && echo "BASELINE ok" || echo "BASELINE failed"
  for r in $(seq 1 '"$ROUNDS"'); do
    for c in $(seq 1 '"$CLIENTS"'); do python3 -I '"$OUT"'/py/client.py > '"$OUT"'/udp-$r-$c.txt & done
    for t in 1 2 3 4; do curl -s -m 15 -o /dev/null -w "%{size_download}\n" http://203.0.113.1:8080/big.bin > '"$OUT"'/tcp-$r-$t.txt & done
    wait $(jobs -p | grep -v "^$JP$") 2>/dev/null
    udp=$(cat '"$OUT"'/udp-$r-*.txt | paste -sd+ | bc); tcp=$(cat '"$OUT"'/tcp-$r-*.txt | paste -sd+ | bc)
    if curl -s -m 5 http://203.0.113.1:8080/ | grep -q ALIVE; then echo "ROUND $r PASS udp_pkts=$udp tcp_bytes=$tcp"; else echo "ROUND $r FAIL tunnel dead udp_pkts=$udp tcp_bytes=$tcp"; fi
  done
  kill $JP 2>/dev/null; wait $JP 2>/dev/null
  wg-quick down jafn0 >/dev/null 2>&1
' 2>&1 | tee "$OUT/result.txt"
grep -q "BASELINE ok" "$OUT/result.txt" || FAILS=$((FAILS+1))
n=$(grep -c "FAIL" "$OUT/result.txt"); FAILS=$((FAILS+n))
echo "=== joiner log (tail) ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner.log" | tail -8
[ "$FAILS" -eq 0 ] && echo "RESULT: PASS" || echo "RESULT: FAIL ($FAILS)"
exit $FAILS
