//! The tunnel's gateway address is outside 10.0.2.0/24, the default network of
//! QEMU/VirtualBox NAT and some hotels.
//! The sharer's router answers DNS and ping there, and the joiner's DNS line
//! points at it.

use std::net::Ipv4Addr;

use jafnify_core::config::ConfigGenerator;
use jafnify_core::{GATEWAY_DNS_IP, JOINER_IP, SHARER_IP};

const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

#[test]
fn the_gateway_address_is_not_in_the_common_nat_network() {
    let gw: Ipv4Addr = GATEWAY_DNS_IP.parse().unwrap();
    assert_eq!(gw, Ipv4Addr::new(10, 233, 0, 1));
    assert!(!gw.octets().starts_with(&[10, 0, 2]));
}

#[test]
fn the_tunnel_addresses_stay_in_their_own_network() {
    let sharer: Ipv4Addr = SHARER_IP.parse().unwrap();
    let joiner: Ipv4Addr = JOINER_IP.parse().unwrap();
    assert_eq!(sharer.octets()[..3], [10, 9, 9]);
    assert_eq!(joiner.octets()[..3], [10, 9, 9]);
}

#[test]
fn the_joiner_dns_line_points_at_the_gateway() {
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    assert_eq!(cfg.interface.dns.as_deref(), Some(GATEWAY_DNS_IP));
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert!(
        text.contains(&format!("\nDNS = {GATEWAY_DNS_IP}\n")),
        "{text}"
    );
}
