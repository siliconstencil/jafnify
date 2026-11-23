mod common;

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use base64::Engine;
use common::{legacy_04_announcement, legacy_04_frame, send_raw};
use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::handshake::{Handshake, HandshakeAnnouncement};
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::pairing::PairingCode;
use jafnify_core::LISTEN_PORT;

fn code() -> PairingCode {
    PairingCode::parse("1234").unwrap()
}

#[test]
fn listen_port_is_9797() {
    assert_eq!(LISTEN_PORT, 9797);
}

#[test]
fn keypair_public_key_bytes_round_trips_with_b64() {
    let kp = KeyPair::generate();
    let raw = kp.public_key_bytes();
    assert_eq!(raw.len(), 32);
    assert_eq!(
        kp.public_key_b64(),
        base64::engine::general_purpose::STANDARD.encode(raw)
    );
}

#[test]
fn announcement_byte_round_trip() {
    // The core has no builder for the legacy 0x04 frame any more (P0-1):
    // the test builds it by hand and checks the receiver's view of it.
    let ann = legacy_04_announcement(&code(), &KeyPair::generate(), LISTEN_PORT);
    let bytes = ann.to_bytes();
    assert_eq!(bytes.len(), 66); // key 32 + port 2 + mac 32
    let back = HandshakeAnnouncement::from_bytes(&bytes);
    assert_eq!(back, ann);
}

#[test]
fn announcement_verifies_only_with_its_code() {
    let ann = legacy_04_announcement(&code(), &KeyPair::generate(), LISTEN_PORT);
    assert!(ann.verify(&code()));
    let other = PairingCode::parse("5678").unwrap();
    assert!(!ann.verify(&other));
}

/// A group of its own: the two tests below share the process, and a frame of
/// one test must not reach the other one's socket.
fn cfg(port: u16) -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{port}").parse().unwrap(),
        ..Default::default()
    }
}

#[test]
fn announce_then_receive_over_multicast() {
    let cfg = cfg(49799);
    let rx = Handshake::bind(&cfg).expect("bind rx");
    let keypair = KeyPair::generate();
    let ann = legacy_04_announcement(&code(), &keypair, LISTEN_PORT);
    send_raw(cfg.addr, &[legacy_04_frame(&code(), &keypair, LISTEN_PORT)]);
    let got = rx
        .try_receive(Duration::from_secs(3))
        .expect("multicast receive timed out -- is multicast loop enabled on lo?")
        .expect("expected one datagram");
    assert_eq!(got.1, ann);
}

#[test]
fn an_old_0x04_announcement_sets_the_old_peer_flag() {
    let cfg = cfg(49798);
    let rx = Handshake::bind(&cfg).expect("bind rx");
    send_raw(
        cfg.addr,
        &[legacy_04_frame(&code(), &KeyPair::generate(), LISTEN_PORT)],
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !rx.old_peer_seen(&code()) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rx.old_peer_seen(&code()));
}
