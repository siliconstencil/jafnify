#!/bin/bash
# Jafnify lab repro (diagnostic only, run as root). Fully isolated:
#   - two network namespaces (jl-solo, jl-up) joined by a veth; nothing is
#     attached to the host network, wg0 / default route are never touched
#   - inside jl-solo a private mount namespace puts tmpfs over /etc/wireguard
#     and HOME, so no real system file is written
# Runs ONE Linux joiner alone (no sharer exists) and records:
#   - which multicast frame types it sends on 239.255.42.99:9799
#     (0x01 = discovery token, 0x02 = handshake/public key)
#   - whether it "connects" with nobody there (self-pairing)
#   - whether wg-quick accepts the config it generates
set -u
. "$(dirname "$0")/lib.sh"
BIN=$(cd "$(dirname "$0")/../.." && pwd)/linux/target/release/jafnify
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
mkdir -p "$OUT"; rm -f "$OUT"/*
lab_init "$OUT"

cleanup() { ip netns del jl-solo 2>/dev/null; ip netns del jl-up 2>/dev/null; }
trap 'lab_finish; cleanup' EXIT
cleanup

ip netns add jl-solo
ip netns add jl-up
ip link add jl-a type veth peer name jl-b
ip link set jl-a netns jl-solo
ip link set jl-b netns jl-up
ip -n jl-solo addr add 192.168.77.2/24 dev jl-a
ip -n jl-up   addr add 192.168.77.1/24 dev jl-b
for ns in jl-solo jl-up; do ip -n $ns link set lo up; done
ip -n jl-solo link set jl-a up
ip -n jl-up   link set jl-b up
ip -n jl-solo route add default via 192.168.77.1

ip netns exec jl-solo tcpdump -i jl-a -U -w "$OUT/mcast.pcap" 'udp port 9799' >/dev/null 2>&1 &
TCPDUMP=$!
sleep 1

joiner_sandbox jl-solo bash -c '
  export HOME=$(mktemp -d) XDG_CONFIG_HOME=
  timeout 25 '"$BIN"' join --accept-terms --code 4321 > '"$OUT"'/jafnify.log 2>&1
  echo "exit=$?" >> '"$OUT"'/jafnify.log
  echo "--- generated conf (keys redacted) ---" >> '"$OUT"'/jafnify.log
  for f in /etc/wireguard/*.conf $HOME/.config/jafnify/*.conf; do
    [ -f "$f" ] && { echo "# $f"; sed -E "s/(=\s*)[A-Za-z0-9+\/]{43}=/\1<REDACTED>/" "$f"; }
  done >> '"$OUT"'/jafnify.log 2>&1
  echo "--- links in jl-solo ---" >> '"$OUT"'/jafnify.log
  ip -br link >> '"$OUT"'/jafnify.log
'
sleep 1; kill $TCPDUMP 2>/dev/null; wait $TCPDUMP 2>/dev/null

echo "=== joiner output ==="; cat "$OUT/jafnify.log"
echo "=== multicast frame types sent (first payload byte) ==="
python3 -I - "$OUT/mcast.pcap" <<'PY'
import sys, struct, collections
d = open(sys.argv[1], 'rb').read()
off = 24; c = collections.Counter()
while off + 16 <= len(d):
    incl = struct.unpack('<I', d[off+8:off+12])[0]
    pkt = d[off+16:off+16+incl]; off += 16 + incl
    ihl = (pkt[14] & 0x0f) * 4
    payload = pkt[14+ihl+8:]
    if payload: c['0x%02x' % payload[0]] += 1
print(dict(c) or "no frames")
PY
lab_assert_no_resolvectl "$OUT" "$OUT"/jafnify.log
lab_finish
exit $FAILS
