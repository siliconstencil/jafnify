//! Discovery on several interfaces at once.
//! A phone acting as hotspot has a cellular default route plus the hotspot
//! LAN: unpinned multicast follows the default route and never reaches the
//! LAN. The caller therefore lists every interface to use; one that cannot be
//! used must not stop the others. The namespace lab (tools/lab/e2e-hotspot.sh)
//! proves the routing side; these tests prove the socket side without root.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::discovery::{Discovery, DiscoveryConfig, DiscoveryError};
use jafnify_core::handshake::Handshake;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{Frame, KnownSocket};
use jafnify_core::pairing::{DiscoveryToken, PairingCode};
use jafnify_core::pake::PakeLink;
use jafnify_core::state::Role;
use jafnify_core::LISTEN_PORT;

/// The tests share the multicast group on loopback: one at a time.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// TEST-NET-1 (RFC 5737): never a local address, so joining on it fails.
const UNUSABLE: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Not the product's 9799: a running app on this host also binds it.
const TEST_PORT: u16 = 49930;

fn loopback() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

/// The hotspot is NOT the first interface in the list.
fn unusable_first() -> DiscoveryConfig {
    DiscoveryConfig {
        ifaces: vec![UNUSABLE, Ipv4Addr::LOCALHOST],
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

fn code() -> PairingCode {
    PairingCode::parse("4321").unwrap()
}

#[test]
fn interfaces_lists_ifaces_and_falls_back_to_the_single_pin() {
    assert!(DiscoveryConfig::default().interfaces().is_empty());
    assert_eq!(loopback().interfaces(), vec![Ipv4Addr::LOCALHOST]);
    assert_eq!(
        unusable_first().interfaces(),
        vec![UNUSABLE, Ipv4Addr::LOCALHOST]
    );
    // A list replaces the single pin.
    let both = DiscoveryConfig {
        iface: Some(Ipv4Addr::new(10, 0, 0, 1)),
        ..unusable_first()
    };
    assert_eq!(both.interfaces(), vec![UNUSABLE, Ipv4Addr::LOCALHOST]);
}

#[test]
fn discovery_announces_and_hears_through_a_list_whose_first_entry_is_unusable() {
    let _g = serial();
    let tx = Discovery::bind(&unusable_first()).expect("bind tx");
    let rx = Discovery::bind(&loopback()).expect("bind rx");
    let token = DiscoveryToken::derive(&[7u8; 16]);
    tx.announce(&token)
        .expect("announce must reach the usable interface");
    let (_, got) = rx
        .try_receive(Duration::from_secs(3))
        .unwrap()
        .expect("no frame: the usable interface was not used");
    assert_eq!(got, token);

    // The list side also hears.
    let other = Discovery::bind(&loopback()).unwrap();
    other.announce(&token).unwrap();
    let (_, got) = tx
        .try_receive(Duration::from_secs(3))
        .unwrap()
        .expect("the listed socket heard nothing");
    assert_eq!(got, token);
}

/// The PAKE Hello goes out through a list whose first entry is unusable.
///
/// The legacy 0x04 announce is gone (its frame is verifiable with the code
/// alone, P0-1), so the pairing path that replaces it proves the egress list
/// instead.
#[test]
fn the_pake_hello_goes_through_a_list_whose_first_entry_is_unusable() {
    let _g = serial();
    let mut tx = PakeLink::open(
        &unusable_first(),
        Role::Sharer,
        &code(),
        &KeyPair::generate(),
        LISTEN_PORT,
    )
    .expect("bind tx");
    let mut rx = PakeLink::open(&loopback(), Role::Joiner, &code(), &KeyPair::generate(), 0)
        .expect("bind rx");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && rx.hellos_seen() == 0 {
        tx.poll(Instant::now()).expect("poll tx");
        rx.poll(Instant::now()).expect("poll rx");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        rx.hellos_seen() > 0,
        "no Hello: the usable interface was not used"
    );
}

#[test]
fn known_socket_sends_through_a_list_whose_first_entry_is_unusable() {
    let _g = serial();
    let tx = KnownSocket::bind(&unusable_first()).expect("bind tx");
    let rx = KnownSocket::bind(&loopback()).expect("bind rx");
    let own = KeyPair::generate();
    let peer = KeyPair::generate();
    let session =
        jafnify_core::known::KnownSession::new(&own, &peer.public_key_b64(), Role::Sharer).unwrap();
    tx.send(&session.hello().encode()).expect("send");
    let (_, frame) = rx
        .receive(Duration::from_secs(3))
        .unwrap()
        .expect("no 0x03 frame");
    assert!(matches!(frame, Frame::Hello(_)));
}

#[test]
fn a_list_of_only_unusable_interfaces_is_an_error() {
    let cfg = DiscoveryConfig {
        ifaces: vec![UNUSABLE],
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    };
    assert!(matches!(
        Discovery::bind(&cfg),
        Err(DiscoveryError::Io(_)) | Err(DiscoveryError::JoinMulticast(_))
    ));
    assert!(Handshake::bind(&cfg).is_err());
    assert!(KnownSocket::bind(&cfg).is_err());
}

#[test]
fn two_addresses_on_one_link_are_not_an_error() {
    let _g = serial();
    // 127.0.0.1 and 127.0.0.2 are both on lo: the second join of the group on
    // the same link must not fail the bind.
    let cfg = DiscoveryConfig {
        ifaces: vec![Ipv4Addr::LOCALHOST, Ipv4Addr::new(127, 0, 0, 2)],
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    };
    let tx = Discovery::bind(&cfg).expect("bind");
    let rx = Discovery::bind(&loopback()).unwrap();
    let token = DiscoveryToken::derive(&[3u8; 16]);
    tx.announce(&token).unwrap();
    assert!(rx.try_receive(Duration::from_secs(3)).unwrap().is_some());
}
