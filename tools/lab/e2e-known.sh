#!/bin/bash
# Jafnify known-peer lab (run as root). Isolated namespaces; host wg0/routes
# untouched. State dirs hold throwaway lab keys only, never printed, deleted
# at the end.
#   run 1: pair with code 4321, generate traffic so both sides persist the pair
#   run 1b: the joiner is killed and comes back without adopting its tunnel;
#           the phone (still the code track) must answer at once, under 10 s
#   run 2: restart both WITHOUT a code -> must reconnect, same WG keys, only 0x03
#   run 3: a stranger namespace with its own (empty) state cannot connect
#   run 4: joiner `forget` -> code-less join refuses ("no paired device")
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-known
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
ST=$OUT/state
mkdir -p "$OUT"; rm -rf "$OUT"/*; mkdir -p "$ST/jo" "$ST/sh" "$ST/x"
lab_init "$OUT"

NS="jl-jo jl-sh jl-net jl-x"
cleanup() {
  for n in $NS; do for p in $(ip netns pids $n 2>/dev/null); do kill $p 2>/dev/null; done; done
  sleep 0.5
  for n in $NS; do ip netns del $n 2>/dev/null; done
}
trap 'lab_finish; cleanup; [ -z "${KEEP:-}" ] && rm -rf "$ST"' EXIT
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

# joiner LABEL "JOIN ARGS" SECONDS [NS] [STATE_DIR]
joiner() {
  joiner_sandbox "${4:-jl-jo}" bash -c '
    OUT='"$OUT"'; L='"$1"'
    export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"${5:-$ST/jo}"'
    unset JAFNIFY_CODE
    timeout '"$3"' '"$JOIN"' join --accept-terms '"$2"' > $OUT/joiner-$L.log 2>&1 &
    JP=$!; T0=$(date +%s); UP=
    for i in $(seq 1 '"$3"'); do
      if ip link show jafn0 >/dev/null 2>&1; then
        C=$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)
        [ -z "$UP" ] && { UP=$(( $(date +%s) - T0 )); echo "$L: jafn0 up after ${UP}s, curl=$C" >> $OUT/summary.txt; }
        if [ "$C" = JAFNIFY-E2E-OK ]; then
          sleep 3; curl -s -m 2 http://203.0.113.1:8080/ >/dev/null
          echo "$L: curl OK; jafn0 pubkey sha=$(wg show jafn0 public-key | sha256sum | cut -c1-12)" >> $OUT/summary.txt
          sleep 2; break
        fi
      fi
      sleep 1
    done
    [ -z "$UP" ] && echo "$L: jafn0 never came up in '"$3"' s" >> $OUT/summary.txt
    kill $JP 2>/dev/null; wait $JP 2>/dev/null; wg-quick down jafn0 >/dev/null 2>&1; true
  '
}
sharer() { ip netns exec jl-sh "$SHARE" --iface-ip 192.168.77.1 --state-dir "$ST/sh" "$@" > "$OUT/sharer-$L.log" 2>&1 & SP=$!; }
stop_sharer() { kill $SP 2>/dev/null; wait $SP 2>/dev/null; for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p; done; sleep 1; }
peers() { for s in jo sh; do f=$(find "$ST/$s" -name state.toml 2>/dev/null | head -1); if [ -n "$f" ]; then echo "  $s: mode=$(stat -c %a "$f") peers=$(grep -c '^\[\[peers\]\]' "$f") has_priv=$(grep -c '^private_key' "$f")"; else echo "  $s: no state file"; fi; done; }

# Poll one joiner log for Connected, the same way e2e-link.sh does: a one-shot
# grep can miss a line the joiner writes a moment later.
wait_connected() {
  local n=$1 log=$2 i=0
  while [ "$i" -lt "$n" ]; do
    if grep -q Connected "$log" 2>/dev/null; then return 0; fi
    i=$((i + 1)); sleep 1
  done
  return 1
}

ip netns exec jl-sh tcpdump -i jl-br -U -w "$OUT/lan.pcap" 'udp port 9799' >/dev/null 2>&1 &
sleep 1

L=run1; sharer --code 4321; sleep 1
joiner run1 "--code 4321" 40

# --- the joiner comes back after a kill, without adopting its tunnel ---------
# The sharer is still the same process (the code track). The joiner's stored
# pair makes it come back on the known track, so the sharer must answer that
# Hello at once: the known connect must not wait for the 35 s no-data edge.
L=run1b; joiner run1b "" 70
KNOWN_S=$(lab_known_connect_seconds "$OUT/joiner-run1b.log")
if [ -n "$KNOWN_S" ] && [ "$KNOWN_S" -lt 10 ]; then
  echo "PASS  first-known-connect-after-killed-joiner: ${KNOWN_S}s (under 10s)" | tee -a "$OUT/results.txt"
else
  echo "FAIL  first-known-connect-after-killed-joiner: ${KNOWN_S:-no known connect}s (want under 10s)" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS+1))
fi
# --- the same again while the phone is on the KNOWN track (the everyday case) --
# After run1b the phone serves the known track; a second kill/come-back of the
# laptop must also be answered at once.
L=run1c; joiner run1c "" 70
KNOWN_C=$(lab_known_connect_seconds "$OUT/joiner-run1c.log")
if [ -n "$KNOWN_C" ] && [ "$KNOWN_C" -lt 10 ]; then
  echo "PASS  known-track-comeback-after-killed-joiner: ${KNOWN_C}s (under 10s)" | tee -a "$OUT/results.txt"
else
  echo "FAIL  known-track-comeback-after-killed-joiner: ${KNOWN_C:-no known connect}s (want under 10s)" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS+1))
fi
stop_sharer
echo "after run1:" >> $OUT/summary.txt; peers >> $OUT/summary.txt
date +%s > $OUT/run2.mark

L=run2; sharer; sleep 1
joiner run2 "" 40
joiner run3 "" 12 jl-x "$ST/x"
joiner run3b "--code 1111" 20 jl-x "$ST/x"

# --- adopt: a second joiner while the first one's tunnel is up ---------------
# The lab runs as root, so the wrapper's real pkexec runs without a prompt; it
# only records which privileged calls happened.
mkdir -p "$ST/bin"; : > "$OUT/pkexec-adopt.log"
cat > "$ST/bin/pkexec" <<'PK'
#!/bin/sh
echo "$@" >> "$JAFNIFY_PKEXEC_LOG"
exec /usr/bin/pkexec "$@"
PK
chmod +x "$ST/bin/pkexec"
export JAFNIFY_PKEXEC_LOG="$OUT/pkexec-adopt.log" PATH="$ST/bin:$PATH"
joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST/jo"' JAFNIFY_PKEXEC_LOG='"$JAFNIFY_PKEXEC_LOG"' PATH='"$ST/bin"':$PATH
  exec timeout 90 '"$JOIN"' join --accept-terms
' > "$OUT/joiner-adopt-first.log" 2>&1 &
FIRST=$!
for i in $(seq 1 40); do
  C=$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || true)
  [ "$C" = JAFNIFY-E2E-OK ] && break
  sleep 1
done
BEFORE=$(wc -l < "$JAFNIFY_PKEXEC_LOG")
joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST/jo"' JAFNIFY_PKEXEC_LOG='"$JAFNIFY_PKEXEC_LOG"' PATH='"$ST/bin"':$PATH
  exec timeout 30 '"$JOIN"' join --accept-terms
' > "$OUT/joiner-adopt-second.log" 2>&1 &
SECOND=$!
# Poll: the second joiner may print Connected a moment after the socket opens.
if wait_connected 30 "$OUT/joiner-adopt-second.log"; then
  CONNECTED_MSG="adopt: second joiner says Connected"
else
  CONNECTED_MSG="adopt: second joiner did NOT say Connected"
fi
kill $SECOND 2>/dev/null; wait $SECOND 2>/dev/null
AFTER=$(wc -l < "$JAFNIFY_PKEXEC_LOG")
C2=$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || true)
{
  echo "adopt: first curl=$C after second curl=$C2 pkexec_before=$BEFORE pkexec_after=$AFTER"
  echo "$CONNECTED_MSG"
} >> "$OUT/summary.txt"
kill $FIRST 2>/dev/null; wait $FIRST 2>/dev/null
ip -n jl-jo link del jafn0 2>/dev/null
stop_sharer

joiner_sandbox jl-jo bash -c '
  export XDG_CONFIG_HOME='"$ST/jo"' HOME='"$ST"'
  "'"$JOIN"'" forget > "'"$OUT"'/forget.log" 2>&1
  echo "forget exit=$?" >> "'"$OUT"'/summary.txt"
'
echo "after forget:" >> $OUT/summary.txt; peers >> $OUT/summary.txt
joiner_sandbox jl-jo bash -c '
  export XDG_CONFIG_HOME='"$ST/jo"' HOME='"$ST"'
  env -u JAFNIFY_CODE timeout 5 "'"$JOIN"'" join > "'"$OUT"'/joiner-run4.log" 2>&1
  echo "run4 code-less join exit=$?" >> "'"$OUT"'/summary.txt"
'

sleep 1; for p in $(ip netns pids jl-sh); do grep -q tcpdump /proc/$p/cmdline 2>/dev/null && kill $p; done; sleep 1
echo "=== summary ==="; cat $OUT/summary.txt
for f in run1 run1b run1c run2 run3 run3b run4; do echo "=== joiner $f ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" $OUT/joiner-$f.log 2>/dev/null | head -10; done
for f in run1 run2; do echo "=== sharer $f ==="; grep -v "^20" $OUT/sharer-$f.log | sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" | tail -8; done
echo "=== forget ==="; cat $OUT/forget.log
echo "=== 9799 frames by phase (src, first byte, payload len): count ==="
python3 -I - "$OUT/lan.pcap" "$(cat $OUT/run2.mark)" <<'PY'
import sys, struct, collections
d=open(sys.argv[1],'rb').read(); t2=float(sys.argv[2]); off=24
c={'run1':collections.Counter(),'run2+':collections.Counter()}
while off+16<=len(d):
    ts=struct.unpack('<I',d[off:off+4])[0]; incl=struct.unpack('<I',d[off+8:off+12])[0]
    pkt=d[off+16:off+16+incl]; off+=16+incl; ihl=(pkt[14]&15)*4; pl=pkt[14+ihl+8:]
    src='.'.join(map(str,pkt[26:30]))
    c['run1' if ts<t2 else 'run2+'][(src, '0x%02x'%pl[0] if pl else '-', len(pl))]+=1
for k,v in c.items(): print(k, dict(sorted(v.items())))
PY
echo "=== host after ==="; ip -br addr show wg0; ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner-*.log
lab_finish
exit $FAILS
