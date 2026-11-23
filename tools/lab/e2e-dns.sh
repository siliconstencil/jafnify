#!/bin/bash
# Jafnify DNS lab (run as root). Same isolated 3-namespace layout as e2e-pair.sh.
# Question: once the tunnel is up, does the sharer gateway's built-in DNS
# (10.233.0.1:53, libwgslirpy serve_dns) answer the joiner through the tunnel,
# and does the generated joiner config point at it?
# The sharer resolves via its own namespace: /etc/netns/jl-sh/hosts (created
# here, removed at exit) maps jafnify-lab.test -> 203.0.113.1 (the lab's internet host).
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-dns
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
mkdir -p "$OUT"; rm -rf "$OUT"/*
lab_init "$OUT"
FAILS=0
NS="jl-jo jl-sh jl-net"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5; for n in $NS; do ip netns del $n 2>/dev/null; done
  rm -rf /etc/netns/jl-sh; rmdir /etc/netns 2>/dev/null; true
}
trap 'lab_finish; cleanup' EXIT
cleanup
mkdir -p /etc/netns/jl-sh
printf '127.0.0.1 localhost\n203.0.113.1 jafnify-lab.test\n' > /etc/netns/jl-sh/hosts

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

ip netns exec jl-sh "$SHARE" --code 4321 --iface-ip 192.168.77.1 > "$OUT/sharer.log" 2>&1 &
sleep 1
joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d) XDG_CONFIG_HOME=$HOME/.config
  timeout 40 '"$JOIN"' join --accept-terms --code 4321 > '"$OUT"'/joiner.log 2>&1 &
  for i in $(seq 1 20); do ip link show jafn0 >/dev/null 2>&1 && break; sleep 1; done
  sleep 2
  echo "=== generated conf: DNS lines ==="; grep -ci "^dns" /etc/wireguard/jafn0.conf || true
  echo "=== dig @10.233.0.1 jafnify-lab.test through the tunnel ==="
  dig +time=3 +tries=1 +short @10.233.0.1 jafnify-lab.test A || echo "dig failed"
  echo "=== route to 10.233.0.1 ==="; ip route get 10.233.0.1 | head -1
  echo "=== dig @10.233.0.1 for a name the sharer cannot resolve ==="
  dig +time=3 +tries=1 @10.233.0.1 nonexistent.invalid A | grep -E "status:|timed out" || echo "no answer"
'
echo "=== joiner log ==="; cat "$OUT/joiner.log"
echo "=== sharer log (DNS lines) ==="; grep -iE "dns|state:" "$OUT/sharer.log" | sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" | tail -8
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner*.log
lab_finish
exit $FAILS
