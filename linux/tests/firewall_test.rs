//! Host firewall handling.
//!
//! Inbound flows the laptop needs that conntrack does not cover, found by
//! reading the code, not by guessing:
//!   - 0x01 discovery, 0x02 key announcement and 0x03 Hello/Proof are sent
//!     BOTH to the multicast group 239.255.42.99:9799 and by unicast to the
//!     learned peer; the PAKE confirms are unicast too.
//!   - WireGuard replies (sharer:9797 -> the laptop's WireGuard socket) answer
//!     the laptop's own packets, so conntrack accepts them; the joiner never
//!     needs an inbound 9797 rule.
//!
//! So one rule covers a joiner: UDP on port 9799 arriving on the default-route
//! LAN interface, whatever the destination. A group-only rule would silently
//! drop the unicast copies. Scoping by interface (not the current subnet)
//! keeps the rule valid when a phone hotspot picks a new subnet.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_linux::firewall::{
    blocked_help, lan_subnet, nft_rule, rule_lines, ufw_allow_args, ufw_enabled, verdict,
    FirewallCheck, FirewallKind, Verdict, ECHO_GRACE,
};
use jafnify_linux::root::{ufw_rule_script, RootCommands};

// ---- the verdict -----------------------------------------------------------

#[test]
fn hearing_our_own_announcement_means_the_firewall_is_not_in_the_way() {
    assert_eq!(verdict(true, Duration::ZERO, false), Verdict::Open);
    assert_eq!(verdict(true, ECHO_GRACE * 10, true), Verdict::Open);
}

