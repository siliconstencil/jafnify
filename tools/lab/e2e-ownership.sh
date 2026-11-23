#!/bin/bash
# O1/O2 ownership lab (run as root). Isolated namespaces; host wg0/routes
# untouched.
#
#   O1: instance A is connected through jafn0. Instance B starts in the SAME
#       netns with a different config dir and NO pair, and runs for
#       OWNERSHIP_SECONDS. B must make ZERO privileged calls on jafn0: its
#       pkexec log contains no `down`, no `route-*` and no `suppress`. A's
#       tunnel and its internet path must still work after B stops (B's
#       SIGTERM path must not run route-fallback either).
#   O2 (same run): B's status line is the plain
#       "Another Jafnify connection is active", not "Not sharing".
#
# PASS means every line in results.txt is PASS and exit code 0.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
# The incident's second instance was the test build; use it when it exists.
B=${B:-$ROOT/linux/target/release/jafnify-test}
[ -x "$B" ] || B=$JOIN
OWNERSHIP_SECONDS=${OWNERSHIP_SECONDS:-60}
OUT=${LAB_OUT:-$ROOT/tmp/lab}/out-ownership
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"
ST=$OUT/state
rm -rf "$OUT"/*; mkdir -p "$ST/a" "$ST/b" "$ST/bin" "$ST/sh"
lab_init "$OUT"

declare -a NS=(jl-jo jl-sh jl-net)
cleanup() {
  for n in "${NS[@]}"; do for p in $(ip netns pids "$n" 2>/dev/null); do kill "$p" 2>/dev/null; done; done
  sleep 0.5
  for n in "${NS[@]}"; do ip netns del "$n" 2>/dev/null; done
}
trap 'lab_finish; cleanup; [ -z "${KEEP:-}" ] && rm -rf "$ST"' EXIT
cleanup
ip route show default | head -2 > "$OUT/host-before"

for n in "${NS[@]}"; do ip netns add "$n"; ip -n "$n" link set lo up; done
ip -n jl-sh link add jl-br type bridge
ip -n jl-sh addr add 192.168.77.1/24 dev jl-br
ip -n jl-sh link set jl-br up
ip link add jl-a type veth peer name jl-ab
ip link set jl-a netns jl-jo
ip link set jl-ab netns jl-sh
ip -n jl-jo addr add 192.168.77.2/24 dev jl-a
ip -n jl-jo link set jl-a up
ip -n jl-sh link set jl-ab master jl-br
ip -n jl-sh link set jl-ab up
ip -n jl-jo route add default via 192.168.77.1
ip link add jl-c type veth peer name jl-d
ip link set jl-c netns jl-sh
ip link set jl-d netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-c
ip -n jl-sh link set jl-c up
ip -n jl-net addr add 10.200.0.1/24 dev jl-d
ip -n jl-net link set jl-d up
ip -n jl-net addr add 203.0.113.1/24 dev jl-d
ip -n jl-sh route add default via 10.200.0.1
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0
mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 \
  --directory "$OUT/www" > "$OUT/http.log" 2>&1 &

# A wrapper that records every privileged call and then runs the real one.
# The lab runs as root, so no prompt appears.
cat > "$ST/bin/pkexec" <<'PK'
#!/bin/sh
echo "$@" >> "$JAFNIFY_PKEXEC_LOG"
exec /usr/bin/pkexec "$@"
PK
chmod +x "$ST/bin/pkexec"

# The sharer A connects to.
ip netns exec jl-sh "$SHARE" --iface-ip 192.168.77.1 --state-dir "$ST/sh" \
  --code 4321 > "$OUT/sharer.log" 2>&1 &
SHARE_PID=$!
sleep 1

# A: a real joiner with the code.
joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST/a"'
  exec timeout 150 '"$JOIN"' join --accept-terms --code 4321
' > "$OUT/joiner-a.log" 2>&1 &
A_PID=$!

C=""
for _ in $(seq 1 60); do
  C=$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || true)
  [ "$C" = "JAFNIFY-E2E-OK" ] && break
  sleep 1
done
if [ "$C" = "JAFNIFY-E2E-OK" ]; then
  echo "PASS  A connected through jafn0" | tee -a "$OUT/results.txt"
else
  echo "FAIL  A never connected through jafn0" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS + 1))
fi

# B: same netns, a different config dir, no pair. It sees A's jafn0.
: > "$OUT/pkexec-b.log"
joiner_sandbox jl-jo bash -c '
  export HOME=$(mktemp -d -p '"$ST"') XDG_CONFIG_HOME='"$ST/b"'
  export JAFNIFY_PKEXEC_LOG='"$OUT/pkexec-b.log"' PATH='"$ST/bin"':$PATH
  timeout '"$OWNERSHIP_SECONDS"' '"$B"' join --accept-terms --code 1111
' > "$OUT/joiner-b.log" 2>&1 &
B_PID=$!
wait "$B_PID" || true

if grep -Eq '(^| )(down|route[-a-z]*|suppress)($| )' "$OUT/pkexec-b.log"; then
  echo "FAIL  B touched jafn0: $(grep -E '(^| )(down|route[-a-z]*|suppress)($| )' "$OUT/pkexec-b.log" | tr '\n' ';')" \
    | tee -a "$OUT/results.txt"
  FAILS=$((FAILS + 1))
else
  echo "PASS  B made zero down/route/suppress calls on jafn0" | tee -a "$OUT/results.txt"
fi

if ip netns exec jl-jo ip link show jafn0 >/dev/null 2>&1; then
  echo "PASS  A's jafn0 still exists after B stopped" | tee -a "$OUT/results.txt"
else
  echo "FAIL  B removed A's jafn0" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS + 1))
fi
C2=$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || true)
if [ "$C2" = "JAFNIFY-E2E-OK" ]; then
  echo "PASS  A still reachable through jafn0" | tee -a "$OUT/results.txt"
else
  echo "FAIL  A lost its internet path after B stopped (curl=$C2)" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS + 1))
fi

if grep -q "Another Jafnify connection is active" "$OUT/joiner-b.log"; then
  echo "PASS  B shows the plain foreign status" | tee -a "$OUT/results.txt"
else
  echo "FAIL  B did not show the plain foreign status" | tee -a "$OUT/results.txt"
  FAILS=$((FAILS + 1))
fi

kill "$A_PID" 2>/dev/null; wait "$A_PID" 2>/dev/null
ip -n jl-jo link del jafn0 2>/dev/null
for p in $(ip netns pids jl-sh); do
  grep -q sharer-host /proc/"$p"/cmdline 2>/dev/null && kill "$p"
done
for p in $(ip netns pids jl-net); do
  grep -q http.server /proc/"$p"/cmdline 2>/dev/null && kill "$p"
done
sleep 1

{
  echo "=== B pkexec log ==="
  sed -E 's#[A-Za-z0-9+/]{43}=#<KEY>#g' "$OUT/pkexec-b.log"
  echo "=== B status ==="
  grep -E "Another Jafnify connection is active|Not sharing" "$OUT/joiner-b.log" | head -3
  echo "=== A tail ==="
  sed -E 's#[A-Za-z0-9+/]{43}=#<KEY>#g' "$OUT/joiner-a.log" | tail -5
} > "$OUT/summary.txt"
echo "=== summary ==="; cat "$OUT/summary.txt"
echo "=== host after ==="; ip -br addr show wg0; ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner-*.log
lab_finish
exit "$FAILS"
