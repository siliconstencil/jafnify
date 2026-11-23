#!/bin/bash
# Jafnify topology lab (run as root). Isolated namespaces. The host wg0 and
# routes are untouched (checked before and after). Models a phone (cellular
# uplink plus hotspot LAN) and a laptop:
#
#   jl-sh = the phone: "cellular" jl-cell owns the default route, "hotspot"
#           jl-hot is the LAN shared with the laptop, jl-p2p is one more
#           non-cellular interface (hotspot is NOT the first one listed)
#   jl-jo = the laptop: host firewall equivalent to ufw defaults (nftables input
#           policy drop, established/related, loopback, nothing else)
#   jl-net = the internet behind the cellular link (HTTP on 203.0.113.1:8080,
#           a public TEST-NET-3 address; the cellular gateway is 10.200.0.1)
#
# Own multicast loopback re-enters the input path on the LAN interface, not on
# lo, so the firewall drops the laptop's own announcements exactly as ufw did.
#
# Scenarios (each prints PASS/FAIL; exit status is the number of failures):
#   B0  an unpinned sharer is refused by design: without a LAN interface it
#       cannot open discovery (no LAN interface selected).
#   B2  sharer given every non-cellular interface, hotspot first: found, tunnel
#       up, HTTP only through the tunnel. Before the fix only the LAST
#       --iface-ip counted (jl-p2p), so this fails.
#   A1  firewalled laptop, no rule: pairs and the tunnel comes up by unicast,
#       and the laptop does NOT say the firewall blocks the search.
#   A2  `jafnify firewall rule` prints a narrow rule; once inserted, the laptop
#       finds the sharer and the tunnel works. Before the fix the command does
#       not exist.
#   C1  probe both ways with the narrow rule in place and UDP 9797 held by
#       another program on both sides (the legacy WgServer stand-in). Discovery
#       is the PAKE exchange: 0x01 announce plus 0x05 Hello / 0x06 Confirm.
#   C2  probe with no rule and the phone's multicast dropped on the hotspot:
#       the laptop probe completes by unicast, reports (unicast), and says the
#       firewall is not needed; no multicast from the phone reached it.
#   P1  full pairing + tunnel while a 9797 stand-in holds UDP 9797 on the phone:
#       the sharer uses another port (logged) and the tunnel still comes up.
#   U1  the phone's multicast to the group is DROPPED on the hotspot (counted),
#       the laptop firewall has no inbound rule at all: pairing and the tunnel
#       must still succeed by unicast (the laptop's gateway is the phone).
#   S7/S8  a real PAKE probe from the cellular side reaches no peer (the
#       ingress filter drops a datagram sent to an unpinned address); the same
#       probe from the laptop LAN completes the key exchange (control).
#   S1-S6  with the tunnel up the laptop cannot reach the phone's own addresses or
#       another host on the phone's network, still reaches the internet, and the
#       inbound NEW guard on jafn0 is present while connected and gone after.
#
# Run the "red" pass against binaries built BEFORE the fix, the "green" pass
# after. Build both binaries in release mode first.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# JOIN and SHARE can point at binaries built from an older commit (the "red"
# pass): JOIN=/path/to/old/jafnify SHARE=/path/to/old/jafnify-sharer-host
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-hotspot
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
ST=$OUT/state
CODE=4321
LAN=192.168.77
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
# hotspot LAN: phone jl-hot <-> laptop jl-lan
ip link add jl-hot type veth peer name jl-lan
ip link set jl-hot netns jl-sh; ip link set jl-lan netns jl-jo
ip -n jl-sh addr add $LAN.1/24 dev jl-hot; ip -n jl-sh link set jl-hot up
ip -n jl-jo addr add $LAN.2/24 dev jl-lan; ip -n jl-jo link set jl-lan up
ip -n jl-jo route add default via $LAN.1
# cellular: owns the phone's default route
ip link add jl-cell type veth peer name jl-cellb
ip link set jl-cell netns jl-sh; ip link set jl-cellb netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-cell; ip -n jl-sh link set jl-cell up
ip -n jl-net addr add 10.200.0.1/24 dev jl-cellb; ip -n jl-net link set jl-cellb up
ip -n jl-net addr add 203.0.113.1/24 dev jl-cellb # the "internet" host (TEST-NET-3, public range)
ip -n jl-sh route add default via 10.200.0.1
# one more non-cellular interface (nothing useful on the other end)
ip link add jl-p2p type veth peer name jl-p2pb
ip link set jl-p2p netns jl-sh; ip link set jl-p2pb netns jl-net
ip -n jl-sh addr add 10.77.0.1/24 dev jl-p2p; ip -n jl-sh link set jl-p2p up
ip -n jl-net addr add 10.77.0.2/24 dev jl-p2pb; ip -n jl-net link set jl-p2pb up
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0

mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
ip netns exec jl-sh tcpdump -i jl-hot -U -w "$OUT/hot.pcap" 'udp port 9799' >/dev/null 2>&1 &
ip netns exec jl-sh tcpdump -i jl-cell -U -w "$OUT/cell.pcap" 'udp port 9799' >/dev/null 2>&1 &
sleep 1

# ---- helpers ---------------------------------------------------------------

# The laptop's host firewall: ufw defaults, nothing else. Also resets it.
fw_on() {
  ip netns exec jl-jo nft delete table inet jlfw 2>/dev/null
  ip netns exec jl-jo nft -f - <<'NFT'
table inet jlfw {
  chain input {
    type filter hook input priority 0; policy drop;
    ct state established,related accept
    iif "lo" accept
  }
}
NFT
}

# joiner LABEL "JOIN ARGS" SECONDS: the real laptop app in jl-jo. Writes
# joiner-LABEL.log and LABEL.res ("up=SECONDS curl=ok|fail|none").
joiner() {
  joiner_sandbox jl-jo bash -c '
    OUT='"$OUT"'; L='"$1"'
    export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST"'/jo-'"$1"'
    mkdir -p "$XDG_CONFIG_HOME"
    unset JAFNIFY_CODE
    timeout '"$3"' '"$JOIN"' join --accept-terms '"$2"' > $OUT/joiner-$L.log 2>&1 &
    JP=$!; T0=$(date +%s); UP=; C=
    for i in $(seq 1 '"$3"'); do
      if ip link show jafn0 >/dev/null 2>&1; then
        [ -z "$UP" ] && UP=$(( $(date +%s) - T0 ))
        C=$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)
        if [ "$C" = JAFNIFY-E2E-OK ]; then echo "up=$UP curl=ok" > $OUT/$L.res; break; fi
      fi
      sleep 1
    done
    [ -f $OUT/$L.res ] || echo "up=${UP:-none} curl=${C:-none}" > $OUT/$L.res
    kill $JP 2>/dev/null; wait $JP 2>/dev/null; wg-quick down jafn0 >/dev/null 2>&1; true
  '
}
sharer() { L=$1; shift; ip netns exec jl-sh "$SHARE" "$@" > "$OUT/sharer-$L.log" 2>&1 & SP=$!; }
stop_sharer() {
  kill $SP 2>/dev/null; wait $SP 2>/dev/null
  for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p; done
  sleep 1
}
# Something else holds the WireGuard port (the legacy WgServer): probes must not care.
hold_9797() {
  ip netns exec $1 python3 -I -c 'import socket,time; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.bind(("0.0.0.0",9797)); time.sleep(600)' >/dev/null 2>&1 &
}
# probes LABEL SECONDS: phone-side probe in jl-sh and laptop probe in jl-jo, same code, at once.
probes() {
  (
    ip netns exec jl-sh "$SHARE" --probe --code $CODE --seconds $2 --iface-ip $LAN.1 --iface-ip 10.77.0.1 > "$OUT/probe-sh-$1.log" 2>&1
    echo $? > "$OUT/probe-sh-$1.rc"
  ) &
  PSH=$!
  joiner_sandbox jl-jo bash -c '
    export XDG_CONFIG_HOME='"$ST/probe-jo"' HOME='"$ST"'
    "'"$JOIN"'" probe --code '"$CODE"' --seconds '"$2"' > "'"$OUT"'/probe-jo-'"$1"'.log" 2>&1
  '
  echo $? > "$OUT/probe-jo-$1.rc"
  wait $PSH
}

