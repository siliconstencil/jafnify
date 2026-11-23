//! The uplink watch: carrier/address transitions and route parsing. The
//! carrier is read from an injected temp sysfs tree; the IPv4 check uses
//! `getifaddrs`, so no `ip` process runs. The host's real Wi-Fi is never read.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use jafnify_linux::uplink::{
    parse_operstate, parse_route_dev, parse_route_src, parse_uplink_route, read_uplink_state_with,
    state_from, usable_ipv4, LinkChange, LinkWatch,
};

fn sysfs_with(dir: &Path, iface: &str, carrier: Option<&str>) -> PathBuf {
    let root = dir.join("net");
    let iface_dir = root.join(iface);
    fs::create_dir_all(&iface_dir).unwrap();
    if let Some(carrier) = carrier {
        fs::write(iface_dir.join("carrier"), carrier).unwrap();
    }
    root
}

#[test]
fn a_down_uplink_reports_once_and_an_up_reports_back() {
    let mut w = LinkWatch::new("wlo1".into());
    assert_eq!(w.observe(true), None);
    assert_eq!(w.observe(false), Some(LinkChange::Lost));
    assert!(w.lost());
    assert_eq!(w.observe(false), None);
    assert_eq!(w.observe(true), Some(LinkChange::Back));
    assert!(!w.lost());
    assert_eq!(w.observe(true), None);
}

#[test]
fn an_uplink_that_starts_down_reports_lost_immediately() {
    let mut w = LinkWatch::new("wlo1".into());
    assert_eq!(w.observe(false), Some(LinkChange::Lost));
    assert!(w.lost());
}

#[test]
fn an_uplink_that_starts_up_reports_nothing() {
    let mut w = LinkWatch::new("wlo1".into());
    assert_eq!(w.observe(true), None);
    assert!(!w.lost());
}

#[test]
fn the_interface_name_is_preserved() {
    let w = LinkWatch::new("wlo1".into());
    assert_eq!(w.iface(), "wlo1");
}

#[test]
fn route_dev_is_the_token_after_dev() {
    assert_eq!(
        parse_route_dev("192.168.43.1 dev wlo1 src 192.168.43.23 uid 1000\n"),
        Some("wlo1".into())
    );
    assert_eq!(
        parse_route_dev("10.9.9.1 dev enp0s3 src 10.0.0.2\n    cache\n"),
        Some("enp0s3".into())
    );
    assert_eq!(parse_route_dev("unreachable\n"), None);
    assert_eq!(parse_route_dev(""), None);
}

#[test]
fn route_src_is_the_token_after_src() {
    assert_eq!(
        parse_route_src("1.2.3.4 dev wlo1 src 10.0.0.5 uid 1000\n"),
        Some("10.0.0.5".parse().unwrap())
    );
    assert_eq!(
        parse_route_src("10.9.9.1 dev enp0s3 src 192.168.1.23 uid 1000\n    cache\n"),
        Some("192.168.1.23".parse().unwrap())
    );
    assert_eq!(parse_route_src("unreachable\n"), None);
    assert_eq!(parse_route_src("1.2.3.4 dev wlo1 src not-an-ip\n"), None);
    assert_eq!(parse_route_src(""), None);
}

#[test]
fn operstate_is_the_first_sysfs_word() {
    assert_eq!(parse_operstate("up\n"), Some("up"));
    assert_eq!(parse_operstate("down\n"), Some("down"));
    assert_eq!(parse_operstate("unknown\n"), Some("unknown"));
    assert_eq!(parse_operstate("dormant  \n"), Some("dormant"));
    assert_eq!(parse_operstate("   \n"), None);
    assert_eq!(parse_operstate(""), None);
}

#[test]
fn the_uplink_route_rejects_loopback_and_tunnel_devices() {
    assert_eq!(
        parse_uplink_route("192.168.43.1 dev wlo1 src 192.168.43.23 uid 1000\n"),
        Some("wlo1".into())
    );
    for rejected in ["jafn0", "wg0", "tun0", "lo"] {
        let line = format!("10.1.0.2 dev {rejected} src 10.0.0.2 uid 1000\n");
        assert_eq!(parse_uplink_route(&line), None, "accepted {rejected}");
    }
    assert_eq!(parse_uplink_route("unreachable\n"), None);
}

#[test]
fn usable_ipv4_rejects_loopback_and_unspecified_addresses() {
    assert!(usable_ipv4(Ipv4Addr::new(192, 168, 1, 5)));
    assert!(!usable_ipv4(Ipv4Addr::LOCALHOST));
    assert!(!usable_ipv4(Ipv4Addr::UNSPECIFIED));
}

#[test]
fn state_from_maps_carrier_and_ipv4() {
    assert_eq!(state_from(None, false), None);
    assert_eq!(state_from(None, true), None);
    assert_eq!(state_from(Some(false), false), Some(false));
    assert_eq!(state_from(Some(false), true), Some(false));
    assert_eq!(state_from(Some(true), false), Some(false));
    assert_eq!(state_from(Some(true), true), Some(true));
}

#[test]
fn a_down_carrier_is_down() {
    let dir = tempfile::tempdir().unwrap();
    let sysfs = sysfs_with(dir.path(), "wlo1", Some("0\n"));
    assert_eq!(read_uplink_state_with("wlo1", &sysfs), Some(false));
}

#[test]
fn a_missing_carrier_file_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let sysfs = sysfs_with(dir.path(), "wlo1", None);
    assert_eq!(read_uplink_state_with("wlo1", &sysfs), None);
}

#[test]
fn a_carrier_up_interface_without_ipv4_is_down() {
    // A name that cannot exist on the host has no address. This exercises the
    // carrier=1 path without running `ip`.
    let dir = tempfile::tempdir().unwrap();
    let iface = "jafnify-test-no-such0";
    let sysfs = sysfs_with(dir.path(), iface, Some("1\n"));
    assert_eq!(read_uplink_state_with(iface, &sysfs), Some(false));
}
