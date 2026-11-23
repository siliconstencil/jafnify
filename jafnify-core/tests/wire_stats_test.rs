//! Additive counters on the shared control socket. The counters are
//! diagnostics only: a send/recv still returns exactly what it did before.
//! Every test uses a unique loopback group port and holds `SERIAL`, because
//! the process-wide counters are shared by all live sockets.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::known::KnownSocket;
use jafnify_core::wire;
use jafnify_core::wire::HubStats;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfg(port: u16, unicast: Vec<Ipv4Addr>) -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{port}").parse().unwrap(),
        unicast,
        ..Default::default()
    }
}

fn delta(after: HubStats, before: HubStats, field: impl Fn(&HubStats) -> u64) -> u64 {
    field(&after).saturating_sub(field(&before))
}

#[test]
fn one_sent_datagram_counts_one_packet_and_its_bytes() {
    let _g = serial();
    let socket = KnownSocket::bind(&cfg(49821, Vec::new())).unwrap();
    let before = wire::stats().unwrap();

    socket.send(b"hello").unwrap();

    let after = wire::stats().unwrap();
    assert_eq!(delta(after, before, |s| s.tx_packets), 1);
    assert_eq!(delta(after, before, |s| s.tx_bytes), 5);
    assert!(after.last_tx.is_some());
    assert_eq!(delta(after, before, |s| s.send_errors), 0);
}

#[test]
fn one_received_datagram_counts_one_packet_and_its_bytes() {
    let _g = serial();
    let config = cfg(49822, Vec::new());
    let receiver = KnownSocket::bind(&config).unwrap();
    let sender = KnownSocket::bind(&config).unwrap();
    let before = wire::stats().unwrap();

    // The known-peer endpoint takes only 0x03 frames; `receive_raw` keeps the
    // bytes undecoded, so a 0x03 prefix is enough to be delivered.
    sender.send(b"\x03hello").unwrap();
    let datagram = receiver.receive_raw(Duration::from_millis(500)).unwrap();

    let bytes = datagram.expect("the loopback datagram arrived").1;
    assert_eq!(bytes.as_slice(), b"\x03hello");
    let after = wire::stats().unwrap();
    assert_eq!(delta(after, before, |s| s.rx_packets), 1);
    assert_eq!(delta(after, before, |s| s.rx_bytes), 6);
    assert!(after.last_rx.is_some());
    assert_eq!(delta(after, before, |s| s.recv_errors), 0);
}

#[test]
fn a_failed_unicast_counts_an_error_without_changing_the_return() {
    let _g = serial();
    // The multicast send succeeds; sending to the limited broadcast address
    // without SO_BROADCAST fails. The endpoint still reports the overall send
    // as Ok, exactly as it did before counters existed.
    let socket = KnownSocket::bind(&cfg(49823, vec![Ipv4Addr::BROADCAST])).unwrap();
    let before = wire::stats().unwrap();

    socket
        .send(b"hello")
        .expect("multicast success is still Ok");

    let after = wire::stats().unwrap();
    assert!(
        delta(after, before, |s| s.send_errors) >= 1,
        "a failing unicast was not counted: {after:?}"
    );
    assert_ne!(after.last_errno, 0, "the errno was not recorded");
    assert_eq!(
        delta(after, before, |s| s.tx_packets),
        1,
        "only the multicast datagram was sent"
    );
}
