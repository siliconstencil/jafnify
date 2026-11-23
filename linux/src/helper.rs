//! Root helper protocol: a tiny fixed command set for the polkit action.
//!
//! The helper never accepts arbitrary shell, paths or wg-quick arguments. It
//! receives structured fields, rebuilds the config with the same core
//! generator as the app, and runs fixed commands as root.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use jafnify_core::config::{
    build_config, ConfigGenerator, InboundGuard, KillSwitch, WireGuardConfig,
};
use jafnify_core::state::Role;
use serde::{Deserialize, Serialize};

pub const IFACE: &str = "jafn0";
pub const CONFIG_PATH: &str = "/etc/wireguard/jafn0.conf";
pub const WG_QUICK: &str = "/usr/bin/wg-quick";
pub const WG: &str = "/usr/bin/wg";
pub const IP: &str = "/usr/bin/ip";
pub const MAX_INPUT: u64 = 64 * 1024;

/// The helper prefixes a DNS-routing failure with this line on stderr. The
/// app surfaces it as one WARN, so a real failure is never hidden.
pub const DNS_WARNING_PREFIX: &str = "jafnify-helper: dns routing: ";

/// The DNS-routing warning in a helper's stderr, if it printed one. The root
/// helper and the route binary both use this marker after their own name.
pub fn dns_warning(stderr: &str) -> Option<&str> {
    const MARKER: &str = "dns routing: ";
    stderr
        .lines()
        .map(str::trim)
        .find_map(|line| line.find(MARKER).map(|at| &line[at + MARKER.len()..]))
}

/// The helper prefixes an IPv6-guard failure with this line on stderr. The
/// app surfaces it as one WARN: IPv6 can leave around the tunnel.
pub const IPV6_WARNING_PREFIX: &str = "jafnify-helper: ipv6 guard: ";

/// The IPv6-guard warning in a helper's stderr, if it printed one. The root
/// helper and the route binary both use this marker after their own name.
pub fn ipv6_warning(stderr: &str) -> Option<&str> {
    const MARKER: &str = "ipv6 guard: ";
    stderr
        .lines()
        .map(str::trim)
        .find_map(|line| line.find(MARKER).map(|at| &line[at + MARKER.len()..]))
}

/// The app's WARN line for an IPv6-guard failure.
pub fn ipv6_warning_line(reason: &str) -> String {
    format!("IPv6 can leave outside the tunnel: {reason}")
}

/// Teardown never depends on the config file: wg-quick cannot run without it,
/// so the fixed cleanup below removes the link, routing rules, firewall state
/// and DNS entry first. Success is reported only after the interface is gone.
pub(crate) const DOWN_SCRIPT: &str = r#"
had_iface=0
[ -e "$2" ] && had_iface=1
had_conf=0
[ -e "$1" ] && had_conf=1
if [ "$had_iface" -eq 0 ] && [ "$had_conf" -eq 0 ]; then
  rm -f -- "$1" "$3" 2>/dev/null || true
  exit 0
fi
if [ "$had_conf" -eq 1 ] && [ "$had_iface" -eq 1 ]; then
  wg-quick down jafn0 >/dev/null 2>&1 || true
