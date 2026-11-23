#!/bin/bash
# Jafnify connection-loss lab (run as root). Same isolated 3-namespace layout as
# e2e-pair.sh; host wg0/routes untouched. Scenario:
#   pair (code) -> tunnel up -> kill sharer -> joiner must take jafn0 down and
#   restore jl-jo's normal routing -> restart sharer with NO code (stored pair) ->
#   joiner must reconnect on its own (known peer) -> HTTP through the tunnel again.
# State dirs hold throwaway lab keys only and are deleted at exit.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-loss
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
CODE=${CODE:-4321}
ST=$OUT/state
mkdir -p "$OUT"; rm -rf "$OUT"/* "$OUT"/.kill "$OUT"/.restart; mkdir -p "$ST/jo" "$ST/sh"
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

# start_sharer N [--code C]: first start pairs with the code, the restart uses the stored pair.
start_sharer() { local n=$1; shift; ip netns exec jl-sh "$SHARE" --iface-ip 192.168.77.1 --state-dir "$ST/sh" "$@" > "$OUT/sharer$n.log" 2>&1 & }
ip netns exec jl-jo tcpdump -i jl-a -U -w "$OUT/lan.pcap" udp >/dev/null 2>&1 &
sleep 1
start_sharer 1 --code "$CODE"
sleep 1

# Joiner + probes run through the shared sandbox prelude.
joiner_sandbox jl-jo bash -c '
  OUT='"$OUT"'
  export HOME=$(mktemp -d) XDG_CONFIG_HOME='"$ST/jo"'
  timeout 170 '"$JOIN"' join --accept-terms --code '"$CODE"' > $OUT/joiner.log 2>&1 &
  T0=$(date +%s)
  probe() {
    J=$(ip link show jafn0 >/dev/null 2>&1 && echo up || echo down)
    C=$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)
    R=$(ip rule | grep -c 51820)
    echo "t=$(( $(date +%s) - T0 ))s jafn0=$J curl=$C rules51820=$R" >> $OUT/timeline.txt
  }
  for i in $(seq 1 6); do probe; ip link show jafn0 >/dev/null 2>&1 && break; sleep 2; done
  sleep 2; probe
  echo "KILL_SHARER t=$(( $(date +%s) - T0 ))s" >> $OUT/timeline.txt
  touch $OUT/.kill
  for i in $(seq 1 14); do sleep 5; probe; done
  echo "RESTART_SHARER t=$(( $(date +%s) - T0 ))s" >> $OUT/timeline.txt
  touch $OUT/.restart
  for i in $(seq 1 12); do sleep 5; probe; done
  wg-quick down jafn0 >/dev/null 2>&1
' &
INNER=$!

while [ ! -e "$OUT/.kill" ] && kill -0 $INNER 2>/dev/null; do sleep 0.5; done
for p in $(ip netns pids jl-sh); do kill $p 2>/dev/null; done
while [ ! -e "$OUT/.restart" ] && kill -0 $INNER 2>/dev/null; do sleep 0.5; done
start_sharer 2
wait $INNER

echo "=== timeline ==="; cat "$OUT/timeline.txt"
echo "=== joiner log ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner.log"
echo "=== sharer 1 (tail) ==="; grep -v "^20" "$OUT/sharer1.log" | tail -6
echo "=== sharer 2 (tail) ==="; grep -v "^20" "$OUT/sharer2.log" | tail -6
echo "=== LAN frames per 5s bucket (src:first-byte) ==="
python3 -I - "$OUT/lan.pcap" <<'PY'
import sys, struct, collections
d=open(sys.argv[1],'rb').read(); off=24; t0=None; b=collections.OrderedDict()
while off+16<=len(d):
    ts=struct.unpack('<I',d[off:off+4])[0]+struct.unpack('<I',d[off+4:off+8])[0]/1e6
    incl=struct.unpack('<I',d[off+8:off+12])[0]; pkt=d[off+16:off+16+incl]; off+=16+incl
    t0=t0 or ts; ihl=(pkt[14]&15)*4; sp,dp=struct.unpack('>HH',pkt[14+ihl:14+ihl+4])
    src=pkt[29]; pl=pkt[14+ihl+8:]; k=int((ts-t0)//5)*5
    tag=('sh' if src==1 else 'jo')+(':%02x'%pl[0] if pl else '')+('/wg' if 9797 in (sp,dp) else '')
    b.setdefault(k,collections.Counter())[tag]+=1
for k,c in b.items(): print('%4ds'%k, dict(c))
PY
echo "=== host after ==="; ip -br addr show wg0; ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner*.log
lab_finish
exit $FAILS
