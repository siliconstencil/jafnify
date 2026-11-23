use std::time::Duration;

use jafnify_core::discovery::{Discovery, DiscoveryConfig, DiscoveryError};
use jafnify_core::handshake::Handshake;
use jafnify_core::pairing::{DiscoveryToken, PairingCode};
use jafnify_core::pake::{PakeError, PakeLink};
use jafnify_core::state::Role;
use jafnify_core::KeyPair;

#[test]
fn token_byte_round_trip() {
    let token = DiscoveryToken::derive(&[42u8; 16]);
    let bytes = token.to_bytes();
    let back = DiscoveryToken::from_bytes(&bytes);
    assert_eq!(back, token);
}

#[test]
fn announce_then_receive_over_multicast() {
    // Pin to loopback: the default route on dev machines is often a tunnel
    // (wg0) that swallows multicast, making the kernel-default interface test
    // environment-dependent.
    let cfg = DiscoveryConfig {
        iface: Some(std::net::Ipv4Addr::LOCALHOST),
        // Not the product's 9799: a running app on this host also binds it.
        addr: "239.255.42.99:49932".parse().unwrap(),
        ..DiscoveryConfig::default()
    };
    let tx = Discovery::bind(&cfg).unwrap();
    let rx = Discovery::bind(&cfg).unwrap();
    let token = DiscoveryToken::derive(&[9u8; 16]);
    tx.announce(&token).unwrap();
    let got = rx
        .try_receive(Duration::from_secs(3))
        .expect("multicast receive timed out -- is multicast loop enabled on lo?")
        .expect("expected one datagram");
    assert_eq!(got.1, token);
}

#[test]
fn no_interface_is_refused_by_discovery_handshake_and_pake() {
    let cfg = DiscoveryConfig::default();
    assert!(matches!(
        Discovery::bind(&cfg),
        Err(DiscoveryError::NoLanInterface)
    ));
    assert!(matches!(
        Handshake::bind(&cfg),
        Err(DiscoveryError::NoLanInterface)
    ));
    let code = PairingCode::parse("1234").unwrap();
    let key = KeyPair::generate();
    assert!(matches!(
        PakeLink::open(&cfg, Role::Joiner, &code, &key, 0),
        Err(PakeError::Discovery(DiscoveryError::NoLanInterface))
    ));
}