fi
mark=
if [ "$had_iface" -eq 1 ]; then
  mark=$(wg show jafn0 fwmark 2>/dev/null || true)
  ip link del dev jafn0 >/dev/null 2>&1 || true
  if [ -n "$mark" ] && [ "$mark" != "off" ]; then
    mark=${mark%%/*}
    table=$((mark))
    ip -4 rule del not from all fwmark "$mark" lookup "$table" >/dev/null 2>&1 || true
    ip -4 route flush table "$table" >/dev/null 2>&1 || true
  fi
  ip -4 rule del table main suppress_prefixlength 0 >/dev/null 2>&1 || true
  ip -6 route del blackhole ::/0 metric 1 >/dev/null 2>&1 || true
  nft delete table inet jafnify_guard >/dev/null 2>&1 || true
  nft delete table inet jafnify_killswitch >/dev/null 2>&1 || true
  nft delete table inet wg-quick-jafn0 >/dev/null 2>&1 || true
  nft delete table ip wg-quick-jafn0 >/dev/null 2>&1 || true
  iptables -D INPUT -i jafn0 -m conntrack --ctstate NEW -j DROP >/dev/null 2>&1 || true
  iptables -D INPUT ! -i jafn0 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT >/dev/null 2>&1 || true
  iptables -D FORWARD ! -i jafn0 -o jafn0 -j ACCEPT >/dev/null 2>&1 || true
  iptables -D FORWARD -i jafn0 -j ACCEPT >/dev/null 2>&1 || true
  iptables -t mangle -D POSTROUTING -o jafn0 -j MARK --set-mark "$mark" >/dev/null 2>&1 || true
  iptables -t mangle -D PREROUTING -i jafn0 -j MARK --set-mark "$mark" >/dev/null 2>&1 || true
  iptables -D OUTPUT -j JAFNIFY_KS >/dev/null 2>&1 || true
  iptables -F JAFNIFY_KS >/dev/null 2>&1 || true
  iptables -X JAFNIFY_KS >/dev/null 2>&1 || true
  resolvectl revert jafn0 >/dev/null 2>&1 || true
  resolvconf -d jafn0 >/dev/null 2>&1 || true
fi
rm -f -- "$1" 2>/dev/null || true
[ -n "$3" ] && { rm -f -- "$3" 2>/dev/null || true; }
[ ! -e "$1" ] || { echo "jafn0.conf is still present after teardown" >&2; exit 1; }
[ ! -e "$2" ] || { echo "jafn0 is still present after teardown" >&2; exit 1; }
"#;

const ROOT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub fn teardown(iface_path: &Path, conf_path: &Path) -> Result<(), String> {
    teardown_with_path(iface_path, conf_path, Some(Path::new(ROOT_PATH)))
}

/// Testable form of [`teardown`]: the fixed script uses the supplied PATH.
pub fn teardown_with_path(
    iface_path: &Path,
    conf_path: &Path,
    path: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", DOWN_SCRIPT, "sh"])
        .arg(conf_path)
        .arg(iface_path)
        .arg("");
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let ok = cmd
        .status()
        .map_err(|e| format!("cannot run teardown: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err("jafn0 is still present after teardown".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperCommand {
    Up,
    Down,
    Status,
    Inspect,
    Endpoint,
    Forget,
    Suppress,
    HandshakeAge,
    /// Kill switch off: let the main table win again without taking jafn0 down.
    RouteFallback,
    /// Connected again: put the tunnel routing, blackhole and DNS back.
    RouteRestore,
}

/// How old the peer's newest WireGuard handshake is. Read-only, from the
/// helper: the app never talks to `wg` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeAge {
    Seconds(u64),
    Never,
    Unknown,
}

/// The helper accepts exactly one known verb and no extra arguments. All
/// request data for `up` arrives on stdin and is validated separately.
pub fn parse_command(args: &[String]) -> Result<HelperCommand, String> {
    if args.len() != 1 {
        return Err(
            "usage: jafnify-helper up|down|status|inspect|endpoint|forget|suppress|handshake-age|route-fallback|route-restore"
                .to_string(),
        );
    }
    match args[0].as_str() {
        "up" => Ok(HelperCommand::Up),
        "down" => Ok(HelperCommand::Down),
        "status" => Ok(HelperCommand::Status),
        "inspect" => Ok(HelperCommand::Inspect),
        "endpoint" => Ok(HelperCommand::Endpoint),
        "forget" => Ok(HelperCommand::Forget),
        "suppress" => Ok(HelperCommand::Suppress),
        "handshake-age" => Ok(HelperCommand::HandshakeAge),
        "route-fallback" => Ok(HelperCommand::RouteFallback),
        "route-restore" => Ok(HelperCommand::RouteRestore),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Parse `wg show jafn0 latest-handshakes`: one `<pubkey>\t<epoch-seconds>`
/// line per peer. The newest peer wins; timestamp 0 means it never handshook.
/// Empty or wholly malformed output is `None`.
pub fn parse_latest_handshakes(text: &str, now_epoch: u64) -> Option<HandshakeAge> {
    let mut newest: Option<u64> = None;
    for line in text.lines() {
        let Some((key, stamp)) = line.split_once('\t') else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        let Ok(stamp) = stamp.trim().parse::<u64>() else {
            continue;
        };
        newest = Some(newest.map_or(stamp, |current| current.max(stamp)));
    }
    let newest = newest?;
    if newest == 0 {
        Some(HandshakeAge::Never)
    } else {
        Some(HandshakeAge::Seconds(now_epoch.saturating_sub(newest)))
    }
}

/// Parse the helper's own answer: exactly one line, `never` or whole seconds.
pub fn parse_handshake_age(text: &str) -> HandshakeAge {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') || text.contains('\r') {
        return HandshakeAge::Unknown;
    }
    if text == "never" {
        return HandshakeAge::Never;
    }
    match text.parse::<u64>() {
        Ok(seconds) => HandshakeAge::Seconds(seconds),
        Err(_) => HandshakeAge::Unknown,
    }
}

/// The public fields the app can safely compare with what `jafn0` has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveConfig {
    pub peer_public_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub allowed_ips: String,
    pub address: String,
    pub kill_switch: bool,
}

impl LiveConfig {
    pub fn from_config(cfg: &WireGuardConfig, kill_switch: bool) -> Self {
        Self {
            peer_public_key: cfg.peer.public_key.clone(),
            endpoint: cfg.peer.endpoint.clone(),
            allowed_ips: cfg.peer.allowed_ips.clone(),
            address: cfg.interface.address.clone(),
            kill_switch,
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| format!("cannot encode live config: {e}"))
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("bad live config: {e}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reuse {
    Same,
    Endpoint,
    Different,
}

/// A live tunnel may be adopted only when everything except the endpoint
/// matches. The endpoint is learned by the sharer and may be stale in the
/// stored pair; peer data still proves the tunnel works.
pub fn adoption_match(desired: &LiveConfig, live: &LiveConfig) -> bool {
    desired.peer_public_key == live.peer_public_key
        && desired.allowed_ips == live.allowed_ips
        && desired.address == live.address
        && desired.kill_switch == live.kill_switch
}

/// Compare what the new session needs with what `jafn0` currently has.
/// Endpoint-only changes can be applied with `wg set`; anything else needs a
/// normal helper `up` (which replaces the interface).
pub fn reuse_decision(desired: &LiveConfig, live: &LiveConfig) -> Reuse {
    if desired.peer_public_key != live.peer_public_key
        || desired.allowed_ips != live.allowed_ips
        || desired.address != live.address
        || desired.kill_switch != live.kill_switch
    {
        return Reuse::Different;
    }
    if desired.endpoint == live.endpoint {
        Reuse::Same
    } else if desired.endpoint.is_some() {
        Reuse::Endpoint
    } else {
        Reuse::Different
    }
}

/// True when `ip -4 rule show` already has the shared rule wg-quick installs
/// with a default route. Any priority counts; another tool may have added it.
pub fn suppress_rule_present(text: &str) -> bool {
    text.lines().any(|line| {
        if !line.contains("from all lookup main suppress_prefixlength 0") {
            return false;
        }
        line.split(':')
            .next()
            .and_then(|p| p.trim().parse::<u32>().ok())
            .is_some_and(|priority| priority < 32766)
    })
}

/// `wg show <iface> endpoints` / `allowed-ips` print one `<pubkey>\t<value>`
/// line per peer. Pick the requested peer; `(none)` is returned as-is.
pub fn peer_value(text: &str, peer: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('\t').or_else(|| line.split_once(' '))?;
        (key.trim() == peer).then(|| value.trim().to_string())
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequest {
    pub role: String,
    pub private_key: String,
    pub peer_public_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub guard: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub kill_switch: bool,
    #[serde(default)]
    pub lan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Down,
    Up { peer: Option<String> },
}

/// Validate a request and render the exact config wg-quick will see.
pub fn render(text: &str) -> Result<String, String> {
    let req: HelperRequest = toml::from_str(text).map_err(|e| format!("bad request: {e}"))?;
    let role = match req.role.as_str() {
        "joiner" => Role::Joiner,
        "sharer" => Role::Sharer,
        other => return Err(format!("bad role: {other}")),
    };
    let guard = match req.guard.as_str() {
        "iptables" => InboundGuard::Iptables,
        "nft" => InboundGuard::Nft,
        "none" => InboundGuard::None,
        other => return Err(format!("bad guard: {other}")),
    };
    validate_private(&req.private_key)?;
    validate_public(&req.peer_public_key)?;
    let endpoint = req
        .endpoint
        .as_deref()
        .map(|e| {
            e.parse::<SocketAddr>()
                .map_err(|_| format!("bad endpoint: {e}"))
        })
        .transpose()?;
    let kill = if req.kill_switch {
        let lan = req.lan.ok_or("kill switch needs a LAN subnet")?;
        if role != Role::Joiner {
            return Err("the kill switch is joiner-only".to_string());
        }
        Some(KillSwitch {
            iptables: guard == InboundGuard::Iptables,
            lan: validate_lan(&lan)?,
        })
    } else {
        None
    };
    let cfg = build_config(
        role,
        &req.private_key,
        &req.peer_public_key,
        endpoint,
        guard,
        req.resolved,
        kill.as_ref(),
    );
    Ok(ConfigGenerator.to_wg_quick_text(&cfg))
}

fn validate_private(key: &str) -> Result<(), String> {
    jafnify_core::KeyPair::from_private_b64(key)
        .map(|_| ())
        .map_err(|_| "bad private key".to_string())
}

/// The exact request the app sends to the helper.
pub fn request_toml(
    role: Role,
    private_key: &str,
    peer_public_key: &str,
    endpoint: Option<SocketAddr>,
    guard: InboundGuard,
    resolved: bool,
    kill: Option<&KillSwitch>,
) -> String {
    let req = HelperRequest {
        role: match role {
            Role::Joiner => "joiner",
            Role::Sharer => "sharer",
        }
        .to_string(),
        private_key: private_key.to_string(),
        peer_public_key: peer_public_key.to_string(),
        endpoint: endpoint.map(|e| e.to_string()),
        guard: match guard {
            InboundGuard::Iptables => "iptables",
            InboundGuard::Nft => "nft",
            InboundGuard::None => "none",
        }
        .to_string(),
        resolved,
        kill_switch: kill.is_some(),
        lan: kill.map(|k| k.lan.clone()),
    };
    toml::to_string(&req).expect("the helper request is serializable")
}

fn validate_lan(value: &str) -> Result<String, String> {
    let (ip, prefix) = value.split_once('/').ok_or("bad LAN subnet")?;
    let ip: Ipv4Addr = ip.parse().map_err(|_| "bad LAN subnet".to_string())?;
    let prefix: u8 = prefix.parse().map_err(|_| "bad LAN subnet".to_string())?;
    crate::firewall::lan_subnet(ip, prefix).ok_or_else(|| "bad LAN subnet".to_string())
}

fn validate_public(key: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| "bad peer key".to_string())?;
    if bytes.len() != 32 || bytes.iter().all(|b| *b == 0) {
        return Err("bad peer key".to_string());
    }
    Ok(())
}

/// The helper's status line protocol, parsed without trusting the caller.
pub fn parse_status(text: &str) -> Status {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    match lines.next().map(str::trim) {
        Some("up") => {
            let peer = lines
                .next()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty());
            Status::Up { peer }
        }
        _ => Status::Down,
    }
}

/// `up`/`down` as the helper prints them.
pub fn status_text(status: &Status) -> String {
    match status {
        Status::Down => "down\n".to_string(),
        Status::Up { peer: Some(peer) } => format!("up\n{peer}\n"),
        Status::Up { peer: None } => "up\n".to_string(),
    }
}

pub fn read_request(input: &mut impl Read) -> Result<String, String> {
    let mut text = String::new();
    input
        .take(MAX_INPUT)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read the request: {e}"))?;
    if text.is_empty() {
        return Err("empty request".to_string());
    }
    Ok(text)
}

/// The fixed paths the helper writes and runs. Tests inspect the shape; they
/// never execute these.
pub fn paths() -> (PathBuf, PathBuf) {
    (PathBuf::from(CONFIG_PATH), PathBuf::from(WG_QUICK))
}

pub fn interface_path() -> PathBuf {
    Path::new("/sys/class/net").join(IFACE)
}

#[cfg(test)]
mod tests {
    use super::{
        dns_warning, ipv6_warning, ipv6_warning_line, parse_handshake_age,
        parse_latest_handshakes, HandshakeAge, DOWN_SCRIPT,
    };

    #[test]
    fn teardown_removes_only_jafn0s_own_rule() {
        // A fallback keeps jafn0, so the rule is removed in the interface
        // branch. It is jafn0's own mark/table rule; a global main-table rule
        // (an administrator's or another VPN's) must survive.
        assert!(
            DOWN_SCRIPT.contains("ip -4 rule del not from all fwmark"),
            "jafn0's own rule must be removed"
        );
        assert!(
            !DOWN_SCRIPT.contains("priority 100"),
            "another tool's rule must survive teardown"
        );
    }

    #[test]
    fn teardown_script_uses_only_injected_paths() {
        assert!(!DOWN_SCRIPT.contains("/etc/wireguard"), "host config path");
        assert!(!DOWN_SCRIPT.contains("/sys/class/net"), "host iface path");
        assert!(DOWN_SCRIPT.contains("\"$1\""), "config path argument");
        assert!(DOWN_SCRIPT.contains("\"$2\""), "interface path argument");
    }

    #[test]
    fn the_newest_handshake_wins_and_old_timestamps_are_ages() {
        let text = "AAAA\t1000\nBBBB\t1200\nCCCC\t900\n";
        assert_eq!(
            parse_latest_handshakes(text, 1300),
            Some(HandshakeAge::Seconds(100))
        );
        assert_eq!(
            parse_latest_handshakes(text, 1200),
            Some(HandshakeAge::Seconds(0))
        );
        assert_eq!(
            parse_latest_handshakes("AAAA\t2000\n", 1900),
            Some(HandshakeAge::Seconds(0)),
            "a clock skew never underflows"
        );
    }

    #[test]
    fn a_zero_handshake_is_never() {
        assert_eq!(
            parse_latest_handshakes("AAAA\t0\nBBBB\t0\n", 5000),
            Some(HandshakeAge::Never)
        );
    }

    #[test]
    fn empty_and_garbage_handshake_output_is_none_or_ignored() {
        assert_eq!(parse_latest_handshakes("", 5000), None);
        assert_eq!(parse_latest_handshakes("garbage\nAAAA\n", 5000), None);
        assert_eq!(
            parse_latest_handshakes("garbage\nAAAA\t10\nBBBB\tnope\n", 20),
            Some(HandshakeAge::Seconds(10)),
            "malformed lines are skipped, not fatal"
        );
    }

    #[test]
    fn the_helper_answer_is_strictly_one_line() {
        assert_eq!(parse_handshake_age("42\n"), HandshakeAge::Seconds(42));
        assert_eq!(parse_handshake_age(" 42 \n"), HandshakeAge::Seconds(42));
        assert_eq!(parse_handshake_age("never\n"), HandshakeAge::Never);
        for bad in ["", "-1", "42\n7", "never 42", "1 2", "0x10", "unknown"] {
            assert_eq!(parse_handshake_age(bad), HandshakeAge::Unknown, "{bad}");
        }
    }

    #[test]
    fn the_dns_warning_is_picked_out_of_other_stderr() {
        let stderr = "some noise\n  jafnify-helper: dns routing: Access denied  \nmore\n";
        assert_eq!(dns_warning(stderr), Some("Access denied"));
        let route = "jafnify-route: dns routing: resolvectl domain jafn0 ~. failed\n";
        assert_eq!(
            dns_warning(route),
            Some("resolvectl domain jafn0 ~. failed")
        );
        assert_eq!(dns_warning("some noise\n"), None);
        assert_eq!(dns_warning(""), None);
    }

    #[test]
    fn the_ipv6_warning_is_picked_out_of_other_stderr() {
        let stderr = "some noise\njafnify-helper: ipv6 guard: ip -6 route replace failed\n";
        assert_eq!(
            ipv6_warning(stderr),
            Some("ip -6 route replace failed")
        );
        let route = "jafnify-route: ipv6 guard: no IPv6 route\n";
        assert_eq!(ipv6_warning(route), Some("no IPv6 route"));
        assert_eq!(ipv6_warning("some noise\n"), None);
        assert_eq!(ipv6_warning(""), None);
    }

    #[test]
    fn the_ipv6_warn_line_says_ipv6_can_leave_outside_the_tunnel() {
        assert_eq!(
            ipv6_warning_line("ip -6 route replace failed"),
            "IPv6 can leave outside the tunnel: ip -6 route replace failed"
        );
    }
}