FAILS=0
res() { # NAME cmd...
  if "${@:2}"; then echo "PASS  $1" | tee -a "$OUT/results.txt"
  else echo "FAIL  $1" | tee -a "$OUT/results.txt"; FAILS=$((FAILS+1)); fi
}
found()   { grep -q "Pairing" "$OUT/joiner-$1.log"; }
unfound() { ! found "$1"; }
tunnel()  { grep -q "curl=ok" "$OUT/$1.res" 2>/dev/null; }
says()    { grep -qi "$2" "$OUT/joiner-$1.log"; }
rc_is()   { [ "$(cat "$OUT/$1.rc" 2>/dev/null)" = "$2" ]; }
rc_not()  { [ "$(cat "$OUT/$1.rc" 2>/dev/null || echo x)" != "$2" ]; }
has()     { grep -q "$2" "$OUT/$1.log"; }
lacks()   { ! grep -qi "$2" "$OUT/joiner-$1.log"; }
# The phone's multicast to the group never leaves the hotspot LAN (counted).
drop_phone_mcast() {
  ip netns exec jl-sh nft delete table inet jlph 2>/dev/null
  ip netns exec jl-sh nft -f - <<'NFT'
table inet jlph {
  chain output {
    type filter hook output priority 0; policy accept;
    oifname "jl-hot" ip daddr 239.255.42.99 counter drop
  }
}
NFT
}
narrow()  { # the printed nft rule is scoped to the LAN interface and the discovery port, and nothing else.
  # No group match since 0a47b56: the unicast copies and PAKE confirms need the port, not the group.
  [ "$NFT_RULE" = "iifname \"jl-lan\" udp dport 9799 accept" ]
}

# ---- B0: an unpinned sharer is refused by design ----------------------------
# The sharer must be told which interface to search on. With no --iface-ip the
# product fails with "no LAN interface selected" (jafnify-core/src/discovery.rs:
# 60-68, via PakeLink::open) instead of following the cellular default route.
sharer b0 --code $CODE
sleep 2
joiner b0 "--code $CODE" 20
stop_sharer
res "B0 an unpinned sharer is refused (no LAN interface selected)" has sharer-b0 "no LAN interface selected"
res "B0 and the laptop does not pair with it" unfound b0

# ---- B2: sharer on every non-cellular interface, hotspot listed first -------
sharer b2 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
joiner b2 "--code $CODE" 40
stop_sharer
res "B2 fix: sharer on all non-cellular interfaces is found, tunnel works" tunnel b2

# ---- A1: the laptop's firewall -----------------------------------------------
fw_on
sharer a1 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
joiner a1 "--code $CODE" 40
stop_sharer
res "A1 firewalled laptop (no rule) pairs by unicast" found a1
res "A1 and the tunnel is up with no inbound rule" tunnel a1
res "A1 and does not say the firewall blocks the search" lacks a1 "blocking the search"

