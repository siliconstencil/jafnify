//! Helpers shared by the core integration tests.
//!
//! The legacy 0x04 pairing frame is built here by hand: the core no longer
//! exposes a builder or a sender for it (the frame is verifiable with the
//! pairing code alone, P0-1), but the tests must still be able to put one on
//! the wire to prove that the receivers skip it and report an old peer.

#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};

use hmac::{Hmac, Mac};
use jafnify_core::handshake::HandshakeAnnouncement;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::pairing::PairingCode;
use sha2::Sha256;
use socket2::{Domain, Protocol, Socket, Type};

type HmacSha256 = Hmac<Sha256>;

/// The legacy 0x04 frame: type byte + key (32) + port (2) + MAC (32) = 67 B.
pub const LEGACY_04_LEN: usize = 67;

/// The legacy 0x02 frame: type byte + key (32) + MAC over the key (32) = 65 B.
pub const LEGACY_02_LEN: usize = 65;

/// The legacy 0x04 announcement without its type byte (66 B): the shape
/// `HandshakeAnnouncement` carries.
pub fn legacy_04_bytes(code: &PairingCode, keypair: &KeyPair, port: u16) -> [u8; 66] {
    let key = keypair.public_key_bytes();
    let mut mac =
        HmacSha256::new_from_slice(code.as_str().as_bytes()).expect("HMAC accepts any key length");
    mac.update(&key);
    mac.update(&port.to_be_bytes());
    let mac = mac.finalize().into_bytes();
    let mut out = [0u8; 66];
    out[..32].copy_from_slice(&key);
    out[32..34].copy_from_slice(&port.to_be_bytes());
    out[34..].copy_from_slice(&mac);
    out
}

/// The legacy 0x04 frame on the wire (67 B, type byte first).
pub fn legacy_04_frame(code: &PairingCode, keypair: &KeyPair, port: u16) -> Vec<u8> {
    let mut frame = vec![0x04u8];
    frame.extend_from_slice(&legacy_04_bytes(code, keypair, port));
    frame
}

/// The legacy 0x04 announcement as the receiver sees it.
pub fn legacy_04_announcement(
    code: &PairingCode,
    keypair: &KeyPair,
    port: u16,
) -> HandshakeAnnouncement {
    HandshakeAnnouncement::from_bytes(&legacy_04_bytes(code, keypair, port))
}

/// The legacy 0x02 frame on the wire (65 B, type byte first).
pub fn legacy_02_frame(code: &PairingCode, keypair: &KeyPair) -> Vec<u8> {
    let key = keypair.public_key_bytes();
    let mut mac =
        HmacSha256::new_from_slice(code.as_str().as_bytes()).expect("HMAC accepts any key length");
    mac.update(&key);
    let mac = mac.finalize().into_bytes();
    let mut frame = vec![0x02u8];
    frame.extend_from_slice(&key);
    frame.extend_from_slice(&mac);
    frame
}

/// Sends raw frames to the multicast group, as a peer on the LAN would.
pub fn send_raw(addr: SocketAddr, frames: &[Vec<u8>]) {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).expect("socket");
    sock.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)
        .expect("pin the loopback interface");
    sock.set_multicast_loop_v4(true).expect("multicast loop");
    let dest: SocketAddr = addr;
    for f in frames {
        sock.send_to(f, &dest.into()).expect("send");
    }
}
