#!/bin/bash
# Jafnify resilience lab (run as root). Same namespace layout as e2e-hotspot.sh
# (jl-jo laptop, jl-sh phone, jl-net internet at 203.0.113.1:8080). Scenarios:
#   K1  kill switch on: connected, the internet host is reached through the tunnel
#   K2  kill switch on, sharer killed (tunnel lost): jafn0 kept, kill switch rules
#       in place, the internet host is NOT reachable and the default route still
#       points into the tunnel (nothing falls back to the plain hotspot)
#   K3  kill switch on, `jafnify stop`: jafn0 and the kill switch chain are gone
#   K4  kill switch off, sharer killed: after the grace jafn0 is kept, the
#       internet works and the default route no longer points into the tunnel
#   K5  kill switch off, loss past the grace, then the sharer returns: the
#       tunnel route, jafn0's own fwmark rule and the IPv6 blackhole come back
#       through the password-free route binary (no helper up/down)
#   R1  kill switch off: laptop LAN link down for 20 s, then the sharer restarted
#       in known-peer mode from its stored pair (as the Android app auto-starts).
#       The joiner must be Connected again with no user action. The time from the
#       sharer restart to the first internet reply is printed.
#   R2  kill switch off: laptop LAN link down 20 s and up; the sharer keeps running.
#       The time from the link coming back to the first internet reply is printed.
#   R3  peer loss 40 s, then the sharer restarted in known mode. jafn0 is kept;
#       a changed endpoint is updated in place (inspect/endpoint), never down/up.
#       R3d checks the adoption rule is installed inside the lab namespace.
#   R4  the joiner process is stopped (SIGTERM) while jafn0 is up, then both
#       sides restart. The new joiner must inspect and adopt jafn0, show
#       Connecting until peer data arrives, and only then report Connected.
#   R5  delete jafn0.conf inside the joiner sandbox, then Disconnect: the helper
#       must remove jafn0 without the file. Connect again must recreate the
#       config and routing rule and reach the internet.
#   S1  fresh helper: delete the shared suppress rule in the joiner namespace;
#       the app's poll must restore it within 10 s.
# Each joiner runs in a private mount namespace (tmpfs /etc/wireguard, private
# HOME and XDG), so the host's /etc and config are never touched.
set -u
. "$(dirname "$0")/lib.sh"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOIN=${JOIN:-$ROOT/linux/target/release/jafnify}
SHARE=${SHARE:-$ROOT/app/wgserv/target/release/jafnify-sharer-host}
LAB_HELPER=${LAB_HELPER:-$ROOT/linux/target/release/jafnify-helper}
export LAB_HELPER
LAB_ROUTE=${LAB_ROUTE:-$ROOT/linux/target/release/jafnify-route}
export LAB_ROUTE
OUT=${LAB_OUT:-$(cd "$(dirname "$0")/../.." && pwd)/tmp/lab}/out-resilience
mkdir -p "$OUT/.tmp"; export TMPDIR="$OUT/.tmp"  # never system /tmp (RAM-backed, wiped on reboot)
ST=$OUT/state
CODE=4321
LAN=192.168.77
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
# Ubuntu (systemd) lets every user ping; a fresh namespace does not. The app's
# liveness probe is an ICMP echo through jafn0, so mirror the host default.
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
internet_ok()  { [ "$(ip netns exec jl-jo curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; }
# The phone normally gives the laptop plain internet too (a real hotspot NATs
# its clients). The lab keeps that off so internet_ok proves the tunnel; the
# fallback scenarios turn it on to check the laptop reaches the internet
# outside the tunnel.
hotspot_plain_on() {
  ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=1
  ip netns exec jl-sh iptables -t nat -C POSTROUTING -s $LAN.0/24 -o jl-cell -j MASQUERADE 2>/dev/null \
    || ip netns exec jl-sh iptables -t nat -A POSTROUTING -s $LAN.0/24 -o jl-cell -j MASQUERADE
}
hotspot_plain_off() {
  ip netns exec jl-sh iptables -t nat -D POSTROUTING -s $LAN.0/24 -o jl-cell -j MASQUERADE 2>/dev/null || true
  ip netns exec jl-sh sysctl -qw net.ipv4.ip_forward=0
}
internet_no()  { ! ip netns exec jl-jo curl -s -m 3 http://203.0.113.1:8080/ >/dev/null 2>&1; }
jafn0_up()     { ip netns exec jl-jo ip link show jafn0 >/dev/null 2>&1; }
jafn0_gone()   { ! jafn0_up; }
suppress_present() { ip netns exec jl-jo ip -4 rule show 2>/dev/null | grep -q 'from all lookup main suppress_prefixlength 0'; }
r5_done() { [ -f "$OUT/r5-done" ]; }
# The joiner's private mount namespace has the rule the fresh helper installed.
rule_installed() {
  local p
  for p in $(ip netns pids jl-jo); do
    [ -f "/proc/$p/root/etc/polkit-1/rules.d/49-jafnify.rules" ] && return 0
  done
  return 1
}
route_via_tunnel() { ip netns exec jl-jo ip route get 203.0.113.1 2>/dev/null | grep -q 'dev jafn0'; }
route_not_via_tunnel() { ! route_via_tunnel; }
# jafn0's own wg-quick rule (its mark, its table). Another tunnel's rule is not
# this one, so a scoped fallback must leave it alone (V1).
jafn0_mark() { ip netns exec jl-jo wg show jafn0 fwmark 2>/dev/null | sed 's#/.*##'; }
jafn0_rule_present() {
  local m
  m=$(jafn0_mark)
  [ -n "$m" ] && [ "$m" != "off" ] \
    && ip netns exec jl-jo ip -4 rule show 2>/dev/null | grep -Eq "fwmark $m(/[0-9a-fx]+)? lookup"
}
# wg-quick order: the shared suppress rule first, jafn0's rule right after it.
# A restore that lets the kernel pick the priority puts the tunnel rule first
# and pulls LAN traffic into the tunnel.
jafn0_rule_after_suppress() {
  local m rules sp tp
  m=$(jafn0_mark)
  rules=$(ip netns exec jl-jo ip -4 rule show 2>/dev/null)
  sp=$(printf '%s\n' "$rules" | sed -n 's/^\([0-9]*\):.*lookup main suppress_prefixlength 0.*/\1/p' | sort -n | head -n 1)
  tp=$(printf '%s\n' "$rules" | grep -E "fwmark $m(/[0-9a-fx]+)? lookup" | sed -n 's/^\([0-9]*\):.*/\1/p' | head -n 1)
  [ -n "$sp" ] && [ -n "$tp" ] && [ "$tp" -gt "$sp" ]
}
jafn0_rule_gone() { ! jafn0_rule_present; }
# The kernel prints the IPv6 default as "default", not "::/0".
blackhole_present() { ip netns exec jl-jo ip -6 route show 2>/dev/null | grep -Eq '^blackhole (default|::/0) '; }
jafnify_rules_gone() { jafn0_rule_gone && ! blackhole_present; }
ks_present()   { ip netns exec jl-jo iptables -S JAFNIFY_KS >/dev/null 2>&1; }
ks_gone()      { ! ks_present; }
# wait_for CMD SECONDS: poll every 0.5 s; returns 0 when CMD succeeds.
wait_for() { local end=$(( $(date +%s) + $2 )); while [ "$(date +%s)" -lt "$end" ]; do $1 && return 0; sleep 0.5; done; $1; }
# sharer_start LABEL [known]: the phone's sharer. The first start pairs with the
# code and keeps the pair in $ST/sh-LABEL; `known` restarts from that stored pair,
# as the Android app does when it auto-starts (no code).
sharer_start() {
  local l=$1 args
  mkdir -p "$ST/sh-$l"
  if [ "${2:-}" = known ]; then args=(); else args=(--code $CODE); fi
  ip netns exec jl-sh "$SHARE" "${args[@]}" --state-dir "$ST/sh-$l" --iface-ip $LAN.1 --iface-ip 10.77.0.1 > "$OUT/sharer-$l${2:+-$2}.log" 2>&1 &
}
sharer_kill() {
  for p in $(ip netns pids jl-sh); do grep -q sharer-host /proc/$p/cmdline 2>/dev/null && kill $p; done
  sleep 1
}
# sharer_paired LABEL: the sharer has paired and saved the pair, so a restart
# in known mode finds it. The sharer says "Connected" and keeps the pair in
# $ST/sh-LABEL/state.toml. Killing the sharer before it saved the pair leaves
# the later known-mode restart with no peer.
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
# joiner_block LABEL KILLSWITCH(on|off): the laptop app in a private mount ns.
# Runs until $OUT/stop-LABEL exists, then `jafnify stop` (inside the same ns).
# A logged pkexec wrapper counts every privileged call for that label, and the
# freshly built helper replaces any older installed helper inside the sandbox.
joiner_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    mkdir -p "$XDG_CONFIG_HOME"
    export PATH="$3:$PATH"
    if [ "$2" = on ]; then "$JOIN" kill-switch on; else "$JOIN" kill-switch off; fi
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1.log" 2>&1 &
    JP=$!
    while [ ! -f "$OUT/stop-$1" ]; do sleep 0.5; done
    "$JOIN" stop >> "$OUT/joiner-$1.log" 2>&1
    kill $JP 2>/dev/null; wait $JP 2>/dev/null
  ' _ "$1" "$2" "$bin" &
}

# joiner_restart_block LABEL: first joins with the code, then waits for
# $OUT/kill-LABEL to stop (SIGTERM, like the user unit), waits for
# $OUT/restart-LABEL and starts again in known mode on the same config.
joiner_restart_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    mkdir -p "$XDG_CONFIG_HOME"
    export PATH="$2:$PATH"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-first.log" 2>&1 &
    JP=$!
    while [ ! -f "$OUT/kill-$1" ]; do sleep 0.5; done
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    while [ ! -f "$OUT/restart-$1" ]; do sleep 0.5; done
    "$JOIN" join --accept-terms > "$OUT/joiner-$1-second.log" 2>&1 &
    JP=$!
    while [ ! -f "$OUT/stop-$1" ]; do sleep 0.5; done
    "$JOIN" stop >> "$OUT/joiner-$1-second.log" 2>&1
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
  ' _ "$1" "$bin" &
}

# joiner_hardkill_block LABEL: like joiner_restart_block, but the first process
# is killed with SIGKILL, so no in-process cleanup can run. The next start must
# clean the stale routing before it connects.
joiner_hardkill_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    mkdir -p "$XDG_CONFIG_HOME"
    export PATH="$2:$PATH"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-first.log" 2>&1 &
    JP=$!
    while [ ! -f "$OUT/kill-$1" ]; do sleep 0.5; done
    kill -9 $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    while [ ! -f "$OUT/restart-$1" ]; do sleep 0.5; done
    "$JOIN" join --accept-terms > "$OUT/joiner-$1-second.log" 2>&1 &
    JP=$!
    while [ ! -f "$OUT/stop-$1" ]; do sleep 0.5; done
    "$JOIN" stop >> "$OUT/joiner-$1-second.log" 2>&1
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
  ' _ "$1" "$bin" &
}

