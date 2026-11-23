//! What the window and the CLI say when the firewall is eating discovery.

use jafnify_core::ConnectionState;
use jafnify_linux::firewall::{rule_lines, FirewallKind, BLOCKED_STATUS};
use jafnify_linux::view::{view, Primary, ViewInput};

fn searching(kind: Option<FirewallKind>, iface: Option<&str>) -> ViewInput {
    ViewInput {
        firewall: kind,
        firewall_iface: iface.map(str::to_string),
        ..ViewInput::new(ConnectionState::Searching)
    }
}

#[test]
fn a_blocked_search_says_so_instead_of_searching_forever() {
    let v = view(&searching(Some(FirewallKind::Ufw), Some("wlo1")));
    assert_eq!(v.status, BLOCKED_STATUS);
    assert!(v.status.to_lowercase().contains("firewall"));
    assert!(v.warning);
    assert_eq!(v.primary, Primary::Cancel, "Cancel stays available");
    let help = v.help.as_deref().unwrap();
    assert!(
        help.contains("9799") && !help.contains("239.255.42.99"),
        "{help}"
    );
    assert!(help.contains("on wlo1"), "{help}");
    assert!(!help.contains("10.20.89.0/24"), "{help}");
    assert!(!help.contains("from"), "{help}");
}

#[test]
fn it_beats_the_no_device_found_hint() {
    let v = view(&ViewInput {
        no_peer: true,
        ..searching(Some(FirewallKind::Other), None)
    });
    assert_eq!(v.status, BLOCKED_STATUS);
}

#[test]
fn the_rule_is_offered_for_ufw_with_a_lan_interface_only() {
    assert!(view(&searching(Some(FirewallKind::Ufw), Some("wlo1"))).offer_rule);
    // Another firewall: the rule is shown, never applied.
    assert!(!view(&searching(Some(FirewallKind::Other), Some("wlo1"))).offer_rule);
    // No interface to scope it to: nothing to offer.
    assert!(!view(&searching(Some(FirewallKind::Ufw), None)).offer_rule);
}

#[test]
fn another_firewall_gets_the_rule_text_to_add_by_hand() {
    let v = view(&searching(Some(FirewallKind::Other), Some("wlo1")));
    assert!(v
        .help
        .unwrap()
        .contains("iifname \"wlo1\" udp dport 9799 accept"));
}

#[test]
fn an_open_search_looks_as_before() {
    let v = view(&searching(None, None));
    assert_eq!(v.status, "Searching for the other device");
    assert!(!v.warning);
    assert!(v.help.is_none());
    assert!(!v.offer_rule);
}

#[test]
fn the_firewall_only_matters_while_searching() {
    for state in [
        ConnectionState::Idle,
        ConnectionState::Pairing,
        ConnectionState::Connecting,
        ConnectionState::Connected,
    ] {
        let v = view(&ViewInput {
            firewall: Some(FirewallKind::Ufw),
            firewall_iface: Some("wlo1".to_string()),
            ..ViewInput::new(state)
        });
        assert_ne!(v.status, BLOCKED_STATUS, "{state:?}");
        assert!(v.help.is_none(), "{state:?}");
        assert!(!v.offer_rule, "{state:?}");
    }
}

#[test]
fn the_cli_prints_both_interface_rules_one_per_line() {
    assert_eq!(
        rule_lines("wlo1"),
        vec![
            "ufw: ufw allow in on wlo1 to any port 9799 proto udp comment 'Jafnify discovery'"
                .to_string(),
            "nft: iifname \"wlo1\" udp dport 9799 accept".to_string(),
        ]
    );
}
