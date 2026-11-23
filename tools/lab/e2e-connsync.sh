#!/bin/bash
# Jafnify connected-time lab (run as root).
#
# One event both sides count from: the phone's own first authenticated
# exchange. The phone (the sharer) is the clock authority and sends its
# connected milliseconds in the summary reply; the laptop adopts that value.
# Both sides also leave "Connected" for a neutral display while no
# authenticated peer frame arrived, without a drop.
#
# What it proves, per scenario:
#   C1  before the disruption both sides report a connected clock
#   C2  during a long-enough gap the laptop does not say "Connected"
#   C3  after the return both sides report Connected again
#   C4  the two connected values are within 1 s
#   C5  the two Connected moments are within 1 s
#   C6  a quick app update adds no drop; a 40 s link loss adds one
#   C7  a ten-second app install is neutral, with the new app's notice and
#       without it (an old app); the notice writes one "updated" line and
#       never a lost-connection notice
#   C8  a crash reported by the new app still counts and still notifies
#
# Layout is e2e-away-return.sh's: jl-jo is the laptop, jl-sh is the phone (the
# hotspot), jl-net is the internet at 203.0.113.1:8080. The laptop runs the
# tray daemon so the summary (and so the phone clock) is polled. The phone's
# sharer serves a fake summary line because the host binary has no Android
# sampler.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
LAB_HELPER=${LAB_HELPER:-$ROOT/linux/target/release/jafnify-helper}
export LAB_HELPER
LAB_ROUTE=${LAB_ROUTE:-$ROOT/linux/target/release/jafnify-route}
export LAB_ROUTE
OUT=${LAB_OUT:-$ROOT/tmp/lab}/out-connsync
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"
ST=$OUT/state
CODE=4321
LAN=192.168.77
QUICK_GAP=${QUICK_GAP:-3}
UPDATE_GAP=${UPDATE_GAP:-10}
STALL_GAP=${STALL_GAP:-40}
export OUT ST JOIN SHARE CODE LAN
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
ip link add jl-hot type veth peer name jl-lan
ip link set jl-hot netns jl-sh; ip link set jl-lan netns jl-jo
ip -n jl-sh addr add $LAN.1/24 dev jl-hot; ip -n jl-sh link set jl-hot up
ip -n jl-jo addr add $LAN.2/24 dev jl-lan; ip -n jl-jo link set jl-lan up
ip -n jl-jo route add default via $LAN.1
ip netns exec jl-jo sysctl -qw net.ipv4.ping_group_range="0 2147483647"
ip link add jl-cell type veth peer name jl-cellb
ip link set jl-cell netns jl-sh; ip link set jl-cellb netns jl-net
ip -n jl-sh addr add 10.200.0.2/24 dev jl-cell; ip -n jl-sh link set jl-cell up
ip -n jl-net addr add 10.200.0.1/24 dev jl-cellb; ip -n jl-net link set jl-cellb up
ip -n jl-net addr add 203.0.113.1/24 dev jl-cellb
ip -n jl-sh route add default via 10.200.0.1
ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0
mkdir -p "$OUT/www"; echo "JAFNIFY-E2E-OK" > "$OUT/www/index.html"
ip netns exec jl-net python3 -I -m http.server 8080 --bind 203.0.113.1 --directory "$OUT/www" > "$OUT/http.log" 2>&1 &
sleep 1