# joiner_tray_block LABEL: pairs headless, stops that process, then runs the
# tray service (daemon::run) on the stored pair. The harness signals the tray
# with SIGTERM through $OUT/kill-tray-LABEL, so its own stop path runs.
joiner_tray_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    export XDG_RUNTIME_DIR="$ST/run-$1"
    mkdir -p "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
    chmod 0700 "$XDG_RUNTIME_DIR"
    export PATH="$2:$PATH"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-pair.log" 2>&1 &
    JP=$!
    n=0
    while [ $n -lt 120 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then break; fi
      sleep 0.5; n=$((n+1))
    done
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    "$JOIN" tray > "$OUT/joiner-$1-tray.log" 2>&1 &
    TP=$!
    n=0
    while [ $n -lt 240 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then
        : > "$OUT/tray-up-$1"
        break
      fi
      sleep 0.5; n=$((n+1))
    done
    while [ ! -f "$OUT/kill-tray-$1" ]; do sleep 0.5; done
    kill $TP 2>/dev/null || true
    wait $TP 2>/dev/null || true
  ' _ "$1" "$bin" &
}

# joiner_missing_conf_block LABEL: one sandbox, one join process. It deletes the
# config while jafn0 is up, stops (the helper must clean up without the file),
# then joins in known mode and checks that up recreated the config and rule.
joiner_missing_conf_block() {
  local bin="$OUT/bin-$1"
  mkdir -p "$bin"
  printf '#!/bin/sh\necho "$@" >> "%s/pkexec-%s.log"\nexec /usr/bin/pkexec "$@"\n' \
    "$OUT" "$1" > "$bin/pkexec"
  chmod +x "$bin/pkexec"
  joiner_sandbox jl-jo bash -c '
    export HOME=$(mktemp -d) XDG_CONFIG_HOME="$ST/jo-$1"
    mkdir -p "$XDG_CONFIG_HOME"
    export PATH="$2:$PATH"
    "$JOIN" join --code "$CODE" --accept-terms > "$OUT/joiner-$1-first.log" 2>&1 &
    JP=$!
    n=0
    paired=0
    was_up=0
    while [ $n -lt 120 ]; do
      if grep -q "state Connecting -> Connected" "$OUT/joiner-$1-first.log" 2>/dev/null; then
        paired=1; break
      fi
      sleep 0.5; n=$((n+1))
    done
    [ $paired -eq 1 ] || echo "pair not persisted before deleting config" >> "$OUT/r5-fail"
    if [ $paired -eq 1 ] \
      && [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then
      was_up=1
    fi
    [ $was_up -eq 1 ] || echo "not connected before deleting config" >> "$OUT/r5-fail"
    mark=$(wg show jafn0 fwmark 2>/dev/null || true)
    rm -f /etc/wireguard/jafn0.conf
    "$JOIN" stop > "$OUT/joiner-$1-stop.log" 2>&1
    stale_rule=0
    stale_v6=0
    stale_wg=0
    if [ -n "$mark" ] && [ "$mark" != "off" ]; then
      mark=${mark%%/*}
      table=$((mark))
      ip -4 rule show | grep -q "lookup $table" && stale_rule=1
    fi
    ip -6 route show blackhole 2>/dev/null | grep -q "::/0" && stale_v6=1
    nft list tables 2>/dev/null | grep -q "wg-quick-jafn0" && stale_wg=1
    if [ $was_up -eq 1 ] && [ ! -e /sys/class/net/jafn0 ] \
      && [ $stale_rule -eq 0 ] && [ $stale_v6 -eq 0 ] && [ $stale_wg -eq 0 ]; then
      : > "$OUT/r5-stop-ok"
    else
      echo "jafn0/rules survived stop" >> "$OUT/r5-fail"
    fi
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    "$JOIN" join --accept-terms > "$OUT/joiner-$1-second.log" 2>&1 &
    JP=$!
    n=0
    ok=0
    while [ $n -lt 120 ]; do
      if [ "$(curl -s -m 2 http://203.0.113.1:8080/ || echo fail)" = JAFNIFY-E2E-OK ]; then ok=1; break; fi
      sleep 0.5; n=$((n+1))
    done
    mark=$(wg show jafn0 fwmark 2>/dev/null || true)
    table=
    if [ -n "$mark" ] && [ "$mark" != "off" ]; then mark=${mark%%/*}; table=$((mark)); fi
    if [ $ok -eq 1 ] && [ -s /etc/wireguard/jafn0.conf ] \
      && [ -f /etc/polkit-1/rules.d/49-jafnify.rules ] \
      && [ -n "$table" ] && ip -4 rule show | grep -q "lookup $table"; then
      : > "$OUT/r5-connect-ok"
    else
      echo "config/rule/internet missing" >> "$OUT/r5-fail"
    fi
    "$JOIN" stop > "$OUT/joiner-$1-stop2.log" 2>&1
    kill $JP 2>/dev/null || true
    wait $JP 2>/dev/null || true
    : > "$OUT/r5-done"
  ' _ "$1" "$bin" &
}

# elapsed_since T0_NS LABEL: prints the time since T0 (ns) and records it.
elapsed_since() { local ms=$(( ($(date +%s%N) - $1) / 1000000 )); echo "$2: $(( ms / 1000 )).$(printf %03d $(( ms % 1000 ))) s" | tee -a "$OUT/results.txt"; }

# ---- K1 - K3: kill switch on ------------------------------------------------------
sharer_start k1; sleep 1
joiner_block k1 on; BK1=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "K1 kill switch on: connected, the internet host is reached through the tunnel" internet_ok
sharer_kill
# Plain hotspot internet is available here on purpose: the kill switch must
# still block it after any grace.
hotspot_plain_on
wait_for internet_no 10; sleep 40
res "K2a kill switch on, tunnel lost: jafn0 is kept (no fallback by taking it down)" jafn0_up
res "K2b kill switch on, tunnel lost: the kill switch rules are in place" ks_present
res "K2c kill switch on, tunnel lost: the internet host is NOT reachable" internet_no
res "K2d kill switch on, tunnel lost: the default route still points into the tunnel" route_via_tunnel
hotspot_plain_off
touch "$OUT/stop-k1"; wait $BK1
res "K3a kill switch on, stop: jafn0 is gone" jafn0_gone
res "K3b kill switch on, stop: the kill switch chain is gone" ks_gone

# ---- K4: kill switch off, tunnel lost --------------------------------------------
sharer_start k4; sleep 1
joiner_block k4 off; BK4=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
sharer_kill
hotspot_plain_on
T0=$(date +%s%N)
# Kill switch off, the phone app died but its hotspot stays: liveness probes go
# unanswered and the system's own routing wins on its own.
if wait_for internet_ok 45; then
  K4_LIVE=true
  elapsed_since $T0 "K4 time from the phone app dying to the internet via the ordinary route"
  K4_MS=$(( ($(date +%s%N) - T0) / 1000000 ))
else
  K4_LIVE=false; K4_MS=999999
fi
res "K4e the ordinary route wins within 12 s (liveness, not a long grace)" \
  sh -c '[ "$1" -le 12000 ]' _ "$K4_MS"
res "K4a kill switch off, tunnel lost: jafn0 is kept (fast return)" jafn0_up
res "K4b kill switch off, tunnel lost: the internet host is reachable via the ordinary route" "$K4_LIVE"
res "K4c kill switch off, tunnel lost: the default route no longer points into the tunnel" route_not_via_tunnel
hotspot_plain_off
touch "$OUT/stop-k4"; wait $BK4
res "K4d kill switch off, stop after a fallback: no Jafnify rule is left behind" jafnify_rules_gone

# ---- K5: loss past the grace, then the sharer returns ------------------------------
# The return must hand routing back to the tunnel through the password-free route
# binary: the rule, the IPv6 blackhole and the DNS routing come back.
sharer_start k5; sleep 1
joiner_block k5 off; BK5=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
# The sharer must have saved the pair before it is killed: the return below
# starts in known mode and finds no peer without it.
res "K5z the sharer saved the pair before it was killed" sharer_paired k5
: > "$OUT/pkexec-k5.log"
sharer_kill
if wait_for route_not_via_tunnel 45; then K5_DOWN=true; else K5_DOWN=false; fi
res "K5a loss past the grace: the route left the tunnel" "$K5_DOWN"
res "K5b loss past the grace: jafn0's own fwmark rule is gone" jafn0_rule_gone
sharer_start k5 known
T0=$(date +%s%N)
if wait_for route_via_tunnel 120; then
  elapsed_since $T0 "K5 time from the sharer return to the tunnel route back"
  K5_BACK=true
else
  K5_BACK=false
fi
res "K5c the sharer returns: traffic goes through the tunnel again" "$K5_BACK"
res "K5d the sharer returns: jafn0's fwmark rule is back" jafn0_rule_present
res "K5e the sharer returns: the IPv6 blackhole is back" blackhole_present
res "K5h the sharer returns: jafn0's rule sits after the suppress rule (wg-quick order)" \
  jafn0_rule_after_suppress
res "K5f the return used the password-free route binary, not a helper up/down" \
  sh -c "grep -q 'jafnify-route route-restore' '$OUT/pkexec-k5.log' \
         && ! grep -Eq 'jafnify-helper (up|down)|wg-quick' '$OUT/pkexec-k5.log'"
touch "$OUT/stop-k5"; wait $BK5
res "K5g stop: no Jafnify rule is left behind" jafnify_rules_gone
sharer_kill

# ---- R1: laptop LAN down 20 s + sharer restarted in known mode (as the phone's auto-start)
sharer_start r1; sleep 1
joiner_block r1 off; BR1=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "R1 before the outage: connected" internet_ok
res "R1z the sharer saved the pair before it was killed" sharer_paired r1
ip netns exec jl-jo ip link set jl-lan down
sharer_kill
sleep 20
ip netns exec jl-jo ip link set jl-lan up
# Link down drops the default route; DHCP/NetworkManager restore it on a real laptop.
ip -n jl-jo route replace default via $LAN.1
sleep 1
sharer_start r1 known
T0=$(date +%s%N)
if wait_for internet_ok 120; then
  elapsed_since $T0 "R1 time from the sharer restart to the internet reachable again"
  res "R1 after the link outage and sharer restart: connected again with no user action (within 120 s)" true
else
  res "R1 after the link outage and sharer restart: connected again with no user action (within 120 s)" false
fi
touch "$OUT/stop-r1"; wait $BR1
sharer_kill

# ---- R2: laptop LAN down 20 s and up again; the sharer keeps running -------------------
sharer_start r2; sleep 1
joiner_block r2 off; BR2=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "R2 before the outage: connected" internet_ok
ip netns exec jl-jo ip link set jl-lan down
sleep 20
ip netns exec jl-jo ip link set jl-lan up
ip -n jl-jo route replace default via $LAN.1
T0=$(date +%s%N)
if wait_for internet_ok 120; then
  elapsed_since $T0 "R2 time from the link coming back to the internet reachable"
  res "R2 after the link outage: connected again with no user action (within 120 s)" true
else
  res "R2 after the link outage: connected again with no user action (within 120 s)" false
fi
touch "$OUT/stop-r2"; wait $BR2

# ---- R3: peer loss and return must not ask for root --------------------------------
# Count every pkexec the joiner makes after the initial connection. The old
# behaviour ran `wg-quick down` on loss and `wg-quick up` on return.
sharer_start r3; sleep 1
joiner_block r3 off; BR3=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "R3z the sharer saved the pair before it was killed" sharer_paired r3
: > "$OUT/pkexec-r3.log"
sharer_kill
sleep 40
res "R3a peer loss: jafn0 is kept" jafn0_up
sharer_start r3 known
T0=$(date +%s%N)
if wait_for internet_ok 120; then
  elapsed_since $T0 "R3 time from the sharer restart to the internet reachable again"
  res "R3b peer loss and return: connected again with no user action" true
else
  res "R3b peer loss and return: connected again with no user action" false
fi
# The sharer was restarted, so it is a new session with new WireGuard keys.
# Since f7e562f the joiner sets jafn0 up again exactly once (a kept tunnel
# with the old keys never handshakes). More than one rebuild, or wg-quick,
# is a failure. The last "down" in the log is the scenario's own stop.
res "R3c a restarted sharer leads to exactly one tunnel rebuild, no wg-quick" \
  sh -c "[ \$(grep -c 'jafnify-helper up' '$OUT/pkexec-r3.log') -eq 1 ] && ! grep -q 'wg-quick' '$OUT/pkexec-r3.log' && grep -q 'the peer started a new session' '$OUT/joiner-r3.log'"
res "R3d the adoption rule is present in the lab mount namespace" rule_installed
touch "$OUT/stop-r3"; wait $BR3
sharer_kill

# ---- R4: joiner daemon restart with jafn0 up, sharer new session ------------------
sharer_start r4; sleep 1
joiner_restart_block r4; BR4=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "R4 before the restart: connected" internet_ok
res "R4z the sharer saved the pair before it was killed" sharer_paired r4
sharer_kill
sleep 2
touch "$OUT/kill-r4"
sleep 2
res "R4a stopping the joiner (SIGTERM) keeps jafn0" jafn0_up
# SIGTERM must leave the laptop online: the handler removes the dead routing.
hotspot_plain_on
if wait_for internet_ok 45; then R4_LIVE=true; else R4_LIVE=false; fi
res "R4a2 stopping the joiner (SIGTERM) leaves the internet reachable" "$R4_LIVE"
res "R4a3 after SIGTERM the default route no longer points into the tunnel" route_not_via_tunnel
hotspot_plain_off
sharer_start r4 known
touch "$OUT/restart-r4"
T0=$(date +%s%N)
if wait_for internet_ok 120; then
  elapsed_since $T0 "R4 time from the new sharer session to the internet reachable"
  res "R4b daemon restart and new sharer session reach the internet" true
else
  res "R4b daemon restart and new sharer session reach the internet" false
fi
adopt_line=$(grep -n 'adopted existing jafn0 after inspect; waiting for peer data' \
  "$OUT/joiner-r4-second.log" | head -1 | cut -d: -f1)
connected_line=$(grep -n 'connected (peer' "$OUT/joiner-r4-second.log" \
  | head -1 | cut -d: -f1)
res "R4c adopted only after inspect and before Connected" \
  sh -c '[ -n "$1" ] && [ -n "$2" ] && [ "$1" -lt "$2" ]' _ "$adopt_line" "$connected_line"
touch "$OUT/stop-r4"; wait $BR4
res "R4h headless stop after the restart leaves no Jafnify rule behind" jafnify_rules_gone
sharer_kill

# ---- R4d: the tray service's own SIGTERM path --------------------------------------
tray_up_r4t() { [ -f "$OUT/tray-up-r4t" ]; }
sharer_start r4t; sleep 1
joiner_tray_block r4t; BT4=$!
if wait_for tray_up_r4t 180; then R4T_UP=true; else R4T_UP=false; fi
res "R4d the tray service connects on its own" "$R4T_UP"
sharer_kill
sleep 2
touch "$OUT/kill-tray-r4t"
hotspot_plain_on
if wait_for internet_ok 45; then R4T_LIVE=true; else R4T_LIVE=false; fi
res "R4e tray mode SIGTERM leaves the internet reachable" "$R4T_LIVE"
res "R4f tray mode SIGTERM leaves the route out of the tunnel" route_not_via_tunnel
wait $BT4
res "R4g tray stop leaves no Jafnify rule behind" jafnify_rules_gone
hotspot_plain_off
sharer_kill
# The tray stop keeps jafn0 on purpose (fast return), but its config lived in
# this scenario's private tmpfs and is gone. Start R5 from a clean namespace.
ip netns exec jl-jo ip link del jafn0 2>/dev/null || true

# ---- R5: teardown without the config file ------------------------------------------
sharer_start r5; sleep 1
joiner_missing_conf_block r5; BR5=$!
wait_for r5_done 180
res "R5a missing config teardown removes jafn0" \
  sh -c '[ -f "$1/r5-stop-ok" ] && [ ! -f "$1/r5-fail" ]' _ "$OUT"
res "R5b next connect recreates config and routing rule" \
  sh -c '[ -f "$1/r5-connect-ok" ] && [ ! -f "$1/r5-fail" ]' _ "$OUT"
# The password note is not checked here: the lab joiner runs as root, and polkit
# always authorizes root, so pkcheck says "no prompt" in both connects. The note
# is covered by polkit_hint_test (fake pkcheck) and by a live pkcheck check of
# the installed rule.
wait $BR5
sharer_kill

# ---- R6: joiner killed with -9 while up; the next start must restore routing ------
sharer_start r6; sleep 1
joiner_hardkill_block r6; BR6=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "R6 before the -9: connected" internet_ok
res "R6z the sharer saved the pair before it was killed" sharer_paired r6
sharer_kill
sleep 2
touch "$OUT/kill-r6"
sleep 1
sharer_start r6 known
touch "$OUT/restart-r6"
T0=$(date +%s%N)
if wait_for internet_ok 90; then
  elapsed_since $T0 "R6 time from the restart after -9 to the internet reachable"
  res "R6 a -9 kill and restart leave the laptop online" true
else
  res "R6 a -9 kill and restart leave the laptop online" false
fi
touch "$OUT/stop-r6"; wait $BR6
res "R6b stop after the -9 restart leaves no Jafnify rule behind" jafnify_rules_gone
sharer_kill

# ---- V1: a second full-tunnel WireGuard interface keeps its own routing ------------
# A user's other VPN marks its own traffic and uses its own table. A Jafnify
# fallback must remove only jafn0's rule, never this one.
ip netns exec jl-jo ip link add vpn0 type wireguard
ip netns exec jl-jo wg genkey > "$OUT/vpn0.key"
ip netns exec jl-jo wg pubkey < "$OUT/vpn0.key" > "$OUT/vpn0.pub"
ip netns exec jl-jo wg set vpn0 private-key "$OUT/vpn0.key" \
  peer "$(cat "$OUT/vpn0.pub")" allowed-ips 0.0.0.0/0
ip netns exec jl-jo wg set vpn0 fwmark 0xc000
ip netns exec jl-jo ip -4 rule add fwmark 0xc000 lookup 51999 priority 250
ip netns exec jl-jo ip link set vpn0 up
ip netns exec jl-jo ip route add default dev vpn0 table 51999
sharer_start v1; sleep 1
joiner_block v1 off; BV1=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
{ ip netns exec jl-jo ip -4 rule show; ip netns exec jl-jo ip route show table 51999; \
  ip netns exec jl-jo ip -br link show vpn0; \
  ip netns exec jl-jo ip route get 203.0.113.1 mark 0xc000; } > "$OUT/v1-before.txt" 2>&1
# wg-quick puts jafn0's rules just before the first existing rule, so while
# jafn0 is Connected they win over this one; that is wg-quick, not Jafnify.
res "V1 before the fallback: the other tunnel's rule and table are in place" \
  sh -c 'ip netns exec jl-jo ip -4 rule show | grep -Eq "fwmark 0xc000(/0x[0-9a-f]+)? lookup 51999" && ip netns exec jl-jo ip route show table 51999 | grep -q "dev vpn0"'
sharer_kill
wait_for route_not_via_tunnel 45
res "V1a the Jafnify fallback happened" route_not_via_tunnel
res "V1b the other tunnel's fwmark rule survived the fallback" \
  sh -c 'ip netns exec jl-jo ip -4 rule show | grep -Eq "fwmark 0xc000(/0x[0-9a-f]+)? lookup 51999"'
res "V1c the other tunnel's traffic still goes through it after the fallback" \
  sh -c 'ip netns exec jl-jo ip route get 203.0.113.1 mark 0xc000 | grep -q "dev vpn0"'
touch "$OUT/stop-v1"; wait $BV1
ip netns exec jl-jo ip link del vpn0
ip netns exec jl-jo ip -4 rule del priority 250 2>/dev/null || true
sharer_kill

# ---- N1: normal Wi-Fi, never connected: install no tunnel routing ----------------
joiner_block n1 off; BN1=$!
sleep 10
res "N1 never connected: no tunnel interface" jafn0_gone
res "N1 never connected: no Jafnify policy rule" jafn0_rule_gone
res "N1 never connected: no IPv6 blackhole" \
  sh -c '! ip netns exec jl-jo ip -6 route show 2>/dev/null | grep -Eq "^blackhole (default|::/0) "'
hotspot_plain_on
res "N1 never connected: the internet host is reachable" internet_ok
hotspot_plain_off
touch "$OUT/stop-n1"; wait $BN1

# ---- S1: a co-existing tool deletes the shared suppress rule; Jafnify restores it --
# Needs the freshly built helper (the app repairs through its fixed `suppress` verb).
if [ -x "$LAB_HELPER" ]; then
  sharer_start s1; sleep 1
  joiner_block s1 off; BS1=$!
  wait_for jafn0_up 60 && wait_for internet_ok 60
  ip netns exec jl-jo ip -4 rule del table main suppress_prefixlength 0 2>/dev/null || true
  if wait_for suppress_present 10; then
    res "S1 suppress rule removed externally is restored" true
  else
    res "S1 suppress rule removed externally is restored" false
  fi
  touch "$OUT/stop-s1"; wait $BS1
  sharer_kill
else
  echo "SKIP  S1 suppress-rule repair: build linux/target/release/jafnify-helper first" \
    | tee -a "$OUT/results.txt"
fi

# ---- L1: connected and idle for 60 s: the routing never flaps -----------------
sharer_start l1; sleep 1
joiner_block l1 off; BL1=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "L1 before idling: connected" internet_ok
: > "$OUT/pkexec-l1.log"
sleep 60
res "L1 60 s idle: no routing change (no route-fallback/restore)" \
  sh -c "! grep -Eq 'route-(fallback|restore)' '$OUT/pkexec-l1.log'"
res "L1 after idling: still through the tunnel" route_via_tunnel
touch "$OUT/stop-l1"; wait $BL1
sharer_kill

# ---- N2: the laptop's Wi-Fi joins another network (no phone there) -----------------
# The same interface comes back on a network where the phone's address answers
# nothing: the hotspot end moves to the plain internet namespace. The system's
# own routing must win within seconds.
sharer_start n2; sleep 1
joiner_block n2 off; BN2=$!
wait_for jafn0_up 60 && wait_for internet_ok 60
res "N2 before the switch: through the tunnel" route_via_tunnel
ip -n jl-jo link set jl-lan down
ip -n jl-sh link set jl-hot netns jl-net
ip -n jl-net addr add $LAN.1/24 dev jl-hot
ip -n jl-net link set jl-hot up
# A real Wi-Fi switch drops the carrier for a few seconds (disassociate,
# associate, DHCP); 1 s is shorter than any real switch.
sleep 3
ip -n jl-jo link set jl-lan up
ip -n jl-jo route replace default via $LAN.1
T0=$(date +%s%N)
if wait_for internet_ok 30; then
  elapsed_since $T0 "N2 time from joining the other network to the internet"
  N2_MS=$(( ($(date +%s%N) - T0) / 1000000 ))
else
  N2_MS=999999
fi
# Usually the link-back probe decides in ~2 s; the bound is the 7 s liveness
# rule plus up to ~1 s of this script's own polling.
res "N2 another network: the internet works within 8 s" sh -c '[ "$1" -le 8000 ]' _ "$N2_MS"
res "N2 another network: the route left the tunnel" route_not_via_tunnel
touch "$OUT/stop-n2"; wait $BN2
sharer_kill

# ---- report ----------------------------------------------------------------------------
echo "=== joiner k1 (kill switch on) ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner-k1.log" 2>/dev/null | head -20
echo "=== joiner r1 ==="; sed -E "s#[A-Za-z0-9+/]{43}=#<KEY>#g" "$OUT/joiner-r1.log" 2>/dev/null | tail -20
echo "=== host after ==="; ip -br addr show wg0 > "$OUT/host-wg0-after" 2>&1; diff "$OUT/host-wg0-before" "$OUT/host-wg0-after" && echo "wg0 unchanged"
ip route show default | head -2 | diff "$OUT/host-before" - && echo "host default route unchanged"
lab_assert_no_resolvectl "$OUT" "$OUT"/joiner-*.log
lab_finish
echo "=== $FAILS failed ==="
exit $FAILS
