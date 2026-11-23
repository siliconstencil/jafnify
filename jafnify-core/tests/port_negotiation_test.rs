//! The WireGuard port is negotiated.
//! The sharer announces the UDP port its gateway bound: in the 0x04 handshake
//! frame, and in the known-peer Proof (frame version 2). The port is under the
//! MAC, so a LAN attacker cannot redirect the joiner's tunnel by rewriting it.
//! A version-1 peer is recognised, authenticated, and reported as "update the
//! other device" instead of failing silently.

use std::net::UdpSocket;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

mod common;

use common::{legacy_04_announcement, legacy_04_frame};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use jafnify_core::config::ConfigGenerator;
use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::handshake::{Handshake, HandshakeAnnouncement};
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{KnownSession, KnownTrack};
use jafnify_core::pairing::PairingCode;
use jafnify_core::state::Role;
use std::net::Ipv4Addr;

// The test port is shared by the whole binary: one at a time. Not the product's
// 9799, which a running Jafnify app on this host may also bind.
const TEST_PORT: u16 = 49799;
static TEST_PORT_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    TEST_PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn code() -> PairingCode {
    PairingCode::parse("4321").unwrap()
}

fn loopback() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

/// A version-1 0x02 frame as the old app sent it: key + MAC(code, key).
fn legacy_handshake_frame(code: &PairingCode, pk: [u8; 32]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_str().as_bytes()).unwrap();
    mac.update(&pk);
    let mut f = vec![0x02];
    f.extend_from_slice(&pk);
    f.extend_from_slice(&mac.finalize().into_bytes());
    f
}

fn pair() -> (KeyPair, KeyPair) {
    (KeyPair::generate(), KeyPair::generate())
}

// ---- the handshake frame carries the port under the MAC ----------------------

#[test]
fn the_announced_port_is_covered_by_the_mac() {
    let kp = KeyPair::generate();
    let ann = legacy_04_announcement(&code(), &kp, 9800);
    assert_eq!(ann.port, 9800);
    assert!(ann.verify(&code()));
    let forged = HandshakeAnnouncement {
        port: 9801,
        ..ann.clone()
    };
    assert!(!forged.verify(&code()), "a rewritten port must not verify");
}

#[test]
fn a_new_handshake_frame_delivers_the_port_to_the_receiver() {
    let _g = serial();
    let hs = Handshake::bind(&loopback()).unwrap();
    let keypair = KeyPair::generate();
    let ann = legacy_04_announcement(&code(), &keypair, 9800);
    common::send_raw(loopback().addr, &[legacy_04_frame(&code(), &keypair, 9800)]);
    let (_, got) = hs
        .try_receive(Duration::from_secs(2))
        .unwrap()
        .expect("the 0x04 frame was not received");
    assert_eq!(got, ann);
    assert_eq!(got.port, 9800);
}

// ---- a version-1 peer is recognised, not silently ignored --------------------

#[test]
fn an_old_handshake_frame_with_a_valid_mac_marks_an_old_peer() {
    let _g = serial();
    let hs = Handshake::bind(&loopback()).unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let pk = KeyPair::generate().public_key_bytes();
    let mut bad = legacy_handshake_frame(&code(), pk);
    *bad.last_mut().unwrap() ^= 1;
    sender
        .send_to(&bad, format!("127.0.0.1:{TEST_PORT}"))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !hs.old_peer_seen(&code()),
        "a forged old frame must not count"
    );

    sender
        .send_to(
            &legacy_handshake_frame(&code(), pk),
            format!("127.0.0.1:{TEST_PORT}"),
        )
        .unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end && !hs.old_peer_seen(&code()) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(hs.old_peer_seen(&code()));
    // A version-1 frame is never a pairing frame.
    assert!(hs.try_receive(Duration::ZERO).unwrap().is_none());
}

// ---- the known-peer Proof carries the port under its MAC ---------------------

#[test]
fn the_proof_port_is_covered_by_the_mac() {
    let (a, b) = pair();
    let mut sharer = KnownSession::new(&a, &b.public_key_b64(), Role::Sharer).unwrap();
    sharer.set_port(9800);
    let joiner = KnownSession::new(&b, &a.public_key_b64(), Role::Joiner).unwrap();

    let proof = sharer.on_hello(&joiner.hello()).expect("a proof");
    assert_eq!(proof.port, 9800);
    assert!(joiner.accept_proof(&proof));

    let forged = jafnify_core::known::Proof {
        port: 9801,
        ..proof
    };
    assert!(
        !joiner.accept_proof(&forged),
        "a rewritten port must not verify"
    );
}

#[test]
fn a_version_one_hello_from_the_stored_peer_is_recognised() {
    let (a, b) = pair();
    let sharer = KnownSession::new(&a, &b.public_key_b64(), Role::Sharer).unwrap();
    let joiner = KnownSession::new(&b, &a.public_key_b64(), Role::Joiner).unwrap();

    let mut old = joiner.hello().encode().to_vec();
    old[1] = 1; // the version before ports; the tag does not cover it
    assert!(sharer.legacy_hello_is_ours(&old));

    let mut forged = old.clone();
    forged[20] ^= 1; // a byte of the tag
    assert!(!sharer.legacy_hello_is_ours(&forged));

    let current = joiner.hello().encode();
    assert!(
        !sharer.legacy_hello_is_ours(&current),
        "version 2 is not old"
    );

    let stranger = KnownSession::new(&KeyPair::generate(), &a.public_key_b64(), Role::Joiner)
        .unwrap()
        .hello()
        .encode();
    let mut stranger_old = stranger.to_vec();
    stranger_old[1] = 1;
    assert!(
        !sharer.legacy_hello_is_ours(&stranger_old),
        "another pair's hello"
    );
}

#[test]
fn a_track_reports_an_old_peer_from_the_wire() {
    let _g = serial();
    let (a, b) = pair();
    let mut track = KnownTrack::open(
        &loopback(),
        &a,
        &b.public_key_b64(),
        Role::Sharer,
        Instant::now(),
    )
    .unwrap();
    let joiner = KnownSession::new(&b, &a.public_key_b64(), Role::Joiner).unwrap();
    let mut old = joiner.hello().encode().to_vec();
    old[1] = 1;
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(&old, format!("127.0.0.1:{TEST_PORT}"))
        .unwrap();

    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end && !track.old_peer_seen() {
        track.poll(Instant::now()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(track.old_peer_seen());
}

// ---- the joiner's config never pins a local port ------------------------------

#[test]
fn the_joiner_config_has_no_fixed_listen_port() {
    let cfg = ConfigGenerator::joiner_config("x", "y", "192.168.43.1:9796");
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert!(!text.contains("ListenPort"), "{text}");
}
