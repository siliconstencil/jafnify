//! Unicast discovery. On a hotspot the
//! phone's multicast can be filtered before it reaches the laptop, while plain
//! unicast UDP works. These tests cover: unicast frames of every type reach the
//! consumer of that type (SO_REUSEPORT would deliver each unicast datagram to
//! only one of the sockets bound on the port), a configured gateway target gets
//! our frames, and a verified peer's source is answered directly.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

mod common;

use common::legacy_04_frame;
use jafnify_core::discovery::{Discovery, DiscoveryConfig};
use jafnify_core::handshake::Handshake;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{KnownSocket, PairSecret};
use jafnify_core::pairing::{DiscoveryToken, PairingCode};
use jafnify_core::state::Role;
use jafnify_core::LISTEN_PORT;

// The test port is shared by the whole binary: one at a time. Not the product's
// 9799: a running jafnify-gui on this host also binds it, and the kernel may
// give it the unicast datagrams this test sends.
const TEST_PORT: u16 = 49799;
static TEST_PORT_LOCK: Mutex<()> = Mutex::new(());

fn code() -> PairingCode {
    PairingCode::parse("4321").unwrap()
}

fn loopback(port: u16) -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: SocketAddr::from(([239, 255, 42, 99], port)),
        ..Default::default()
    }
}

fn discovery_frame() -> Vec<u8> {
    let token = DiscoveryToken::derive(&[5u8; 16]);
    let mut f = vec![0x01];
    f.extend_from_slice(&token.to_bytes());
    f
}

fn handshake_frame() -> Vec<u8> {
    legacy_04_frame(&code(), &KeyPair::generate(), LISTEN_PORT)
}

fn known_frame() -> Vec<u8> {
    PairSecret::from_bytes([0x22; 32])
        .hello(Role::Joiner, [9u8; 16])
        .encode()
        .to_vec()
}

#[test]
fn unicast_frames_of_every_type_reach_their_consumer() {
    let _g = serial();
    let disc = Discovery::bind(&loopback(TEST_PORT)).unwrap();
    let hs = Handshake::bind(&loopback(TEST_PORT)).unwrap();
    let known = KnownSocket::bind(&loopback(TEST_PORT)).unwrap();

    // Each datagram leaves from a fresh source port, so the kernel's
    // SO_REUSEPORT hash would spread them over the three sockets.
    let mut got = (0, 0, 0);
    for _ in 0..20 {
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(&discovery_frame(), format!("127.0.0.1:{TEST_PORT}"))
            .unwrap();
        sender
            .send_to(&handshake_frame(), format!("127.0.0.1:{TEST_PORT}"))
            .unwrap();
        sender
            .send_to(&known_frame(), format!("127.0.0.1:{TEST_PORT}"))
            .unwrap();
        if disc
            .try_receive(Duration::from_millis(100))
            .unwrap()
            .is_some()
        {
            got.0 += 1;
        }
        if hs
            .try_receive(Duration::from_millis(100))
            .unwrap()
            .is_some()
        {
            got.1 += 1;
        }
        if known.receive(Duration::from_millis(100)).unwrap().is_some() {
            got.2 += 1;
        }
    }
    assert_eq!(got, (20, 20, 20), "unicast frames lost or misrouted");
}

#[cfg(unix)]
#[test]
fn a_configured_gateway_target_gets_our_frames() {
    let _g = serial();
    // Port 19799 keeps this test's hub apart from the default-port tests. The
    // gateway stands in as 127.0.0.2:19799; it must share the port with the
    // wildcard hub socket (SO_REUSEPORT), and then the more specific address
    // receives the unicast copy.
    let gateway = reuse_bind(Ipv4Addr::new(127, 0, 0, 2), 19799);
    let cfg = DiscoveryConfig {
        unicast: vec![Ipv4Addr::new(127, 0, 0, 2)],
        ..loopback(19799)
    };
    let tx = Discovery::bind(&cfg).unwrap();
    let token = DiscoveryToken::derive(&[6u8; 16]);
    tx.announce(&token).unwrap();

    gateway
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut buf = [0u8; 256];
    let (len, _) = gateway
        .recv_from(&mut buf)
        .expect("the gateway target got no unicast copy");
    assert_eq!(len, 49);
    assert_eq!(buf[0], 0x01);
}

#[test]
fn a_verified_peer_is_answered_by_unicast() {
    let _g = serial();
    let disc = Discovery::bind(&loopback(TEST_PORT)).unwrap();
    // The peer is a plain socket outside the multicast group: it can only
    // hear us by unicast.
    let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.send_to(&discovery_frame(), format!("127.0.0.1:{TEST_PORT}"))
        .unwrap();

    let (from, _) = disc
        .try_receive(Duration::from_secs(2))
        .unwrap()
        .expect("the peer's frame was not received");
    assert_eq!(from, peer.local_addr().unwrap());
    disc.learn(from);

    disc.announce(&DiscoveryToken::derive(&[8u8; 16])).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut buf = [0u8; 256];
    let (len, src) = peer
        .recv_from(&mut buf)
        .expect("the learned peer got no unicast answer");
    assert_eq!(len, 49);
    assert_eq!(src, SocketAddr::from(([127, 0, 0, 1], TEST_PORT)));
}

#[test]
fn a_probe_that_hears_the_peer_only_by_unicast_says_so() {
    use jafnify_core::probe::{run_probe_at, LocalIface, ProbeConfig};
    let _g = serial();
    // A verified 0x01 sent straight to the probe's address, repeatedly, while
    // the probe listens: nothing is multicast. The probe uses the test port, so
    // a running app on the product's 9799 cannot take these datagrams.
    let sender = std::thread::spawn(|| {
        for _ in 0..30 {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.send_to(&discovery_frame(), format!("127.0.0.1:{TEST_PORT}"))
                .unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let report = run_probe_at(
        &ProbeConfig {
            code: code(),
            role: Role::Joiner,
            ifaces: vec![LocalIface {
                name: "lo".into(),
                ip: Ipv4Addr::LOCALHOST,
                prefix: 8,
            }],
            window: Duration::from_secs(2),
            linger: Duration::from_millis(100),
            unicast: Vec::new(),
        },
        format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        |_| {},
    )
    .unwrap();
    sender.join().unwrap();
    assert!(report.peer_heard_by_unicast(), "{:?}", report.lines());
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// SO_REUSEPORT has no Windows equivalent, so this gateway fixture is Unix
// only (and with it the test above).
#[cfg(unix)]
fn reuse_bind(ip: Ipv4Addr, port: u16) -> UdpSocket {
    let s = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .unwrap();
    s.set_reuse_address(true).unwrap();
    s.set_reuse_port(true).unwrap();
    s.bind(&SocketAddr::from((ip, port)).into()).unwrap();
    s.into()
}