#[test]
fn a_peer_heard_on_any_path_means_the_firewall_is_not_blocking() {
    // The laptop's own announcements never come back (multicast blocked), but
    // the phone's frames arrive by unicast after the grace period.
    //
    // Do not use the product's 9799 here: a running jafnify-gui on the host
    // also binds it (SO_REUSEPORT), and the kernel then delivers this test's
    // unicast datagram to that socket instead.
    let port = 49800;
    let cfg = DiscoveryConfig {
        addr: format!("239.255.42.99:{port}").parse().unwrap(),
        iface: Some(Ipv4Addr::new(127, 0, 0, 2)),
        ..Default::default()
    };
    let mut check = FirewallCheck::start(&cfg, Instant::now(), true).expect("watch");
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.send_to(&[0x03, 0x01], format!("127.0.0.2:{port}"))
        .unwrap();
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        check.poll(Instant::now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(check.poll(Instant::now() + ECHO_GRACE * 10), Verdict::Open);
}

#[test]
fn silence_is_not_blocking_until_the_grace_period_is_over() {
    assert_eq!(verdict(false, Duration::ZERO, true), Verdict::Unknown);
    assert_eq!(
        verdict(false, ECHO_GRACE - Duration::from_millis(1), true),
        Verdict::Unknown
    );
}

#[test]
fn silence_after_the_grace_period_is_a_block_named_by_what_is_active() {
    assert_eq!(
        verdict(false, ECHO_GRACE, true),
        Verdict::Blocked(FirewallKind::Ufw)
    );
    assert_eq!(
        verdict(false, ECHO_GRACE, false),
        Verdict::Blocked(FirewallKind::Other)
    );
}

#[test]
fn the_grace_period_covers_at_least_two_announcements() {
    // Announcements go out once per second.
    assert!(ECHO_GRACE >= Duration::from_secs(3));
}

// ---- ufw state (readable without root) -------------------------------------

#[test]
fn ufw_enabled_reads_ufw_conf() {
    assert!(ufw_enabled("# comment\nENABLED=yes\nLOGLEVEL=low\n"));
    assert!(ufw_enabled("ENABLED=YES\n"));
    assert!(!ufw_enabled("ENABLED=no\n"));
    assert!(!ufw_enabled("# ENABLED=yes\n"));
    assert!(!ufw_enabled(""));
}

// ---- the narrow rule --------------------------------------------------------

#[test]
fn the_subnet_is_the_lan_network_not_the_host_address() {
    assert_eq!(
        lan_subnet(Ipv4Addr::new(10, 20, 89, 70), 24).as_deref(),
        Some("10.20.89.0/24")
    );
    assert_eq!(
        lan_subnet(Ipv4Addr::new(192, 168, 43, 77), 28).as_deref(),
        Some("192.168.43.64/28")
    );
}

#[test]
fn a_network_too_wide_or_a_single_host_gets_no_rule() {
    // Never open anything broader than a LAN.
    assert_eq!(lan_subnet(Ipv4Addr::new(10, 0, 0, 5), 8), None);
    assert_eq!(lan_subnet(Ipv4Addr::new(10, 0, 0, 5), 15), None);
    assert_eq!(lan_subnet(Ipv4Addr::new(10, 0, 0, 5), 31), None);
    assert_eq!(lan_subnet(Ipv4Addr::new(10, 0, 0, 5), 32), None);
    assert_eq!(lan_subnet(Ipv4Addr::new(10, 0, 0, 5), 0), None);
}

#[test]
fn the_ufw_rule_is_udp_the_port_and_the_interface_only() {
    assert_eq!(
        ufw_allow_args("wlo1"),
        [
            "allow",
            "in",
            "on",
            "wlo1",
            "to",
            "any",
            "port",
            "9799",
            "proto",
            "udp",
            "comment",
            "Jafnify discovery"
        ]
    );
}

// K2: the unicast copies of discovery and the PAKE confirms need the port
// open, not only the multicast group.
#[test]
fn the_rule_covers_unicast_arrivals_not_only_the_multicast_group() {
    let args = ufw_allow_args("wlo1");
    assert!(!args.iter().any(|a| a == "239.255.42.99"), "{args:?}");
    let rule = nft_rule("wlo1");
    assert!(!rule.contains("daddr"), "{rule}");
    assert!(!rule.contains("239.255.42.99"), "{rule}");
}

#[test]
fn the_rule_lines_use_the_interface_and_never_a_subnet() {
    assert_eq!(
        rule_lines("wlo1"),
        vec![
            "ufw: ufw allow in on wlo1 to any port 9799 proto udp comment 'Jafnify discovery'"
                .to_string(),
            "nft: iifname \"wlo1\" udp dport 9799 accept".to_string(),
        ]
    );
}

// The offer says how to take the rule back out, so the user is never stuck
// with it. The app never runs the
// delete itself.
#[test]
fn the_ufw_offer_shows_how_to_remove_the_interface_rule() {
    let text = blocked_help(FirewallKind::Ufw, Some("wlo1"));
    assert!(
        text.contains("ufw delete allow in on wlo1 to any port 9799 proto udp"),
        "{text}"
    );
    assert!(text.contains("on wlo1"), "{text}");
    assert!(!text.contains("10.20.89.0/24"), "{text}");
    assert!(!text.contains("239.255.42.99"), "{text}");
    assert!(!text.contains("from"), "{text}");
}

#[test]
fn the_nft_rule_scopes_by_the_interface() {
    assert_eq!(nft_rule("wlo1"), "iifname \"wlo1\" udp dport 9799 accept");
}

#[test]
fn the_root_command_is_a_fixed_pkexec_shell_script_with_the_values_as_arguments() {
    let root = RootCommands {
        pkexec_bin: PathBuf::from("/usr/bin/pkexec"),
        wg_quick_bin: PathBuf::from("/usr/bin/wg-quick"),
        wg_bin: PathBuf::from("/usr/bin/wg"),
        install_bin: PathBuf::from("/usr/bin/install"),
        ufw_bin: PathBuf::from("/tmp/jafnify-test/ufw"),
        helper_bin: PathBuf::from("/nonexistent/jafnify-helper"),
        route_bin: PathBuf::from("/nonexistent/jafnify-route"),
        interface: "jafn0".to_string(),
        teardown_conf: PathBuf::from("/tmp/jafnify-test/jafn0.conf"),
        teardown_iface: PathBuf::from("/tmp/jafnify-test/jafn0.iface"),
        teardown_rules: PathBuf::from("/tmp/jafnify-test/49-jafnify.rules"),
    };
    let cmd = root.ufw_allow_command("wlo1");
    assert_eq!(cmd.get_program(), "/usr/bin/pkexec");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "/bin/sh");
    assert_eq!(args[1], "-c");
    assert_eq!(args[2], ufw_rule_script());
    assert_eq!(args[3], "sh");
    assert_eq!(args[4], "/tmp/jafnify-test/ufw");
    assert_eq!(args[5], "wlo1");
    assert_eq!(args[6], "9799");
    assert_eq!(args[7], "Jafnify discovery");
    // The group is passed only so the script can recognise the old rule.
    assert_eq!(args[8], "239.255.42.99");
    // The interface is never spliced into shell code, and the rule is never
    // scoped by a source subnet.
    assert!(!args[2].contains("wlo1"), "{}", args[2]);
    assert!(!args.iter().any(|a| a.contains("from")), "{args:?}");
    // Uninstall removes every Jafnify discovery rule, both forms.
    let uninstall = root.uninstall_command();
    let script = uninstall
        .get_args()
        .last()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(script.contains("Jafnify discovery"), "{script}");
    assert!(script.contains("--force delete"), "{script}");
}