# ---- A2: the narrow rule ------------------------------------------------------
NFT_RULE=$(joiner_sandbox jl-jo bash -c '
  export XDG_CONFIG_HOME='"$ST/rule"' HOME='"$ST"'
  "'"$JOIN"'" firewall rule 2>/dev/null
' | sed -n 's/^nft: //p')
echo "rule: $NFT_RULE" > "$OUT/rule.txt"
res "A2 'jafnify firewall rule' prints a rule scoped to the LAN interface and UDP 9799" narrow
[ -n "$NFT_RULE" ] && ip netns exec jl-jo nft insert rule inet jlfw input $NFT_RULE
sharer a2 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
joiner a2 "--code $CODE" 40
stop_sharer
res "A2 with that rule in place the laptop finds the sharer and the tunnel works" tunnel a2

# ---- C1: probe both ways, rule in place, port 9797 held elsewhere -----------
hold_9797 jl-sh; hold_9797 jl-jo
sleep 1
probes c1 20
res "C1 phone-side probe completed the key exchange" rc_is probe-sh-c1 0
res "C1 laptop probe completed the key exchange" rc_is probe-jo-c1 0
res "C1 laptop probe heard the phone (0x01 and PAKE Confirm 0x06)" has probe-jo-c1 "seen 0x06 from $LAN.1"
res "C1 phone-side probe heard the laptop (0x01 and PAKE Confirm 0x06)" has probe-sh-c1 "seen 0x06 from $LAN.2"
res "C1 laptop probe: firewall open" has probe-jo-c1 "firewall: open"

# ---- P1: full pairing + tunnel while a 9797 stand-in holds the port on the phone
# The C1 stand-in still holds UDP 9797 on jl-sh. The sharer must use another port,
# announce it under the MAC, and the laptop must reach its gateway through the tunnel.
sharer p1 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
joiner p1 "--code $CODE" 40
stop_sharer
res "P1 the sharer used another port while UDP 9797 was held" \
  sh -c "grep -q 'gateway uses UDP' '$OUT/sharer-p1.log'"
res "P1 pairing and tunnel up while UDP 9797 is held on the phone" tunnel p1

# ---- C2: probe with no rule and the phone's multicast dropped ----------------
drop_phone_mcast
fw_on
probes c2 20
res "C2 laptop probe completes the key exchange with no rule" rc_is probe-jo-c2 0
res "C2 laptop heard the phone by unicast" has probe-jo-c2 "discovery: ok (peer $LAN.1, unicast)"
res "C2 no multicast from the phone reached the laptop" sh -c "! grep -q 'from $LAN.1 .*(multicast)' '$OUT/probe-jo-c2.log'"
res "C2 probe says the firewall is not needed" has probe-jo-c2 "firewall: not needed"
res "C2 phone-side probe still hears the laptop" has probe-sh-c2 "seen 0x01 from $LAN.2"

# ---- U1: phone multicast dropped on the hotspot, laptop default-deny, no rule --
# The C1/C2 stand-in still holds UDP 9797; the sharer needs it.
for n in jl-sh jl-jo; do for p in $(ip netns pids $n); do grep -q '9797' /proc/$p/cmdline 2>/dev/null && kill $p; done; done; sleep 1
fw_on
sharer u1 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
joiner u1 "--code $CODE" 40
stop_sharer
res "U1 phone multicast really dropped on the hotspot (counter > 0)" \
  sh -c "ip netns exec jl-sh nft list table inet jlph | grep -Eq 'packets [1-9]'"
res "U1 pairing by unicast through the default-deny laptop firewall" found u1
res "U1 tunnel up with no inbound firewall rule" tunnel u1

# ---- S7/S8: the cellular side is not a discovery path (B1) ------------------
# A real PAKE probe (0x05 Hello / 0x06 Confirm, jafnify-core/src/probe.rs:93-108)
# runs from the cellular side. The sharer is pinned to the hotspot interface, so
# its egress list is [$LAN.1]; `admits` (jafnify-core/src/wire.rs:92-107) drops
# any datagram whose destination is not the group or a pinned address. Add a
# temporary default route in jl-net so the probe's unicast target is the phone's
# cellular address (10.200.0.2); the probe also announces on the cellular link,
# which the sharer never joined.
sharer s7 --code $CODE --iface-ip $LAN.1
sleep 2
ip -n jl-net route add default via 10.200.0.2 2>/dev/null || true
joiner_sandbox jl-net bash -c '
  export XDG_CONFIG_HOME='"$ST"'/s7 HOME='"$ST"'
  "'"$JOIN"'" probe --code '"$CODE"' --seconds 12 > "'"$OUT"'/probe-cell-s7.log" 2>&1
'
ip -n jl-net route del default 2>/dev/null || true
res "S7 a PAKE probe from the cellular side is ignored" \
  sh -c "! grep -Eq 'Peer found|Connecting' '$OUT/sharer-s7.log'"
res "S7 the cellular probe never completed the key exchange" \
  sh -c "! grep -q 'key exchange: ok' '$OUT/probe-cell-s7.log'"
# The same probe on the hotspot LAN is the control: it completes the exchange
# (probe.rs:200-207) and the sharer leaves Searching.
joiner_sandbox jl-jo bash -c '
  export XDG_CONFIG_HOME='"$ST"'/s8 HOME='"$ST"'
  "'"$JOIN"'" probe --code '"$CODE"' --seconds 12 > "'"$OUT"'/probe-lan-s8.log" 2>&1
'
res "S8 the same probe from the laptop LAN is taken (control)" \
  sh -c "grep -Eq 'Peer found|Connecting' '$OUT/sharer-s7.log'"
res "S8 the LAN probe completed the key exchange (control)" \
  has probe-lan-s8 "key exchange: ok"
stop_sharer

# ---- S1-S6: what the tunnel lets the laptop reach, and the inbound guard (B2-B4) --
# Laptop-side checks run while the tunnel is up, with the default-deny laptop
# firewall still on. Listeners sit on the phone (its own addresses) and on another
# network host (10.77.0.2). Results go to $OUT/s1.checks, one line each.
# tunnel_checks LABEL SECONDS
tunnel_checks() {
  joiner_sandbox jl-jo bash -c '
    OUT='"$OUT"'; L='"$1"'
    export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST"'/jo-'"$1"'
    mkdir -p "$XDG_CONFIG_HOME"
    timeout '"$2"' '"$JOIN"' join --accept-terms --code '"$CODE"' > $OUT/joiner-$L.log 2>&1 &
    JP=$!
    for i in $(seq 1 '"$2"'); do
      if ip link show jafn0 >/dev/null 2>&1 && [ "$(curl -s -m 2 http://203.0.113.1:8080/)" = JAFNIFY-E2E-OK ]; then break; fi
      sleep 1
    done
    {
      curl -s -m 3 http://10.77.0.1:7777/ >/dev/null && echo "phone-own-hotspot-addr=reached" || echo "phone-own-hotspot-addr=blocked"
      curl -s -m 3 http://10.200.0.2:7777/ >/dev/null && echo "phone-own-cellular-addr=reached" || echo "phone-own-cellular-addr=blocked"
      curl -s -m 3 http://10.77.0.2:7777/ >/dev/null && echo "other-network-host=reached" || echo "other-network-host=blocked"
      curl -s -m 3 http://203.0.113.1:8080/ | grep -q JAFNIFY-E2E-OK && echo "internet=reached" || echo "internet=blocked"
      iptables -S INPUT | grep -q -- "-i jafn0 .*--ctstate NEW -j DROP" && echo "inbound-guard=present" || echo "inbound-guard=absent"
      ip -6 route show | grep -q "blackhole default" && echo "ipv6-blackhole=present" || echo "ipv6-blackhole=absent"
    } > $OUT/$L.checks
    kill $JP 2>/dev/null; wait $JP 2>/dev/null; wg-quick down jafn0 >/dev/null 2>&1
    iptables -S INPUT | grep -q -- "-i jafn0" && echo "inbound-guard-after-down=left-behind" || echo "inbound-guard-after-down=removed" >> $OUT/$L.checks
    ip -6 route show | grep -q "blackhole default" && echo "ipv6-blackhole-after-down=left-behind" || echo "ipv6-blackhole-after-down=removed" >> $OUT/$L.checks
  '
}
fw_on
sharer s1 --code $CODE --iface-ip $LAN.1 --iface-ip 10.77.0.1
sleep 1
ip netns exec jl-sh python3 -I -m http.server 7777 --bind 0.0.0.0 --directory "$OUT/www" > "$OUT/local-sh.log" 2>&1 &
ip netns exec jl-net python3 -I -m http.server 7777 --bind 10.77.0.2 --directory "$OUT/www" > "$OUT/local-net.log" 2>&1 &
sleep 1
tunnel_checks s1 40
stop_sharer
res "S1 the phone's own hotspot address is unreachable through the tunnel" \
  sh -c "grep -qx 'phone-own-hotspot-addr=blocked' '$OUT/s1.checks'"
res "S2 the phone's own cellular address is unreachable through the tunnel" \
  sh -c "grep -qx 'phone-own-cellular-addr=blocked' '$OUT/s1.checks'"
res "S3 another host on the phone's network is unreachable through the tunnel" \
  sh -c "grep -qx 'other-network-host=blocked' '$OUT/s1.checks'"
res "S4 the laptop still reaches the internet through the tunnel" \
  sh -c "grep -qx 'internet=reached' '$OUT/s1.checks'"
res "S5 the inbound guard is on jafn0 while connected" \
  sh -c "grep -qx 'inbound-guard=present' '$OUT/s1.checks'"
res "S6 the inbound guard is removed with the tunnel" \
  sh -c "grep -qx 'inbound-guard-after-down=removed' '$OUT/s1.checks'"
res "S9 IPv6 is blackholed while connected (no IPv6 leaves around the tunnel)" \
  sh -c "grep -qx 'ipv6-blackhole=present' '$OUT/s1.checks'"
res "S10 the IPv6 blackhole is removed with the tunnel" \
  sh -c "grep -qx 'ipv6-blackhole-after-down=removed' '$OUT/s1.checks'"

# ---- report ---------------------------------------------------------------------
sleep 1; for n in jl-sh; do for p in $(ip netns pids $n); do grep -q tcpdump /proc/$p/cmdline 2>/dev/null && kill $p; done; done; sleep 1
echo "=== announcements that left the phone: hotspot vs cellular (b0 + b2 + a1 + a2 + probes) ==="
echo "hotspot: $(tcpdump -nr "$OUT/hot.pcap" 2>/dev/null | wc -l) packets, cellular: $(tcpdump -nr "$OUT/cell.pcap" 2>/dev/null | wc -l) packets"
cat "$OUT/s1.checks" 2>/dev/null | sed 's/^/checks: /'
for f in b0 b2 a1 a2 p1 u1 s7; do echo "=== joiner $f ($(cat "$OUT/$f.res" 2>/dev/null)) ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner-$f.log" 2>/dev/null | head -12; done
for f in probe-jo-c1 probe-sh-c1 probe-jo-c2 probe-sh-c2; do echo "=== $f ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/$f.log" 2>/dev/null | head -20; done
echo "=== host after ==="; ip -br addr show wg0 > "$OUT/host-wg0-after" 2>&1; diff "$OUT/host-wg0-before" "$OUT/host-wg0-after" && echo "wg0 unchanged"
ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner-*.log
lab_finish
echo "=== $FAILS failed ==="
exit $FAILS
