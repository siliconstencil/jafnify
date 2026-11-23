//! A laptop whose default route is on another network (Ethernet, a second
//! Wi-Fi) must still find the phone. A sharer uses every up,
//! multicast-capable, non-loopback, non-tunnel IPv4 interface; a joiner uses
//! every up private LAN interface, with the default-route interface as the
//! fallback.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, Discovery, DiscoveryConfig};
use jafnify_linux::app::{parse_sharer_ifaces, AppBuilder};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

const ADDRS: &str = "\
1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever
2: enp3s0    inet 10.1.1.5/24 brd 10.1.1.255 scope global dynamic enp3s0\\       valid_lft 3000sec
3: wlo1    inet 10.20.89.70/24 brd 10.20.89.255 scope global dynamic noprefixroute wlo1\\       valid_lft 3000sec
3: wlo1    inet 192.168.5.9/24 scope global secondary wlo1\\       valid_lft forever
4: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\\       valid_lft forever
5: wg0    inet 10.2.0.2/32 scope global wg0\\       valid_lft forever
6: jafn0    inet 10.9.9.2/32 scope global jafn0\\       valid_lft forever
7: tun0    inet 10.8.0.2/24 scope global tun0\\       valid_lft forever
9: nomulti0    inet 10.5.5.5/24 scope global nomulti0\\       valid_lft forever
";

const LINKS: &str = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
2: enp3s0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP mode DEFAULT group default qlen 1000\\    link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff
3: wlo1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DORMANT group default qlen 1000\\    link/ether 52:54:00:12:34:57 brd ff:ff:ff:ff:ff:ff
4: docker0: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc noqueue state DOWN mode DEFAULT group default\\    link/ether 02:42:ac:11:00:01 brd ff:ff:ff:ff:ff:ff
5: wg0: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/none
6: jafn0: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1280 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 500\\    link/none
7: tun0: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UNKNOWN mode DEFAULT group default qlen 500\\    link/none
9: nomulti0: <BROADCAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 52:54:00:12:34:58 brd ff:ff:ff:ff:ff:ff
";

#[test]
fn a_sharer_uses_every_up_multicast_lan_interface() {
    assert_eq!(
        parse_sharer_ifaces(ADDRS, LINKS),
        vec![
            Ipv4Addr::new(10, 1, 1, 5),
            Ipv4Addr::new(10, 20, 89, 70),
            Ipv4Addr::new(192, 168, 5, 9),
        ]
    );
}

#[test]
fn an_interface_without_carrier_is_not_used() {
    // docker0 above is administratively UP but has NO-CARRIER (LOWER_UP is
    // missing): up means up and running, as Java's isUp() says on the phone.
    assert!(!parse_sharer_ifaces(ADDRS, LINKS).contains(&Ipv4Addr::new(172, 17, 0, 1)));
}

#[test]
fn nothing_usable_gives_an_empty_list() {
    assert!(parse_sharer_ifaces("", "").is_empty());
    assert!(parse_sharer_ifaces("1: lo    inet 127.0.0.1/8 scope host lo\n", LINKS).is_empty());
    // An interface that is not in the link list (down, or the call failed)
    // is not used.
    assert!(parse_sharer_ifaces(ADDRS, "").is_empty());
}

/// A fake `ip` that answers the read-only queries the App makes. The "LAN" is
/// 127.0.0.2 (on lo, so the test never touches a real interface) plus a
/// second interface address nothing owns.
fn fake_ip(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("fake-ip.sh");
    // Written under a temp name, then renamed: the script is never executed while
    // it is still being written (text file busy under parallel tests).
    let tmp = dir.join("fake-ip.sh.tmp");
    std::fs::write(
        &tmp,
        r#"#!/bin/sh
case "$*" in
  "-4 -o route show default") echo "default via 127.0.0.1 dev fake0 proto dhcp metric 100" ;;
  "-4 -o addr show dev fake0") echo "3: fake0    inet 127.0.0.2/8 scope host fake0" ;;
  "-4 -o addr show up")
    echo "2: fake1    inet 192.0.2.1/24 scope global fake1"
    echo "3: fake0    inet 127.0.0.2/8 scope host fake0" ;;
  "-o link show up")
    echo "2: fake1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP"
    echo "3: fake0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

fn listener() -> Discovery {
    Discovery::bind(&DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    })
    .unwrap()
}

fn token_heard(l: &Discovery) -> bool {
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        if let Some((_, t)) = l.try_receive(Duration::from_millis(100)).unwrap() {
            if t.verify() {
                return true;
            }
        }
    }
    false
}

#[test]
fn the_sharer_app_announces_through_the_list_not_only_the_default_route() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .ip_bin(fake_ip(dir.path()))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    app.set_role(Role::Sharer);
    // 192.0.2.1 cannot be joined: the sharer must carry on with 127.0.0.2.
    app.start("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    let l = listener();
    app.tick().unwrap();
    assert!(
        token_heard(&l),
        "no announcement left through the listed interface"
    );
}

#[test]
fn a_joiner_falls_back_when_no_private_address_qualifies() {
    // The list holds only 127.0.0.2 (loopback) and 192.0.2.1 (TEST-NET-1,
    // public), so the joiner's private-LAN filter is empty. The fallback asks
    // for the default-route interface, and that detection refuses loopback
    // addresses (existing rule), so it finds no LAN.
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .ip_bin(fake_ip(dir.path()))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    app.set_role(Role::Joiner);
    assert!(app.start("1234").is_err());
}
