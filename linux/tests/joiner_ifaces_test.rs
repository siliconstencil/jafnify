//! A laptop whose default route is on another network (Ethernet, a second
//! Wi-Fi) must still find the phone: the joiner searches every up, private,
//! non-tunnel, non-container interface, not only the default-route one
//! These tests use fixtures and a fake `ip`; they never open a
//! socket.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jafnify_core::probe::LocalIface;
use jafnify_linux::app::{detect_joiner_ifaces, detect_joiner_search_ifaces, joiner_ifaces};

fn iface(name: &str, ip: &str, prefix: u8) -> LocalIface {
    LocalIface {
        name: name.to_string(),
        ip: ip.parse().unwrap(),
        prefix,
    }
}

#[test]
fn a_joiner_searches_every_private_lan_address() {
    let all = vec![
        iface("enp3s0", "10.1.1.5", 24),
        iface("wlo1", "192.168.5.9", 24),
    ];
    assert_eq!(
        joiner_ifaces(&all),
        vec![Ipv4Addr::new(10, 1, 1, 5), Ipv4Addr::new(192, 168, 5, 9),]
    );
}

#[test]
fn a_joiner_skips_public_link_local_and_container_addresses() {
    let all = vec![
        iface("enp3s0", "203.0.113.5", 24),
        iface("link0", "169.254.10.10", 16),
        iface("docker0", "172.17.0.1", 16),
        iface("br-1234", "172.18.0.1", 16),
        iface("veth9", "10.9.0.1", 24),
        iface("virbr0", "192.168.122.1", 24),
        iface("wlo1", "192.168.5.9", 24),
    ];
    assert_eq!(joiner_ifaces(&all), vec![Ipv4Addr::new(192, 168, 5, 9)]);
}

fn fake_ip(dir: &Path) -> PathBuf {
    let p = dir.join("fake-ip.sh");
    let tmp = dir.join("fake-ip.sh.tmp");
    std::fs::write(
        &tmp,
        r#"#!/bin/sh
case "$*" in
  "-4 -o addr show up")
    echo "2: enp3s0    inet 10.1.1.5/24 brd 10.1.1.255 scope global enp3s0"
    echo "3: wlo1    inet 192.168.5.9/24 brd 192.168.5.255 scope global wlo1"
    echo "4: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0"
    echo "5: eth1    inet 203.0.113.5/24 brd 203.0.113.255 scope global eth1"
    echo "6: link0    inet 169.254.10.10/16 brd 169.254.255.255 scope link link0" ;;
  "-o link show up")
    echo "2: enp3s0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP"
    echo "3: wlo1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP"
    echo "4: docker0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP"
    echo "5: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP"
    echo "6: link0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

#[test]
fn detect_joiner_ifaces_uses_the_same_filter_on_ip_output() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        detect_joiner_ifaces(&fake_ip(dir.path())),
        vec![Ipv4Addr::new(10, 1, 1, 5), Ipv4Addr::new(192, 168, 5, 9),]
    );
}

#[test]
fn the_saved_peers_network_is_searched_first_on_the_same_ip_output() {
    let dir = tempfile::tempdir().unwrap();
    let ip = fake_ip(dir.path());
    assert_eq!(
        detect_joiner_search_ifaces(&ip, Some(Ipv4Addr::new(192, 168, 5, 1))),
        vec![Ipv4Addr::new(192, 168, 5, 9), Ipv4Addr::new(10, 1, 1, 5)]
    );
    assert_eq!(
        detect_joiner_search_ifaces(&ip, None),
        vec![Ipv4Addr::new(10, 1, 1, 5), Ipv4Addr::new(192, 168, 5, 9)]
    );
}
