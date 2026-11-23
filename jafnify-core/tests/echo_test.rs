//! Did our own multicast come back? Multicast loopback re-enters the host's
//! INPUT path on the LAN interface (not on lo), so a firewall that drops the
//! discovery group drops our own announcements too: ufw logged exactly that
//! in the lab (21 blocks of SRC=<own address>). "Never heard ourselves" is
//! therefore a root-free signal that inbound discovery is blocked locally,
//! whatever the firewall is, with no new frame on the wire.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::discovery::{Discovery, DiscoveryConfig};
use jafnify_core::echo::EchoWatch;
use jafnify_core::pairing::DiscoveryToken;

static MULTICAST: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

fn loopback() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        // Not the product's 9799: a running app on this host also binds it.
        addr: "239.255.42.99:49931".parse().unwrap(),
        ..Default::default()
    }
}

fn announce_once(cfg: &DiscoveryConfig) {
    Discovery::bind(cfg)
        .unwrap()
        .announce(&DiscoveryToken::derive(&[5u8; 16]))
        .unwrap();
}

fn heard_within(watch: &mut EchoWatch, wait: Duration) -> bool {
    let end = Instant::now() + wait;
    while Instant::now() < end {
        if watch.poll() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn our_own_announcement_coming_back_is_an_echo() {
    let _g = serial();
    let mut watch = EchoWatch::bind(&loopback(), vec![Ipv4Addr::LOCALHOST]).unwrap();
    assert!(!watch.poll(), "nothing was sent yet");
    announce_once(&loopback());
    assert!(heard_within(&mut watch, Duration::from_secs(2)));
}

#[test]
fn traffic_from_other_addresses_is_not_an_echo() {
    let _g = serial();
    // The announcement arrives from 127.0.0.1; our address is another one,
    // as if a firewall had eaten the real loopback copy and only a stranger's
    // frames came through.
    let mut watch = EchoWatch::bind(&loopback(), vec![Ipv4Addr::new(10, 0, 0, 1)]).unwrap();
    announce_once(&loopback());
    assert!(!heard_within(&mut watch, Duration::from_millis(500)));
    // ...but it IS a frame from another host: the peer's traffic gets through.
    assert!(watch.peer_heard());
}

#[test]
fn nothing_heard_means_no_peer() {
    let _g = serial();
    let mut watch = EchoWatch::bind(&loopback(), vec![Ipv4Addr::LOCALHOST]).unwrap();
    assert!(!watch.poll());
    assert!(!watch.peer_heard());
}

#[test]
fn once_heard_it_stays_heard() {
    let _g = serial();
    let mut watch = EchoWatch::bind(&loopback(), vec![Ipv4Addr::LOCALHOST]).unwrap();
    announce_once(&loopback());
    assert!(heard_within(&mut watch, Duration::from_secs(2)));
    assert!(watch.poll());
}
