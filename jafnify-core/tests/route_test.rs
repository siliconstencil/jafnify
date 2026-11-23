//! Default gateways for unicast discovery.
//! Fixture: `/proc/net/route` as the kernel prints it (gateway = the raw u32 in
//! host byte order, shown in hex, so 10.89.62.120 is "783E590A" on little
//! endian).

use std::net::Ipv4Addr;

use jafnify_core::probe::LocalIface;
use jafnify_core::route::{gateways_for, joiner_search_ifaces};

const TABLE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlo1\t00000000\t783E590A\t0003\t0\t0\t600\t00000000\t0\t0\t0
wlo1\t0040590A\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
wg0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0
enp3s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
";

#[test]
fn default_route_gateway_of_each_listed_interface() {
    assert_eq!(
        gateways_for(TABLE, &["wlo1"]),
        vec![Ipv4Addr::new(10, 89, 62, 120)]
    );
}

#[test]
fn interfaces_are_matched_by_name_and_only_default_routes_count() {
    // wg0 has a default with no gateway (a connected tunnel): not a target.
    // The subnet route of wlo1 (no gateway, mask /24) is not a default route.
    assert!(gateways_for(TABLE, &["wg0"]).is_empty());
    assert_eq!(
        gateways_for(TABLE, &["wlo1", "enp3s0"]),
        vec![
            Ipv4Addr::new(10, 89, 62, 120),
            Ipv4Addr::new(192, 168, 1, 1)
        ]
    );
    assert!(gateways_for(TABLE, &["nope0"]).is_empty());
}

fn iface(name: &str, ip: &str, prefix: u8) -> LocalIface {
    LocalIface {
        name: name.to_string(),
        ip: ip.parse().unwrap(),
        prefix,
    }
}

fn mgmt() -> LocalIface {
    iface("enp0s2", "10.0.2.15", 24)
}

fn hotspot() -> LocalIface {
    iface("hot0", "192.168.77.2", 24)
}

#[test]
fn the_saved_peers_subnet_is_searched_first() {
    let peer: Ipv4Addr = "192.168.77.1".parse().unwrap();
    // `ip` order puts the management NIC first; the choice follows the peer.
    assert_eq!(
        joiner_search_ifaces(&[mgmt(), hotspot()], Some(peer)),
        vec![Ipv4Addr::new(192, 168, 77, 2), Ipv4Addr::new(10, 0, 2, 15)]
    );
    // No hint: the `ip` order is kept.
    assert_eq!(
        joiner_search_ifaces(&[mgmt(), hotspot()], None),
        vec![Ipv4Addr::new(10, 0, 2, 15), Ipv4Addr::new(192, 168, 77, 2)]
    );
}

#[test]
fn a_stale_peer_hint_keeps_every_network_in_the_list() {
    let peer: Ipv4Addr = "192.168.99.1".parse().unwrap();
    // The hint matches nothing: every usable address is still searched, so a
    // hotspot that reappears with a new address is found. The hint only orders.
    assert_eq!(
        joiner_search_ifaces(&[mgmt(), hotspot()], Some(peer)),
        vec![Ipv4Addr::new(10, 0, 2, 15), Ipv4Addr::new(192, 168, 77, 2)]
    );
    // While the hotspot is away only the other network exists; the peer hint
    // must not make the list empty.
    assert_eq!(
        joiner_search_ifaces(&[mgmt()], Some(peer)),
        vec![Ipv4Addr::new(10, 0, 2, 15)]
    );
}

#[test]
fn the_hotspot_address_comes_back_to_the_front_when_the_link_returns() {
    let peer: Ipv4Addr = "192.168.77.1".parse().unwrap();
    // The list the search uses while the hotspot is down.
    let away = joiner_search_ifaces(&[mgmt()], Some(peer));
    // The list it must follow once the address is back: a different list, so
    // the caller reopens the search instead of staying on the old interface.
    let back = joiner_search_ifaces(&[mgmt(), hotspot()], Some(peer));
    assert_ne!(away, back, "the search would never re-evaluate");
    assert_eq!(back.first(), Some(&Ipv4Addr::new(192, 168, 77, 2)));
}

#[test]
fn container_bridges_and_public_addresses_are_not_searched() {
    let all = vec![
        iface("enp3s0", "203.0.113.5", 24),
        iface("docker0", "172.17.0.1", 16),
        iface("br-1234", "172.18.0.1", 16),
        iface("veth9", "10.9.0.1", 24),
        iface("virbr0", "192.168.122.1", 24),
        hotspot(),
    ];
    assert_eq!(
        joiner_search_ifaces(&all, Some("192.168.77.1".parse().unwrap())),
        vec![Ipv4Addr::new(192, 168, 77, 2)]
    );
}
