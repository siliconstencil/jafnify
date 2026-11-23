//! MAC-authenticated public-key announcement over multicast (frame 0x04).
//!
//! Frame 0x04 is the pairing frame of the version before SPAKE2: the
//! announcement is authenticated with HMAC-SHA256(code, public_key || port).
//! A frame like that is verifiable with the code alone, so no build path may
//! build or send one any more (P0-1): the builders and the sender are gone,
//! and the frame is only *recognised* here, so a peer that still runs the old
//! version can be reported ("update the other device").
//!
//! The port is the UDP port the sharer's WireGuard gateway bound
//! (0 from a joiner, which has no fixed port).
//!
//! Frame version 1 (0x02, key + MAC over the key only) is no longer a pairing
//! frame. A device that still receives it knows the other device runs an
//! older Jafnify: `Handshake::old_peer_seen` reports it, once the MAC checks.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::discovery::{DiscoveryConfig, DiscoveryError};
use crate::pairing::PairingCode;
use crate::wire::{Accept, Endpoint, Path};

type HmacSha256 = Hmac<Sha256>;

/// Wire frame: 1 byte version (0x04) + 32 bytes key + 2 bytes port + 32 bytes MAC.
const FRAME_VERSION: u8 = 0x04;
const FRAME_LEN: usize = 67;
/// The version-1 frame (0x02, key + MAC): recognised only to report an old peer.
pub(crate) const LEGACY_VERSION: u8 = 0x02;
pub(crate) const LEGACY_FRAME_LEN: usize = 65;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeAnnouncement {
    pub public_key: [u8; 32],
    /// The UDP port the sender's WireGuard gateway listens on (0 = none).
    pub port: u16,
    pub mac: [u8; 32],
}

fn mac_over(code: &PairingCode, public_key: &[u8; 32], port: u16) -> HmacSha256 {
    let mut mac =
        HmacSha256::new_from_slice(code.as_str().as_bytes()).expect("HMAC accepts any key length");
    mac.update(public_key);
    mac.update(&port.to_be_bytes());
    mac
}

impl HandshakeAnnouncement {
    pub fn verify(&self, code: &PairingCode) -> bool {
        mac_over(code, &self.public_key, self.port)
            .verify_slice(&self.mac)
            .is_ok()
    }

    /// Wire format: public_key (32) + port (2, big endian) + mac (32). The
    /// version byte is added by the discovery layer.
    pub fn to_bytes(&self) -> [u8; FRAME_LEN - 1] {
        let mut out = [0u8; FRAME_LEN - 1];
        out[..32].copy_from_slice(&self.public_key);
        out[32..34].copy_from_slice(&self.port.to_be_bytes());
        out[34..].copy_from_slice(&self.mac);
        out
    }

    pub fn from_bytes(b: &[u8; FRAME_LEN - 1]) -> Self {
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&b[..32]);
        let port = u16::from_be_bytes([b[32], b[33]]);
        let mut mac = [0u8; 32];
        mac.copy_from_slice(&b[34..]);
        Self {
            public_key,
            port,
            mac,
        }
    }
}

/// A version-1 frame whose MAC (over the key alone) verifies under `code`.
fn legacy_verifies(code: &PairingCode, frame: &[u8]) -> bool {
    if frame.len() != LEGACY_FRAME_LEN || frame[0] != LEGACY_VERSION {
        return false;
    }
    let mut mac =
        HmacSha256::new_from_slice(code.as_str().as_bytes()).expect("HMAC accepts any key length");
    mac.update(&frame[1..33]);
    mac.verify_slice(&frame[33..]).is_ok()
}

pub struct Handshake {
    ep: Endpoint,
    legacy: Endpoint,
    old_seen: AtomicBool,
}

impl Handshake {
    /// Shares the process's 9799 socket with the discovery and known-peer
    /// endpoints (see `wire.rs`).
    pub fn bind(cfg: &DiscoveryConfig) -> Result<Self, DiscoveryError> {
        if cfg.interfaces().is_empty() {
            return Err(DiscoveryError::NoLanInterface);
        }
        Ok(Self {
            ep: Endpoint::open(cfg, Accept::Handshake)?,
            legacy: Endpoint::open(cfg, Accept::LegacyHandshake)?,
            old_seen: AtomicBool::new(false),
        })
    }

    /// Remembers a verified peer's address (see `Discovery::learn`).
    pub fn learn(&self, from: SocketAddr) {
        self.ep.learn(from);
    }

    /// True once a legacy pairing frame with a valid MAC under `code` has
    /// arrived: the other device runs an older Jafnify and cannot pair with
    /// this one. Sticky for the life of this socket set.
    ///
    /// This drains the 0x02 and 0x04 endpoints. Once the frontends use PAKE,
    /// 0x04 is legacy-only, so draining it here cannot steal a live frame.
    pub fn old_peer_seen(&self, code: &PairingCode) -> bool {
        while let Ok(Some(d)) = self.legacy.recv(Duration::ZERO) {
            if legacy_verifies(code, &d.frame) {
                self.old_seen.store(true, Ordering::Relaxed);
            }
        }
        while let Ok(Some(d)) = self.ep.recv(Duration::ZERO) {
            if d.frame.len() == FRAME_LEN && d.frame[0] == FRAME_VERSION {
                let mut bytes = [0u8; FRAME_LEN - 1];
                bytes.copy_from_slice(&d.frame[1..]);
                if HandshakeAnnouncement::from_bytes(&bytes).verify(code) {
                    self.old_seen.store(true, Ordering::Relaxed);
                }
            }
        }
        self.old_seen.load(Ordering::Relaxed)
    }

    pub fn try_receive(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, HandshakeAnnouncement)>, DiscoveryError> {
        Ok(self
            .try_receive_via(timeout)?
            .map(|(from, _, ann)| (from, ann)))
    }

    /// As `try_receive`, also saying whether the frame came by multicast or
    /// unicast.
    pub fn try_receive_via(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, Path, HandshakeAnnouncement)>, DiscoveryError> {
        let Some(d) = self.ep.recv(timeout)? else {
            return Ok(None);
        };
        let mut announcement = [0u8; FRAME_LEN - 1];
        announcement.copy_from_slice(&d.frame[1..FRAME_LEN]);
        Ok(Some((
            d.from,
            d.path,
            HandshakeAnnouncement::from_bytes(&announcement),
        )))
    }
}
