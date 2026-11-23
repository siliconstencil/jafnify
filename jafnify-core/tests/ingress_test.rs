//! Ingress filter on the 9799 socket.
//! The socket is bound to 0.0.0.0, so a frame reaches it whatever address it was
//! sent to. A device that pins its interfaces takes frames sent to one of those
//! addresses (or the group) only: a frame arriving on another interface, such as
//! cellular, is ignored.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

mod common;

use common::legacy_04_frame;
use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::handshake::Handshake;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::pairing::PairingCode;

// The test port is shared by the whole binary: one test at a time.
static TEST_PORT_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    TEST_PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn code() -> PairingCode {
    PairingCode::parse("4321").unwrap()
}

/// Pinned to 127.0.0.1: this device listens on loopback only. A port other
/// than the product's 9799 keeps a running app on this host out of the test.
const TEST_PORT: u16 = 49799;

fn pinned() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

/// A valid 0x04 handshake frame, as the sharer used to send it.
fn handshake_frame() -> Vec<u8> {
    legacy_04_frame(&code(), &KeyPair::generate(), 9800)
}

#[test]
fn a_frame_sent_to_an_address_this_device_does_not_use_is_ignored() {
    let _g = serial();
    let hs = Handshake::bind(&pinned()).unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let frame = handshake_frame();

    // 127.0.0.2 is local but not one of our interfaces (like a cellular address).
    sender
        .send_to(&frame, format!("127.0.0.2:{TEST_PORT}"))
        .unwrap();
    let got = hs.try_receive(Duration::from_millis(400)).unwrap();
    assert!(got.is_none(), "a frame to another address must be ignored");

    // The same frame to our own address is received.
    sender
        .send_to(&frame, format!("127.0.0.1:{TEST_PORT}"))
        .unwrap();
    let got = hs.try_receive(Duration::from_secs(2)).unwrap();
    assert!(got.is_some(), "a frame to our address must be received");
}
