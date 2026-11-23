//! A device with no usable network (Wi-Fi or hotspot gone) is not a peer
//! failure: the known-peer poll keeps running, reports "no network" and
//! clears it on the next send that goes out. Any other socket error still
//! fails the poll.

use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{is_no_network_kind, KnownError, KnownTrack, HELLO_INTERVAL_SEARCHING};
use jafnify_core::state::Role;

fn track(now: Instant) -> KnownTrack {
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: "239.255.42.97:49813".parse().unwrap(),
        ..Default::default()
    };
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    KnownTrack::open(&cfg, &own, &peer.public_key_b64(), Role::Joiner, now).unwrap()
}

#[test]
fn the_no_network_kinds_are_the_unreachable_ones() {
    for kind in [
        ErrorKind::NetworkUnreachable,
        ErrorKind::HostUnreachable,
        ErrorKind::NetworkDown,
        ErrorKind::AddrNotAvailable,
    ] {
        assert!(is_no_network_kind(kind), "{kind:?}");
    }
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::ConnectionRefused,
        ErrorKind::InvalidInput,
        ErrorKind::Other,
    ] {
        assert!(!is_no_network_kind(kind), "{kind:?}");
    }
}

#[test]
fn a_poll_with_no_network_does_not_fail_and_clears_when_a_send_goes_out() {
    let t0 = Instant::now();
    let mut t = track(t0);
    t.fail_sends(Some(ErrorKind::NetworkUnreachable));
    let polled = t.poll_detailed(t0).expect("no network is not a poll error");
    assert_eq!(polled.endpoint, None);
    let text = t.no_network().expect("no network is reported").to_string();
    assert!(text.contains("unreachable"), "{text}");
    // Between Hellos nothing is sent: the state holds.
    t.poll_detailed(t0 + Duration::from_millis(100)).unwrap();
    assert!(t.no_network().is_some());

    t.fail_sends(None);
    t.poll_detailed(t0 + HELLO_INTERVAL_SEARCHING).unwrap();
    assert_eq!(t.no_network(), None, "the next Hello went out");
}

#[test]
fn a_probe_with_no_network_does_not_fail() {
    let t0 = Instant::now();
    let mut t = track(t0);
    t.fail_sends(Some(ErrorKind::NetworkDown));
    t.probe(t0).expect("no network is not a probe error");
    assert!(t.no_network().is_some());
}

#[test]
fn another_socket_error_still_fails_the_poll() {
    let t0 = Instant::now();
    let mut t = track(t0);
    t.fail_sends(Some(ErrorKind::PermissionDenied));
    match t.poll_detailed(t0) {
        Err(e @ KnownError::Socket(_)) => assert!(!e.is_no_network()),
        other => panic!("expected a socket error, got {other:?}"),
    }
    assert_eq!(t.no_network(), None);
}
