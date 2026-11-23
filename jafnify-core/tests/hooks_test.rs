//! Laptop hooks while the tunnel is up:
//!   DNS: with systemd-resolved, every domain (`~.`) resolves through jafn0.
//!   IPv6: the IPv6 blackhole is NOT a hook anymore. The root helper applies
//!      it: wg-quick runs its hooks under an AppArmor profile that denies
//!      reading /proc/sys/net/ipv6, and on a kernel booted with ipv6.disable=1
//!      `ip -6 route replace` fails, which under wg-quick's `set -e` would stop
//!      the tunnel from coming up.
//! The inbound guard is the first command of the same PostUp line.

use jafnify_core::config::{ConfigGenerator, InboundGuard};

const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

fn joiner(guard: InboundGuard, resolved: bool) -> jafnify_core::WireGuardConfig {
    ConfigGenerator::joiner_config_with_hooks(PRIV, PUB, "192.168.43.1:9797", guard, resolved)
}

#[test]
fn the_ipv6_blackhole_is_not_a_hook_anymore() {
    let cfg = joiner(InboundGuard::Iptables, true);
    let text = cfg.interface.post_up.unwrap() + &cfg.interface.post_down.unwrap();
    assert!(!text.contains("ip -6"), "{text}");
    assert!(!text.contains("/proc/sys/net/ipv6"), "{text}");
    assert!(!text.contains("disable_ipv6"), "{text}");
    assert!(!text.contains("cat "), "{text}");
}

#[test]
fn the_ipv6_blackhole_is_not_a_hook_without_systemd_resolved_either() {
    let cfg = joiner(InboundGuard::None, false);
    let text = cfg.interface.post_up.unwrap() + &cfg.interface.post_down.unwrap();
    assert!(!text.contains("ip -6"), "{text}");
    assert!(!text.contains("blackhole"), "{text}");
}

#[test]
fn with_systemd_resolved_every_domain_goes_through_the_tunnel() {
    let cfg = joiner(InboundGuard::None, true);
    let up = cfg.interface.post_up.unwrap();
    let down = cfg.interface.post_down.unwrap();
    assert!(up.contains("resolvectl domain %i ~."), "{up}");
    // `~.` is a catch-all routing domain, so jafn0 is already the resolver's
    // default. A separate default-route call is denied by wg-quick's Ubuntu
    // AppArmor profile, so it is not issued here; the root helper applies it
    // again outside the profile.
    assert!(!up.contains("resolvectl default-route"), "{up}");
    assert!(down.contains("resolvectl revert %i"), "{down}");
}

#[test]
fn without_systemd_resolved_no_resolvectl_is_run() {
    let cfg = joiner(InboundGuard::Iptables, false);
    let text = cfg.interface.post_up.unwrap() + &cfg.interface.post_down.unwrap();
    assert!(!text.contains("resolvectl"), "{text}");
}

#[test]
fn the_guard_stays_the_first_command_of_postup() {
    let cfg = joiner(InboundGuard::Iptables, true);
    let up = cfg.interface.post_up.unwrap();
    assert!(up.starts_with("iptables -I INPUT 1 -i %i"), "{up}");
}

#[test]
fn the_hooks_config_round_trips() {
    let cfg = joiner(InboundGuard::Nft, true);
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert_eq!(ConfigGenerator::parse(&text).unwrap(), cfg);
}

#[test]
fn the_sharer_has_no_hooks_at_all() {
    let cfg = ConfigGenerator::sharer_config(PRIV, PUB);
    assert_eq!(cfg.interface.post_up, None);
    assert_eq!(cfg.interface.post_down, None);
}