FAILS=0
res() { # NAME cmd...
  if "${@:2}"; then echo "PASS  $1" | tee -a "$OUT/results.txt"
  else echo "FAIL  $1" | tee -a "$OUT/results.txt"; FAILS=$((FAILS+1)); fi
}
internet_ok() { [ "$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; }
wait_for() { local end=$(( $(date +%s) + $2 )); while [ "$(date +%s)" -lt "$end" ]; do $1 && return 0; sleep 0.5; done; $1; }
secs() { awk -F: '{ if (NF==3) print $1*3600+$2*60+$3; else print $1*60+$2 }'; }

DROPS="$ST/jo-cs/jafnify/drops.tsv"
drop_count() { local n; n=$(grep -c " drop at=" "$DROPS" 2>/dev/null); echo "${n:-0}"; }
updated_count() { local n; n=$(grep -c " updated at=" "$DROPS" 2>/dev/null); echo "${n:-0}"; }
CONNLOG="$ST/state-cs/jafnify/connection.log"
lost_notices() {
  local n
  n=$(grep -c "notified: Jafnify lost the connection" "$CONNLOG" 2>/dev/null)
  echo "${n:-0}"
}

# ---- the phone ---------------------------------------------------------------------
# sharer_start LABEL [known]: the sharer's state dir is kept, so a restart in
# known mode resumes the stored pair. The fake summary line is what the host
# binary serves; the responder adds the session clock to it.
# sharer_start LABEL [known] [exit WORD]: the exit word is what the new app
# reports for the process the update replaced; an old app passes nothing.
sharer_start() {
  local l=$1 args=() exit_args=()
  mkdir -p "$ST/sh-$l"
  if [ "${2:-}" = known ]; then args=(); else args=(--code $CODE); fi
  if [ -n "${3:-}" ]; then exit_args=(--stats-fake-exit "$3"); fi
  ip netns exec jl-sh "$SHARE" "${args[@]}" "${exit_args[@]}" --state-dir "$ST/sh-$l" \
    --iface-ip $LAN.1 --iface-ip 10.77.0.1 \
    --stats-fake-line "JFS1 1 summary v=1 net=wifi sig=3" \
    > "$OUT/sharer-$l${2:+-$2}${3:+-$3}.log" 2>&1 &
  PHONE_LOG="$OUT/sharer-$l${2:+-$2}${3:+-$3}.log"
}
sharer_kill() {
  for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p; done
  sleep 1
}
sharer_paired() {
  local l=$1 end=$(( $(date +%s) + 60 ))
  while [ "$(date +%s)" -lt "$end" ]; do
    if grep -q "sharer: state: Connected" "$OUT/sharer-$l.log" 2>/dev/null \
       && [ -s "$ST/sh-$l/state.toml" ]; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}
# phone_clock: "at_ms connected_ms" from the newest clock line; fails when the
# phone has no connected clock (0 ms).
phone_clock() {
  local last at ms
  last=$(grep "sharer: clock:" "$PHONE_LOG" 2>/dev/null | tail -1)
  [ -n "$last" ] || return 1
  at=$(printf '%s\n' "$last" | sed -n 's/.*at_ms=\([0-9]*\).*/\1/p')
  ms=$(printf '%s\n' "$last" | sed -n 's/.*connected_ms=\([0-9]*\).*/\1/p')
  [ -n "$at" ] && [ -n "$ms" ] && [ "$ms" -gt 0 ] || return 1
  printf '%s %s\n' "$at" "$ms"
}
# phone_neutral: the newest clock line says the native display is neutral.
phone_neutral() {
  grep "sharer: clock:" "$PHONE_LOG" 2>/dev/null | tail -1 | grep -q "neutral=1"
}
tray_up_cs() { [ -f "$OUT/tray-up-cs" ]; }

# ---- the laptop --------------------------------------------------------------------
laptop_stat() {
  joiner_sandbox jl-jo bash -c 'XDG_RUNTIME_DIR="$1" "$2" status' _ "$ST/run-cs" "$JOIN" 2>/dev/null
}
laptop_status_word() { laptop_stat | sed -n '2p'; }
laptop_connected() { local w; w=$(laptop_status_word); [ "$w" = "Connected" ]; }
laptop_secs() { laptop_stat | sed -n 's/^Connected for: //p' | secs; }
# The neutral check: no line may say exactly "Connected".
laptop_neutral() { local w; w=$(laptop_status_word); [ -n "$w" ] && [ "$w" != "Connected" ]; }

# joiner_tray_loop LABEL: pairs headless, then runs the tray daemon. The test
# touches restart-LABEL to make it kill the daemon (SIGKILL: jafn0 stays, like
# a crash and restart) and start a new one. kill-tray-LABEL ends the loop.
joiner_tray_loop() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    export XDG_RUNTIME_DIR="$ST/run-$1" XDG_STATE_HOME="$ST/state-$1"
    mkdir -p "$XDG_CONFIG_HOME/jafnify" "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME"
    chmod 0700 "$XDG_RUNTIME_DIR"
    export PATH="$2:$PATH"
    printf "drop_history = true\nphone_stats = true\nnotify_connection_change = true\n" \
      > "$XDG_CONFIG_HOME/jafnify/flags.toml"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-pair.log" 2>&1 &
    JP=$!
    n=0
    while [ $n -lt 120 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then break; fi
      sleep 0.5; n=$((n+1))
    done
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    "$JOIN" tray >> "$OUT/joiner-$1-tray.log" 2>&1 &
    TP=$!
    n=0
    while [ $n -lt 240 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then
        : > "$OUT/tray-up-$1"
        break
      fi
      sleep 0.5; n=$((n+1))
    done
    while [ ! -f "$OUT/kill-tray-$1" ]; do
      if [ -f "$OUT/restart-$1" ]; then
        rm -f "$OUT/restart-$1"
        kill -9 $TP 2>/dev/null || true
        wait $TP 2>/dev/null || true
        rm -f "$XDG_RUNTIME_DIR/control.sock"
        "$JOIN" tray >> "$OUT/joiner-$1-tray.log" 2>&1 &
        TP=$!
      fi
      sleep 0.2
    done
    kill $TP 2>/dev/null || true
    wait $TP 2>/dev/null || true
  ' _ "$1" "$bin" &
}

# wait_both_connected: the laptop says Connected with a clock and the phone
# reports a positive clock.
both_connected() {
  local l p
  l=$(laptop_secs); p=$(phone_clock) || return 1
  [ -n "$l" ] && [ -n "$p" ]
}


# link_down_gap SECONDS: drop the laptop's hotspot link, watch the laptop go
# neutral (up to 4 s), then keep it down for the rest of SECONDS and restore
# the link and its default route. Returns 0 when the neutral display was seen.
link_down_gap() {
  local secs=$1 t0 spent seen=0
  ip netns exec jl-jo ip link set jl-lan down
  t0=$(date +%s)
  while [ $(( $(date +%s) - t0 )) -lt 4 ]; do
    if laptop_neutral; then seen=1; break; fi
    sleep 0.5
  done
  spent=$(( $(date +%s) - t0 ))
  [ "$spent" -lt "$secs" ] && sleep $(( secs - spent ))
  ip netns exec jl-jo ip link set jl-lan up
  ip -n jl-jo route replace default via $LAN.1
  test "$seen" -eq 1
}

# laptop_moment: read the laptop clock at its whole-second boundary, so the
# moment is exact to one status-call latency instead of a floored second
# subtracted from an arbitrary later instant. Prints "seconds moment_ms".
laptop_moment() {
  local s0 s1 T end
  s0=$(laptop_secs) || return 1
  [ -n "$s0" ] || return 1
  end=$(( $(date +%s%N) / 1000000 + 2000 ))
  while :; do
    s1=$(laptop_secs) || return 1
    if [ -n "$s1" ] && [ "$s1" != "$s0" ]; then
      T=$(date +%s%N)
      T=$(( T / 1000000 ))
      printf '%s %s\n' "$s1" "$(( T - s1 * 1000 ))"
      return 0
    fi
    [ "$(date +%s%N)" -lt "$(( end * 1000000 ))" ] || return 1
    sleep 0.02
  done
}

# check_sync NAME: wait for the two clocks to converge (the summary poll is
# every 5 s), then compare the values and the moments they imply.
check_sync() {
  local name=$1 lv lmoment pc pat pms pmoment dval dmom end
  end=$(( $(date +%s) + 20 ))
  dval=999999
  while [ "$(date +%s)" -lt "$end" ]; do
    if both_connected; then
      lv=$(laptop_secs); pc=$(phone_clock)
      if [ -n "$lv" ] && [ -n "$pc" ]; then
        pms=${pc##* }
        dval=$(( lv - pms / 1000 )); [ "$dval" -lt 0 ] && dval=$(( -dval ))
        [ "$dval" -le 1 ] && break
      fi
    fi
    sleep 0.5
  done
  if ! both_connected; then
    res "$name both sides report a connected clock" false
    return
  fi
  read -r lv lmoment <<< "$(laptop_moment)"
  if [ -z "${lv:-}" ] || [ -z "${lmoment:-}" ]; then
    res "$name both sides report a connected clock" false
    return
  fi
  pc=$(phone_clock) || { res "$name both sides report a connected clock" false; return; }
  pat=${pc%% *}; pms=${pc##* }
  pmoment=$(( pat - pms ))
  dval=$(( lv - pms / 1000 )); [ "$dval" -lt 0 ] && dval=$(( -dval ))
  dmom=$(( lmoment - pmoment )); [ "$dmom" -lt 0 ] && dmom=$(( -dmom ))
  res "$name connected values within 1 s (laptop ${lv}s, phone $(( pms / 1000 ))s)" \
    test "$dval" -le 1
  res "$name Connected moments within 1 s (delta ${dmom} ms)" test "$dmom" -le 1000
}

# ---- setup: one pair, then the tray keeps the pair --------------------------------
sharer_start cs; sleep 1
joiner_tray_loop cs; BTRAY=$!
wait_for tray_up_cs 240
res "C1 before the first disruption: the laptop is connected" internet_ok
check_sync "C1b first connect"
sharer_paired cs || echo "warning: the pair was not saved before the first restart"

# ---- S1: phone app update, quick (3 s): the laptop keeps its old session but
# must count from the phone's new one. This is the "14 s ahead" case.
sharer_kill
sleep "$QUICK_GAP"
sharer_start cs known
check_sync "S1 app update (quick)"
res "S1b app update (quick) adds no drop" \
  test "$(drop_count)" -eq 0

# ---- S2a: the real update gap (10 s), the new app reports "updated" ---------------
# A real install gap: the installer kept the phone away for
# ten seconds, then the new process reported the update in its first summary.
# The outage must stay neutral, be written as one "updated" line and never
# raise a lost-connection notice.
LOST_BEFORE=$(lost_notices)
UPDATED_BEFORE=$(updated_count)
sharer_kill
sleep "$UPDATE_GAP"
sharer_start cs known updated
check_sync "S2a app update (notice)"
END=$(( $(date +%s) + 12 ))
while [ "$(date +%s)" -lt "$END" ] && [ "$(updated_count)" -eq "$UPDATED_BEFORE" ]; do
  sleep 0.5
done
res "S2a1 app update (notice) adds no drop" test "$(drop_count)" -eq 0
res "S2a2 app update (notice) writes one updated line" \
  test "$(updated_count)" -eq "$(( UPDATED_BEFORE + 1 ))"
res "S2a3 app update (notice) sends no lost-connection notice" \
  test "$(lost_notices)" -eq "$LOST_BEFORE"

# ---- S2b: an old app that sends no notice -----------------------------------------
# The same ten-second install gap with a phone that cannot tell why its
# previous process ended: still neutral, still no notice.
LOST_BEFORE=$(lost_notices)
updated_before=$(updated_count)
sharer_kill
sleep "$UPDATE_GAP"
sharer_start cs known
check_sync "S2b app update (no notice)"
res "S2b1 old app update adds no drop" test "$(drop_count)" -eq 0
res "S2b2 old app update writes no updated line" \
  test "$(updated_count)" -eq "$updated_before"
res "S2b3 old app update sends no lost-connection notice" \
  test "$(lost_notices)" -eq "$LOST_BEFORE"

# ---- S2c: a crash, not an update --------------------------------------------------
# The new app says its previous process crashed: the outage is a real drop,
# counted and announced, with the phone's own hint.
DROPS_BEFORE=$(drop_count)
LOST_BEFORE=$(lost_notices)
sharer_kill
sleep "$UPDATE_GAP"
sharer_start cs known crash
check_sync "S2c app crash"
END=$(( $(date +%s) + 12 ))
while [ "$(date +%s)" -lt "$END" ] && [ "$(drop_count)" -eq "$DROPS_BEFORE" ]; do
  sleep 0.5
done
res "S2c1 a crash adds one drop" \
  test "$(drop_count)" -eq "$(( DROPS_BEFORE + 1 ))"
res "S2c2 a crash sends one lost-connection notice" \
  test "$(lost_notices)" -eq "$(( LOST_BEFORE + 1 ))"
res "S2c3 the crash hint names the phone app" \
  grep -q "^JFD1 1 exit at=.* word=crash" "$DROPS"

# ---- S3: link down 5 s -----------------------------------------------------------
res "S3 link down 5 s: the laptop leaves Connected" link_down_gap 5
check_sync "S3 link down 5 s"

# ---- S4: link down 13 s ----------------------------------------------------------
res "S4 link down 13 s: the laptop leaves Connected" link_down_gap 13
check_sync "S4 link down 13 s"

# ---- S4b: link down 18 s: the phone's native display goes neutral too (its
# known-track Hello gap is 12 s; the 35 s stall is not reached).
ip netns exec jl-jo ip link set jl-lan down
P_NEUTRAL=0
BREAK=$(( $(date +%s) + 18 ))
while [ "$(date +%s)" -lt "$BREAK" ]; do
  if phone_neutral; then P_NEUTRAL=1; break; fi
  sleep 0.5
done
res "S4b link down 18 s: the phone leaves Connected too" test "$P_NEUTRAL" -eq 1
while [ "$(date +%s)" -lt "$BREAK" ]; do sleep 0.5; done
ip netns exec jl-jo ip link set jl-lan up
ip -n jl-jo route replace default via $LAN.1
check_sync "S4b link down 18 s"

# ---- S5: link down 40 s (past the 35 s stall: a real drop) -----------------------
BEFORE_DROPS=$(drop_count)
res "S5 link down 40 s: the laptop leaves Connected" link_down_gap "$STALL_GAP"
check_sync "S5 link down 40 s"
res "S5b the 40 s loss is one real drop" \
  test "$(drop_count)" -eq "$(( BEFORE_DROPS + 1 ))"

# ---- S6: laptop daemon restart (jafn0 kept) --------------------------------------
touch "$OUT/restart-cs"
sleep 3
check_sync "S6 laptop daemon restart"

touch "$OUT/kill-tray-cs"; wait $BTRAY
lab_assert_no_resolvectl "$OUT" "$OUT/joiner-cs-pair.log" "$OUT/joiner-cs-tray.log"

echo "---- the drop file of the laptop ----" | tee -a "$OUT/results.txt"
cat "$DROPS" 2>/dev/null | tee -a "$OUT/results.txt"
echo "---- the connection log of the laptop ----" | tee -a "$OUT/results.txt"
tail -20 "$ST/state-cs/jafnify/connection.log" 2>/dev/null | tee -a "$OUT/results.txt"

if [ "$FAILS" -eq 0 ]; then
  echo "RESULT: PASS" | tee -a "$OUT/results.txt"
else
  echo "RESULT: FAIL ($FAILS)" | tee -a "$OUT/results.txt"
fi

ip -br addr show wg0 > "$OUT/host-wg0-after" 2>&1
diff "$OUT/host-wg0-before" "$OUT/host-wg0-after" && echo "wg0 unchanged"
ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"

lab_finish
echo "=== $FAILS failed ==="
exit $FAILS
