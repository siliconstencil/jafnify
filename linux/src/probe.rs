//! `jafnify probe`: the laptop side of the discovery probe. Runs the core
//! probe (0x01 discovery + 0x02 key exchange, no tunnel, never port 9797) on
//! the LAN interfaces and adds the host-firewall verdict.

use std::path::Path;
use std::time::Duration;

use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::{run_probe, ProbeConfig};

use crate::app::detect_lan_ifaces;
use crate::firewall::{blocked_help, ufw_enabled, verdict, FirewallKind, Verdict};

#[derive(Debug)]
pub struct ProbeOutcome {
    /// The key exchange completed (exit code 0).
    pub ok: bool,
    /// The full report, every line starting "probe: ".
    pub lines: Vec<String>,
}

/// The firewall part of the report. `echo`: our own announcements came back
/// (None: nothing to judge by). `listened`: how long the probe announced.
pub fn firewall_lines(
    echo: Option<bool>,
    listened: Duration,
    ufw: bool,
    iface: Option<String>,
) -> Vec<String> {
    let Some(heard) = echo else {
        return vec!["probe: firewall: unknown".to_string()];
    };
    match verdict(heard, listened, ufw) {
        Verdict::Open => vec!["probe: firewall: open".to_string()],
        Verdict::Unknown => {
            vec!["probe: firewall: unknown (listened too briefly)".to_string()]
        }
        Verdict::Blocked(kind) => {
            let name = match kind {
                FirewallKind::Ufw => "ufw",
                FirewallKind::Other => "other",
            };
            let mut lines = vec![format!("probe: firewall: blocked ({name})")];
            lines.push(format!("probe: {}", blocked_help(kind, iface.as_deref())));
            lines
        }
    }
}

/// The firewall part of the report, once the peer's unicast is taken into
/// account: a peer heard by unicast means no firewall rule is part of the
/// discovery. When our own echo came back the firewall is open anyway, and
/// that verdict is the more precise one.
pub fn firewall_report(
    heard_by_unicast: bool,
    echo: Option<bool>,
    listened: Duration,
    ufw: bool,
    iface: Option<String>,
) -> Vec<String> {
    if heard_by_unicast && echo != Some(true) {
        return vec!["probe: firewall: not needed (peer heard by unicast)".to_string()];
    }
    firewall_lines(echo, listened, ufw, iface)
}

/// Run the probe for at most `seconds`. `live` gets a line for every first
/// sighting while it runs. Err: nothing could be run (bad code, no LAN).
pub fn run(
    ip_bin: &Path,
    code: &str,
    seconds: u64,
    ufw_conf: &Path,
    mut live: impl FnMut(String),
) -> Result<ProbeOutcome, String> {
    let code = PairingCode::parse(code)
        .ok_or_else(|| "Invalid code - enter the 4 digits shown on the phone".to_string())?;
    let ifaces = detect_lan_ifaces(ip_bin);
    if ifaces.is_empty() {
        return Err(
            "no LAN interface found - connect to a network (tunnel and loopback interfaces are ignored)"
                .to_string(),
        );
    }
    let iface = ifaces.first().map(|i| i.name.clone());
    let window = Duration::from_secs(seconds);
    // The laptop's default gateway on each LAN interface (a hotspot's phone).
    let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
    let routes = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let cfg = ProbeConfig {
        code,
        role: jafnify_core::state::Role::Joiner,
        ifaces: ifaces.clone(),
        window,
        linger: Duration::from_secs(2),
        unicast: jafnify_core::route::gateways_for(&routes, &names),
    };
    let started = std::time::Instant::now();
    let report = run_probe(&cfg, |s| {
        live(format!(
            "probe: first {} frame from {}{}",
            s.kind.label(),
            s.from,
            if s.verified { "" } else { " (other code)" }
        ))
    })
    .map_err(|e| e.to_string())?;
    let ufw = std::fs::read_to_string(ufw_conf).is_ok_and(|c| ufw_enabled(&c));
    let mut lines = report.lines();
    lines.extend(firewall_report(
        report.peer_heard_by_unicast(),
        report.echo,
        started.elapsed(),
        ufw,
        iface,
    ));
    let ok = report.key_exchange_done();
    lines.push(format!(
        "probe: result: {}",
        if ok { "ok" } else { "failed" }
    ));
    Ok(ProbeOutcome { ok, lines })
}
