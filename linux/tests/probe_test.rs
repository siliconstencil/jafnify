//! `jafnify probe`: the laptop side of the discovery probe. The verdict lines
//! are pure; the run uses a fake `ip` that offers 127.0.0.2 as the LAN (on lo),
//! so no real interface is ever touched.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::{run_probe, LocalIface, ProbeConfig};
use jafnify_core::state::Role;
use jafnify_linux::firewall::ECHO_GRACE;
use jafnify_linux::probe::{firewall_lines, firewall_report, run};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- the firewall verdict as the probe prints it -----------------------------

#[test]
fn hearing_ourselves_is_an_open_firewall() {
    assert_eq!(
        firewall_lines(Some(true), Duration::from_secs(1), false, None),
        vec!["probe: firewall: open"]
    );
}

#[test]
fn silence_after_the_grace_period_is_blocked_and_says_how_to_fix_it() {
    let lines = firewall_lines(Some(false), ECHO_GRACE, true, Some("wlo1".to_string()));
    assert_eq!(lines[0], "probe: firewall: blocked (ufw)");
    let help = lines[1..].join("\n");
    assert!(
        // Batch K: the rule is port-scoped (the group-only rule dropped the
        // unicast copies), so the help names the port, not the group.
        help.contains("UDP 9799") && help.contains("on wlo1") && !help.contains("239.255.42.99"),
        "{help}"
    );
    assert!(!help.contains("10.20.89.0/24"), "{help}");
    assert!(!help.contains("from"), "{help}");
    assert!(lines.iter().all(|l| l.starts_with("probe: ")));

    let other = firewall_lines(Some(false), ECHO_GRACE, false, None);
    assert_eq!(other[0], "probe: firewall: blocked (other)");
}

#[test]
fn a_too_short_listen_or_no_own_address_gives_no_verdict() {
    let short = firewall_lines(Some(false), Duration::from_secs(1), true, None);
    assert_eq!(
        short,
        vec!["probe: firewall: unknown (listened too briefly)"]
    );
    let none = firewall_lines(None, Duration::from_secs(30), true, None);
    assert_eq!(none, vec!["probe: firewall: unknown"]);
}

#[test]
fn a_peer_heard_by_unicast_makes_the_firewall_line_not_needed() {
    // Discovery worked without multicast: no firewall rule is part of it.
    let lines = firewall_report(true, Some(false), ECHO_GRACE, true, Some("wlo1".into()));
    assert_eq!(
        lines,
        vec!["probe: firewall: not needed (peer heard by unicast)"]
    );
    // Without a unicast peer the verdict is unchanged.
    assert_eq!(
        firewall_report(false, Some(false), ECHO_GRACE, false, None),
        firewall_lines(Some(false), ECHO_GRACE, false, None)
    );
    // Own echo back (multicast passes): say open, even if unicast also arrived.
    assert_eq!(
        firewall_report(true, Some(true), ECHO_GRACE, true, None),
        firewall_lines(Some(true), ECHO_GRACE, true, None)
    );
}

// ---- a run against a fake LAN -----------------------------------------------------

fn fake_ip(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("fake-ip.sh");
    // Written under a temp name, then renamed: the script is never executed while
    // it is still being written (text file busy under parallel tests).
    let tmp = dir.join("fake-ip.sh.tmp");
    std::fs::write(
        &tmp,
        r#"#!/bin/sh
case "$*" in
  "-4 -o addr show up") echo "3: fake0    inet 127.0.0.2/8 scope host fake0" ;;
  "-o link show up") echo "3: fake0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP" ;;
  "-4 -o route show default") echo "default via 127.0.0.1 dev fake0" ;;
  "-4 -o addr show dev fake0") echo "3: fake0    inet 127.0.0.2/8 scope host fake0" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

fn phone() -> ProbeConfig {
    ProbeConfig {
        code: PairingCode::parse("4321").unwrap(),
        role: Role::Sharer,
        ifaces: vec![LocalIface {
            name: "fake0".into(),
            ip: "127.0.0.2".parse().unwrap(),
            prefix: 8,
        }],
        window: Duration::from_secs(10),
        linger: Duration::from_millis(1500),
        unicast: Vec::new(),
    }
}

#[test]
fn the_laptop_probe_and_a_phone_probe_complete_the_key_exchange_both_ways() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let ip = fake_ip(dir.path());
    let ufw_conf = dir.path().join("ufw.conf"); // missing: ufw off
    let phone = std::thread::spawn(|| run_probe(&phone(), |_| {}).unwrap());
    let mut live = Vec::new();
    let laptop = run(&ip, "4321", 10, &ufw_conf, |l| live.push(l)).unwrap();
    let phone = phone.join().unwrap();

    assert!(laptop.ok, "{:#?}", laptop.lines);
    assert!(phone.key_exchange_done());
    let text = laptop.lines.join("\n");
    assert!(
        text.contains("probe: seen 0x06 from 127.0.0.2 via fake0"),
        "{text}"
    );
    assert!(text.contains("probe: key exchange: ok"), "{text}");
    assert!(text.contains("probe: firewall: open"), "{text}");
    assert!(text.ends_with("probe: result: ok"), "{text}");
    assert!(
        live.iter()
            .any(|l| l.contains("0x01") && l.contains("127.0.0.2")),
        "no live line while it ran: {live:?}"
    );
}

#[test]
fn a_laptop_probe_nobody_answers_fails_and_says_so() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let ip = fake_ip(dir.path());
    let out = run(&ip, "4321", 3, &dir.path().join("ufw.conf"), |_| {}).unwrap();
    assert!(!out.ok);
    let text = out.lines.join("\n");
    assert!(
        text.contains("probe: key exchange: not completed"),
        "{text}"
    );
    assert!(text.ends_with("probe: result: failed"), "{text}");
}

#[test]
fn a_bad_code_or_no_lan_is_an_error_not_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let ip = fake_ip(dir.path());
    let conf = dir.path().join("ufw.conf");
    assert!(run(&ip, "12ab", 3, &conf, |_| {})
        .unwrap_err()
        .contains("Invalid code"));
    let err = run(&PathBuf::from("/nonexistent/ip"), "4321", 3, &conf, |_| {}).unwrap_err();
    assert!(err.contains("no LAN interface found"), "{err}");
}
