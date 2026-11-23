//! The laptop's inbound guard is picked when the config is made: iptables if
//! present, else nft with its own table, else no guard (the tunnel still comes
//! up, and the caller logs a warning). A PostUp that cannot run would abort
//! wg-quick and the whole tunnel.

use jafnify_core::config::{ConfigGenerator, InboundGuard};

const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

#[test]
fn iptables_is_the_guard_when_it_is_installed() {
    assert_eq!(InboundGuard::choose(true, true), InboundGuard::Iptables);
    assert_eq!(InboundGuard::choose(true, false), InboundGuard::Iptables);
}

#[test]
fn nft_is_the_guard_when_only_nft_is_installed() {
    assert_eq!(InboundGuard::choose(false, true), InboundGuard::Nft);
}

#[test]
fn there_is_no_guard_when_neither_is_installed() {
    assert_eq!(InboundGuard::choose(false, false), InboundGuard::None);
}

#[test]
fn no_guard_means_no_firewall_hook_in_the_config() {
    let cfg = ConfigGenerator::joiner_config_with_guard(
        PRIV,
        PUB,
        "192.168.43.1:9797",
        InboundGuard::None,
    );
    let up = cfg.interface.post_up.unwrap();
    let down = cfg.interface.post_down.unwrap();
    assert!(!up.contains("iptables") && !up.contains("nft"), "{up}");
    assert!(
        !down.contains("iptables") && !down.contains("nft"),
        "{down}"
    );
}

#[test]
fn the_nft_guard_uses_its_own_table_and_is_removed_with_it() {
    let cfg = ConfigGenerator::joiner_config_with_guard(
        PRIV,
        PUB,
        "192.168.43.1:9797",
        InboundGuard::Nft,
    );
    let up = cfg.interface.post_up.as_deref().unwrap();
    let down = cfg.interface.post_down.as_deref().unwrap();
    assert!(up.contains("nft add table inet jafnify_guard"), "{up}");
    assert!(up.contains("iifname %i ct state new drop"), "{up}");
    assert!(
        down.contains("nft delete table inet jafnify_guard"),
        "{down}"
    );
    // Only the guard's own table is touched, never the firewall's.
    assert!(!up.contains("flush ruleset") && !down.contains("flush ruleset"));
}

#[test]
fn the_nft_guard_config_round_trips() {
    let cfg = ConfigGenerator::joiner_config_with_guard(
        PRIV,
        PUB,
        "192.168.43.1:9797",
        InboundGuard::Nft,
    );
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert_eq!(ConfigGenerator::parse(&text).unwrap(), cfg);
}
