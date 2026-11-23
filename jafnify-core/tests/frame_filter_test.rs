//! Discovery (0x01) and Handshake (0x02) sockets share one multicast group and
//! port, so each socket also receives the other's frames. A wrong-type frame
//! must be skipped, not reported as "queue empty": otherwise a socket that was
//! not read for a while (Searching: nobody reads the handshake socket) holds a
//! backlog of foreign frames and the peer's real frame waits behind it, one
//! frame per poll (lab: first attempt after a restart lost 25 s).

use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;

mod common;

use common::{legacy_04_announcement, legacy_04_frame, send_raw};
use jafnify_core::discovery::{Discovery, DiscoveryConfig};
use jafnify_core::handshake::Handshake;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::pairing::{DiscoveryToken, PairingCode};

// One multicast group: one test at a time. A port other than the product's
// 9799 keeps a running app on this host out of the test.
const TEST_PORT: u16 = 49799;
static MULTICAST: Mutex<()> = Mutex::new(());

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

fn code() -> PairingCode {
    PairingCode::parse("1234").unwrap()
}

fn settle() {
    std::thread::sleep(Duration::from_millis(200));
}

/// Sends raw datagrams to the shared group, as the known-peer track will
/// (0x03 Hello/Proof). Includes 0x03 frames padded to the 0x01 (49 B) and
/// 0x02 (65 B) lengths, which only the version byte can tell apart.
fn send_known_frames() {
    use jafnify_core::known::PairSecret;
    use jafnify_core::state::Role;
    use socket2::{Domain, Protocol, Socket, Type};

    let s = PairSecret::from_bytes([0x11; 32]);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for i in 0..3u8 {
        frames.push(s.hello(Role::Joiner, [i; 16]).encode().to_vec());
        frames.push(
            s.proof(Role::Sharer, [i; 16], [i + 1; 16], &[1; 32], &[2; 32])
                .encode()
                .to_vec(),
        );
    }
    for len in [49usize, 65] {
        let mut f = vec![0xAB; len];
        f[0] = 0x03;
        frames.push(f);
    }
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    sock.set_multicast_if_v4(&Ipv4Addr::LOCALHOST).unwrap();
    sock.set_multicast_loop_v4(true).unwrap();
    let dest: std::net::SocketAddr = cfg().addr;
    for f in &frames {
        sock.send_to(f, &dest.into()).unwrap();
    }
}

#[test]
fn handshake_receive_skips_queued_known_frames() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let rx = Handshake::bind(&cfg()).unwrap();

    send_known_frames();
    let keypair = KeyPair::generate();
    let ann = legacy_04_announcement(&code(), &keypair, jafnify_core::LISTEN_PORT);
    send_raw(
        cfg().addr,
        &[legacy_04_frame(
            &code(),
            &keypair,
            jafnify_core::LISTEN_PORT,
        )],
    );
    settle();

    let got = rx
        .try_receive(Duration::ZERO)
        .unwrap()
        .expect("the 0x02 is stuck behind 0x03 frames");
    assert_eq!(got.1, ann);
}

#[test]
fn discovery_receive_skips_queued_known_frames() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let rx = Discovery::bind(&cfg()).unwrap();
    let disc_tx = Discovery::bind(&cfg()).unwrap();

    send_known_frames();
    let token = DiscoveryToken::derive(&[9; 16]);
    disc_tx.announce(&token).unwrap();
    settle();

    let got = rx
        .try_receive(Duration::ZERO)
        .unwrap()
        .expect("the 0x01 is stuck behind 0x03 frames");
    assert_eq!(got.1, token);
}

#[test]
fn receive_returns_none_when_only_known_frames_were_queued() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let hs = Handshake::bind(&cfg()).unwrap();
    let disc = Discovery::bind(&cfg()).unwrap();
    send_known_frames();
    settle();
    assert!(hs.try_receive(Duration::ZERO).unwrap().is_none());
    // The 0x03 frames were drained by the handshake socket only; the discovery
    // socket has its own queue and must also report none, on time.
    let start = std::time::Instant::now();
    assert!(disc
        .try_receive(Duration::from_millis(150))
        .unwrap()
        .is_none());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn handshake_receive_skips_queued_discovery_frames() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let rx = Handshake::bind(&cfg()).unwrap();
    let disc_tx = Discovery::bind(&cfg()).unwrap();

    for i in 0..5u8 {
        disc_tx.announce(&DiscoveryToken::derive(&[i; 16])).unwrap();
    }
    let keypair = KeyPair::generate();
    let ann = legacy_04_announcement(&code(), &keypair, jafnify_core::LISTEN_PORT);
    send_raw(
        cfg().addr,
        &[legacy_04_frame(
            &code(),
            &keypair,
            jafnify_core::LISTEN_PORT,
        )],
    );
    settle();

    let got = rx
        .try_receive(Duration::ZERO)
        .unwrap()
        .expect("the 0x02 is stuck behind 0x01 frames");
    assert_eq!(got.1, ann);
}

#[test]
fn discovery_receive_skips_queued_handshake_frames() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let rx = Discovery::bind(&cfg()).unwrap();
    let disc_tx = Discovery::bind(&cfg()).unwrap();

    let frames: Vec<Vec<u8>> = (0..5)
        .map(|_| legacy_04_frame(&code(), &KeyPair::generate(), jafnify_core::LISTEN_PORT))
        .collect();
    send_raw(cfg().addr, &frames);
    let token = DiscoveryToken::derive(&[7; 16]);
    disc_tx.announce(&token).unwrap();
    settle();

    let got = rx
        .try_receive(Duration::ZERO)
        .unwrap()
        .expect("the 0x01 is stuck behind 0x02 frames");
    assert_eq!(got.1, token);
}

#[test]
fn receive_returns_none_when_only_foreign_frames_were_queued() {
    let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
    let rx = Handshake::bind(&cfg()).unwrap();
    let disc_tx = Discovery::bind(&cfg()).unwrap();
    disc_tx.announce(&DiscoveryToken::derive(&[1; 16])).unwrap();
    settle();
    assert!(rx.try_receive(Duration::ZERO).unwrap().is_none());
    // And a blocking receive with a timeout still ends on time.
    let start = std::time::Instant::now();
    assert!(rx
        .try_receive(Duration::from_millis(150))
        .unwrap()
        .is_none());
    assert!(start.elapsed() < Duration::from_secs(1));
}
