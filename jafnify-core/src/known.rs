//! Known-peer reconnect: pair secret `S`, pairId and the 0x03 Hello/Proof
//! frames.
//!
//! Once two devices hold each other's static WireGuard key, `S` is derived
//! from their X25519 shared secret, so it is NOT derived from the pairing
//! code. A Hello proves "I know S" without sending any key, id or name; a
//! Proof answers one specific Hello nonce and binds both public keys.
//!
//! Wire (0x01 and 0x02 frames are untouched; old builds skip 0x03):
//!   header  `0x03 | ver=1 | kind | role`   (kind 1 Hello, 2 Proof; role 0 sharer, 1 joiner)
//!   Hello   header + n(16) + tag(16)                         = 36 bytes
//!   Proof   header + n_self(16) + n_echo(16) + mac(32)       = 68 bytes
//!   Signal  header + n_self(16) + n_echo(16) + mac(32)       = 68 bytes
//!           (kind 3 Bye: the sender is stopping; kind 4 Fresh: the sender
//!           just started a new session with this peer). Older builds reject
//!           the unknown kind and ignore the frame.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

use base64::Engine as _;

use crate::discovery::DiscoveryConfig;
use crate::keyexchange::KeyPair;
use crate::names::display_name;
use crate::state::{Peer, Role};
use crate::wire::{Accept, Endpoint};
use crate::LISTEN_PORT;

type HmacSha256 = Hmac<Sha256>;

pub const FRAME_TYPE: u8 = 0x03;
pub const FRAME_VERSION: u8 = 3;
pub const HELLO_LEN: usize = 36;
pub const PROOF_LEN: usize = 74;
/// The version before the Proof carried the Hello source address (2).
const LEGACY_VERSION: u8 = 2;
/// The version before the Proof carried the sender's port (1).
const LEGACY_V1_VERSION: u8 = 1;

const KIND_HELLO: u8 = 1;
const KIND_PROOF: u8 = 2;
const KIND_BYE: u8 = 3;
const KIND_FRESH: u8 = 4;
pub const SIGNAL_LEN: usize = 68;
/// How many copies of a Bye go out on stop (UDP may drop one).
const BYE_COPIES: usize = 3;
/// How many accepted-Proof polls of a new track also send a Fresh.
const FRESH_COPIES: u8 = 3;
const TAG_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum KnownError {
    #[error("unusable peer key: {0}")]
    Key(String),
    #[error("known-peer socket error: {0}")]
    Socket(String),
    /// A send failed because this device has no usable network right now
    /// (Wi-Fi or hotspot gone, no route). Not a peer problem: the caller can
    /// keep searching quietly until the network returns.
    #[error("known-peer socket error: {0}")]
    NoNetwork(String),
}

impl KnownError {
    /// True when the device itself has no usable network.
    pub fn is_no_network(&self) -> bool {
        matches!(self, KnownError::NoNetwork(_))
    }
}

/// The send errors that mean "this device has no usable network", as opposed
/// to a broken socket.
pub fn is_no_network_kind(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind;
    matches!(
        kind,
        ErrorKind::NetworkUnreachable
            | ErrorKind::HostUnreachable
            | ErrorKind::NetworkDown
            | ErrorKind::AddrNotAvailable
    )
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    /// Not a 0x03 frame at all (empty, or another frame type sharing the port).
    #[error("not a known-peer frame")]
    NotKnown,
    #[error("bad frame length")]
    Length,
    #[error("unsupported version {0}")]
    Version(u8),
    #[error("unknown kind {0}")]
    Kind(u8),
    #[error("unknown role {0}")]
    Role(u8),
}

fn role_byte(role: Role) -> u8 {
    match role {
        Role::Sharer => 0,
        Role::Joiner => 1,
    }
}

fn mac_for(secret: &PairSecret) -> HmacSha256 {
    HmacSha256::new_from_slice(&secret.0).expect("HMAC accepts any key length")
}

/// `S`: the pair secret. Recomputed from the stored private key and the
/// peer's public key; never stored, never on the wire.
#[derive(Clone)]
pub struct PairSecret([u8; 32]);

impl std::fmt::Debug for PairSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairSecret(<redacted>)")
    }
}

impl PairSecret {
    /// `S = HMAC(X25519(own_priv, peer_pub), "jafnify/v1/pair" || min(pk) || max(pk))`.
    /// Low-order peer keys are rejected (the DH output would be all zero).
    pub fn derive(own: &KeyPair, peer_public_b64: &str) -> Result<Self, KnownError> {
        let shared = own
            .exchange(peer_public_b64)
            .map_err(|e| KnownError::Key(e.to_string()))?;
        let peer_pk = decode_pk(peer_public_b64)?;
        let own_pk = own.public_key_bytes();
        let (lo, hi) = if own_pk <= peer_pk {
            (own_pk, peer_pk)
        } else {
            (peer_pk, own_pk)
        };
        let mut mac = HmacSha256::new_from_slice(&shared).expect("HMAC accepts any key length");
        mac.update(b"jafnify/v1/pair");
        mac.update(&lo);
        mac.update(&hi);
        Ok(Self(mac.finalize().into_bytes().into()))
    }

    /// For tests and for restoring a secret from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// `hex(HMAC(S, "id")[..16])`: same on both sides, never on the wire.
    pub fn pair_id(&self) -> String {
        let mut mac = mac_for(self);
        mac.update(b"id");
        let out = mac.finalize().into_bytes();
        out[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Hello carrying `tag = HMAC(S, "H" || role || n)[..16]`.
    pub fn hello(&self, role: Role, nonce: [u8; 16]) -> Hello {
        let full = hello_mac(self, role, &nonce).finalize().into_bytes();
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&full[..TAG_LEN]);
        Hello { role, nonce, tag }
    }

    /// Proof announcing the default port (see `proof_on`).
    pub fn proof(
        &self,
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        pk_self: &[u8; 32],
        pk_peer: &[u8; 32],
    ) -> Proof {
        self.proof_with_addr(role, n_self, n_echo, pk_self, pk_peer, [127, 0, 0, 1])
    }

    pub fn proof_with_addr(
        &self,
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        pk_self: &[u8; 32],
        pk_peer: &[u8; 32],
        peer_addr: [u8; 4],
    ) -> Proof {
        self.proof_on_with_addr(
            role,
            n_self,
            n_echo,
            pk_self,
            pk_peer,
            ProofTarget {
                port: LISTEN_PORT,
                addr: peer_addr,
            },
        )
    }

    pub fn proof_on(
        &self,
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        pk_self: &[u8; 32],
        pk_peer: &[u8; 32],
        port: u16,
    ) -> Proof {
        self.proof_on_with_addr(
            role,
            n_self,
            n_echo,
            pk_self,
            pk_peer,
            ProofTarget {
                port,
                addr: [127, 0, 0, 1],
            },
        )
    }

    /// Proof `mac = HMAC(S, "P" || role || n_self || n_echo || pk_self || pk_peer || port || peer_addr)`.
    pub fn proof_on_with_addr(
        &self,
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        pk_self: &[u8; 32],
        pk_peer: &[u8; 32],
        target: ProofTarget,
    ) -> Proof {
        let mac = proof_mac(self, role, &n_self, &n_echo, pk_self, pk_peer, &target)
            .finalize()
            .into_bytes()
            .into();
        Proof {
            role,
            n_self,
            n_echo,
            port: target.port,
            peer_addr: target.addr,
            mac,
        }
    }
}

fn decode_pk(b64: &str) -> Result<[u8; 32], KnownError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| KnownError::Key(e.to_string()))?;
    raw.try_into()
        .map_err(|r: Vec<u8>| KnownError::Key(format!("expected 32 bytes, got {}", r.len())))
}

fn hello_mac(s: &PairSecret, role: Role, nonce: &[u8; 16]) -> HmacSha256 {
    let mut mac = mac_for(s);
    mac.update(b"H");
    mac.update(&[role_byte(role)]);
    mac.update(nonce);
    mac
}

/// Signal `mac = HMAC(S, "B"|"F" || role || n_self || n_echo || pk_self || pk_peer)`.
fn signal_mac(
    s: &PairSecret,
    kind: SignalKind,
    role: Role,
    n_self: &[u8; 16],
    n_echo: &[u8; 16],
    pk_self: &[u8; 32],
    pk_peer: &[u8; 32],
) -> HmacSha256 {
    let mut mac = mac_for(s);
    mac.update(match kind {
        SignalKind::Bye => b"B",
        SignalKind::Fresh => b"F",
    });
    mac.update(&[role_byte(role)]);
    mac.update(n_self);
    mac.update(n_echo);
    mac.update(pk_self);
    mac.update(pk_peer);
    mac
}

/// The port and source address bound into a Proof's MAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofTarget {
    pub port: u16,
    pub addr: [u8; 4],
}

fn proof_mac(
    s: &PairSecret,
    role: Role,
    n_self: &[u8; 16],
    n_echo: &[u8; 16],
    pk_self: &[u8; 32],
    pk_peer: &[u8; 32],
    target: &ProofTarget,
) -> HmacSha256 {
    let mut mac = mac_for(s);
    mac.update(b"P");
    mac.update(&[role_byte(role)]);
    mac.update(n_self);
    mac.update(n_echo);
    mac.update(pk_self);
    mac.update(pk_peer);
    mac.update(&target.port.to_be_bytes());
    mac.update(&target.addr);
    mac
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub role: Role,
    pub nonce: [u8; 16],
    pub tag: [u8; TAG_LEN],
}

impl Hello {
    /// Constant-time check of the tag under `S`.
    pub fn verify(&self, secret: &PairSecret) -> bool {
        hello_mac(secret, self.role, &self.nonce)
            .verify_truncated_left(&self.tag)
            .is_ok()
    }

    pub fn encode(&self) -> [u8; HELLO_LEN] {
        let mut out = [0u8; HELLO_LEN];
        out[..4].copy_from_slice(&header(KIND_HELLO, self.role));
        out[4..20].copy_from_slice(&self.nonce);
        out[20..].copy_from_slice(&self.tag);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub role: Role,
    pub n_self: [u8; 16],
    pub n_echo: [u8; 16],
    /// The sender's WireGuard port (the sharer's gateway; under the MAC).
    pub port: u16,
    /// The source IPv4 address of the Hello this Proof answers (under the MAC).
    pub peer_addr: [u8; 4],
    pub mac: [u8; 32],
}

impl Proof {
    pub fn encode(&self) -> [u8; PROOF_LEN] {
        let mut out = [0u8; PROOF_LEN];
        out[..4].copy_from_slice(&header(KIND_PROOF, self.role));
        out[4..20].copy_from_slice(&self.n_self);
        out[20..36].copy_from_slice(&self.n_echo);
        out[36..38].copy_from_slice(&self.port.to_be_bytes());
        out[38..42].copy_from_slice(&self.peer_addr);
        out[42..].copy_from_slice(&self.mac);
        out
    }
}

/// What a Signal announces about the sender's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// The sender is stopping: the peer can end its session now.
    Bye,
    /// The sender just started a new session with this peer: a peer that
    /// still counts an older session must start a new one.
    Fresh,
}

impl SignalKind {
    fn kind_byte(self) -> u8 {
        match self {
            SignalKind::Bye => KIND_BYE,
            SignalKind::Fresh => KIND_FRESH,
        }
    }
}

/// A session Signal. `n_echo` is the receiver's current or previous nonce,
/// so a captured Signal goes stale within two nonce rotations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: SignalKind,
    pub role: Role,
    pub n_self: [u8; 16],
    pub n_echo: [u8; 16],
    pub mac: [u8; 32],
}

impl Signal {
    pub fn encode(&self) -> [u8; SIGNAL_LEN] {
        let mut out = [0u8; SIGNAL_LEN];
        out[..4].copy_from_slice(&header(self.kind.kind_byte(), self.role));
        out[4..20].copy_from_slice(&self.n_self);
        out[20..36].copy_from_slice(&self.n_echo);
        out[36..].copy_from_slice(&self.mac);
        out
    }
}

impl PairSecret {
    /// A Signal from `role` (key `pk_self`) to the peer (key `pk_peer`).
    pub fn signal(
        &self,
        kind: SignalKind,
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        pk_self: &[u8; 32],
        pk_peer: &[u8; 32],
    ) -> Signal {
        let mac = signal_mac(self, kind, role, &n_self, &n_echo, pk_self, pk_peer)
            .finalize()
            .into_bytes()
            .into();
        Signal {
            kind,
            role,
            n_self,
            n_echo,
            mac,
        }
    }
}

fn header(kind: u8, role: Role) -> [u8; 4] {
    [FRAME_TYPE, FRAME_VERSION, kind, role_byte(role)]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Hello(Hello),
    Proof(Proof),
    Signal(Signal),
}

impl Frame {
    /// Syntax only: a decoded frame still has to be verified under `S`.
    pub fn decode(buf: &[u8]) -> Result<Frame, FrameError> {
        if buf.first() != Some(&FRAME_TYPE) {
            return Err(FrameError::NotKnown);
        }
        if buf.len() < 4 {
            return Err(FrameError::Length);
        }
        if buf[1] != FRAME_VERSION {
            return Err(FrameError::Version(buf[1]));
        }
        let kind = buf[2];
        let want = match kind {
            KIND_HELLO => HELLO_LEN,
            KIND_PROOF => PROOF_LEN,
            KIND_BYE | KIND_FRESH => SIGNAL_LEN,
            _ => return Err(FrameError::Kind(kind)),
        };
        if buf.len() != want {
            return Err(FrameError::Length);
        }
        let role = match buf[3] {
            0 => Role::Sharer,
            1 => Role::Joiner,
            r => return Err(FrameError::Role(r)),
        };
        let take16 = |at: usize| -> [u8; 16] { buf[at..at + 16].try_into().unwrap() };
        if kind == KIND_BYE || kind == KIND_FRESH {
            return Ok(Frame::Signal(Signal {
                kind: if kind == KIND_BYE {
                    SignalKind::Bye
                } else {
                    SignalKind::Fresh
                },
                role,
                n_self: take16(4),
                n_echo: take16(20),
                mac: buf[36..].try_into().unwrap(),
            }));
        }
        if kind == KIND_HELLO {
            Ok(Frame::Hello(Hello {
                role,
                nonce: take16(4),
                tag: take16(20),
            }))
        } else {
            Ok(Frame::Proof(Proof {
                role,
                n_self: take16(4),
                n_echo: take16(20),
                port: u16::from_be_bytes([buf[36], buf[37]]),
                peer_addr: buf[38..42].try_into().unwrap(),
                mac: buf[42..].try_into().unwrap(),
            }))
        }
    }
}

/// One side's view of the known-peer exchange: `S`, both public keys, own
/// role and the current/previous Hello nonce (rotated by the caller, ~30 s).
/// The peer's nonce history and first-seen times are tracked separately so a
/// Proof from a restarted peer can be told apart from a normal 30 s rotation
/// (and from a normal rotation whose Hello was lost).
pub struct KnownSession {
    secret: PairSecret,
    role: Role,
    own_pk: [u8; 32],
    peer_pk: [u8; 32],
    nonce: [u8; 16],
    prev_nonce: Option<[u8; 16]>,
    /// The peer's latest nonce, learned from a verified peer Hello or from an
    /// accepted Proof (so a reset is reported once, not every poll).
    peer_nonce: Option<[u8; 16]>,
    peer_prev_nonce: Option<[u8; 16]>,
    /// When the current peer nonce was first observed. A replacement that
    /// arrives less than one rotation interval (minus 5 s) later is
    /// suspicious: a restart, not the normal 30 s rotation.
    peer_nonce_seen_at: Option<Instant>,
    /// The current peer nonce arrived suspiciously early; a valid Proof for it
    /// reports one reset and clears this.
    peer_nonce_suspicious: bool,
    /// A normal rotation has been observed, so a later early replacement is no
    /// longer ambiguous. A track opened mid-rotation can see the peer's first
    /// normal replacement very soon after its first observation; treating that
    /// as a restart would false-drop an idle healthy link.
    peer_nonce_settled: bool,
    /// The port our Proofs announce (the sharer's gateway port).
    own_port: u16,
}

impl KnownSession {
    pub fn new(own: &KeyPair, peer_public_b64: &str, role: Role) -> Result<Self, KnownError> {
        let secret = PairSecret::derive(own, peer_public_b64)?;
        Ok(Self {
            secret,
            role,
            own_pk: own.public_key_bytes(),
            peer_pk: decode_pk(peer_public_b64)?,
            nonce: fresh_nonce(),
            prev_nonce: None,
            peer_nonce: None,
            peer_prev_nonce: None,
            peer_nonce_seen_at: None,
            peer_nonce_suspicious: false,
            peer_nonce_settled: false,
            own_port: LISTEN_PORT,
        })
    }

    /// The port our Proofs announce: the sharer sets the port its gateway
    /// actually bound.
    pub fn set_port(&mut self, port: u16) {
        self.own_port = port;
    }

    /// A version-1 Hello (no port in its Proof era) from the opposite role,
    /// with a tag valid under `S`: the stored peer, running an older Jafnify.
    /// The tag does not cover the version byte, so the check is the same.
    pub fn legacy_hello_is_ours(&self, buf: &[u8]) -> bool {
        if buf.len() != HELLO_LEN
            || buf[0] != FRAME_TYPE
            || (buf[1] != LEGACY_VERSION && buf[1] != LEGACY_V1_VERSION)
        {
            return false;
        }
        if buf[2] != KIND_HELLO {
            return false;
        }
        let role = match buf[3] {
            0 => Role::Sharer,
            1 => Role::Joiner,
            _ => return false,
        };
        if role == self.role {
            return false;
        }
        Hello {
            role,
            nonce: buf[4..20].try_into().unwrap(),
            tag: buf[20..].try_into().unwrap(),
        }
        .verify(&self.secret)
    }

    pub fn pair_id(&self) -> String {
        self.secret.pair_id()
    }

    /// The Hello to multicast now.
    pub fn hello(&self) -> Hello {
        self.secret.hello(self.role, self.nonce)
    }

    /// New nonce; the old one stays acceptable in Proofs for one rotation.
    pub fn rotate(&mut self) {
        self.prev_nonce = Some(self.nonce);
        self.nonce = fresh_nonce();
    }

    fn is_own_nonce(&self, n: &[u8; 16]) -> bool {
        *n == self.nonce || self.prev_nonce.as_ref() == Some(n)
    }

    /// Whether `n` was the peer's latest or one-before-latest nonce.
    fn is_peer_nonce(&self, n: &[u8; 16]) -> bool {
        self.peer_nonce.as_ref() == Some(n) || self.peer_prev_nonce.as_ref() == Some(n)
    }

    /// Remember a new peer nonce observed at `now`. Returns None when it is
    /// already the current or previous nonce. Otherwise returns whether the
    /// replacement was suspicious.
    ///
    /// The first-ever peer nonce is never suspicious. A replacement a full
    /// rotation interval (minus 5 s) or more after the previous nonce is a
    /// normal rotation. An earlier replacement is ambiguous until a normal
    /// rotation has been seen: a track opened mid-rotation can observe the
    /// peer's first normal replacement almost at once. Such an unsettled early
    /// replacement is intentionally treated as that first normal rotation (no
    /// reset); only an early replacement after settling is suspicious.
    fn note_peer_nonce(&mut self, n: [u8; 16], now: Instant) -> Option<bool> {
        if self.is_peer_nonce(&n) {
            return None;
        }
        let normal_gap = NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
        let suspicious = match self.peer_nonce_seen_at {
            // First nonce ever: nothing to reset from.
            None => false,
            Some(seen) if now.saturating_duration_since(seen) >= normal_gap => {
                self.peer_nonce_settled = true;
                false
            }
            // First observed replacement on a track that may have joined
            // mid-rotation: ambiguous, treat as normal.
            Some(_) if !self.peer_nonce_settled => {
                self.peer_nonce_settled = true;
                false
            }
            // A settled track replacing its nonce early is a restart.
            Some(_) => true,
        };
        self.peer_prev_nonce = self.peer_nonce;
        self.peer_nonce = Some(n);
        self.peer_nonce_seen_at = Some(now);
        self.peer_nonce_suspicious = suspicious;
        Some(suspicious)
    }

    /// Classify a valid Proof's sender nonce. The current suspicious nonce
    /// reports one reset; the current non-suspicious or previous nonce reports
    /// none. An unknown nonce is recorded: it reports a reset only when it
    /// arrived suspiciously early (a restart), not after a lost-Hello normal
    /// rotation.
    fn proof_nonce_is_reset(&mut self, n: [u8; 16], now: Instant) -> Option<[u8; 16]> {
        if self.peer_nonce.as_ref() == Some(&n) {
            if self.peer_nonce_suspicious {
                self.peer_nonce_suspicious = false;
                return Some(n);
            }
            return None;
        }
        if self.peer_prev_nonce.as_ref() == Some(&n) {
            return None;
        }
        match self.note_peer_nonce(n, now) {
            Some(true) => {
                self.peer_nonce_suspicious = false;
                Some(n)
            }
            _ => None,
        }
    }

    /// A Hello from the opposite role with a tag valid under `S` is answered
    /// with a Proof echoing its nonce. Own-role and own-nonce frames (our
    /// multicast loopback) and anything unverifiable get no answer.
    pub fn on_hello(&self, hello: &Hello) -> Option<Proof> {
        self.on_hello_from(hello, [127, 0, 0, 1])
    }

    pub fn on_hello_from(&self, hello: &Hello, peer_addr: [u8; 4]) -> Option<Proof> {
        if hello.role == self.role || self.is_own_nonce(&hello.nonce) {
            return None;
        }
        if !hello.verify(&self.secret) {
            return None;
        }
        Some(self.secret.proof_on_with_addr(
            self.role,
            self.nonce,
            hello.nonce,
            &self.own_pk,
            &self.peer_pk,
            ProofTarget {
                port: self.own_port,
                addr: peer_addr,
            },
        ))
    }

    /// A version-2 Proof (no `peer_addr`) from the stored peer, with a MAC
    /// valid under `S`: the peer runs an older Jafnify.
    pub fn legacy_proof_is_ours(&self, buf: &[u8]) -> bool {
        if buf.len() != 70
            || buf[0] != FRAME_TYPE
            || buf[1] != LEGACY_VERSION
            || buf[2] != KIND_PROOF
        {
            return false;
        }
        let role = match buf[3] {
            0 => Role::Sharer,
            1 => Role::Joiner,
            _ => return false,
        };
        if role == self.role {
            return false;
        }
        let n_self: [u8; 16] = buf[4..20].try_into().unwrap();
        let n_echo: [u8; 16] = buf[20..36].try_into().unwrap();
        let port = u16::from_be_bytes([buf[36], buf[37]]);
        let mut mac = mac_for(&self.secret);
        mac.update(b"P");
        mac.update(&[role_byte(role)]);
        mac.update(&n_self);
        mac.update(&n_echo);
        mac.update(&self.peer_pk);
        mac.update(&self.own_pk);
        mac.update(&port.to_be_bytes());
        mac.verify_slice(&buf[38..]).is_ok()
    }

    /// A Signal to the peer, sent under `n_self` and echoing the peer's latest
    /// nonce. None until a peer nonce is known (nothing to address it to).
    fn signal(&self, kind: SignalKind, n_self: [u8; 16]) -> Option<Signal> {
        let echo = self.peer_nonce?;
        Some(
            self.secret
                .signal(kind, self.role, n_self, echo, &self.own_pk, &self.peer_pk),
        )
    }

    /// A Signal counts only if it is from the opposite role, echoes our
    /// current or previous nonce, comes from the peer's current or previous
    /// nonce, and its MAC (constant-time) verifies with the sender's key as
    /// `pk_self` and ours as `pk_peer`.
    pub fn accept_signal(&self, sig: &Signal) -> bool {
        if sig.role == self.role
            || !self.is_own_nonce(&sig.n_echo)
            || self.is_own_nonce(&sig.n_self)
            || !self.is_peer_nonce(&sig.n_self)
        {
            return false;
        }
        signal_mac(
            &self.secret,
            sig.kind,
            sig.role,
            &sig.n_self,
            &sig.n_echo,
            &self.peer_pk,
            &self.own_pk,
        )
        .verify_slice(&sig.mac)
        .is_ok()
    }

    /// A Proof that echoes our CURRENT Hello nonce: the peer answered the
    /// Hello we sent last, so it holds the paired key and is live now. An
    /// echo of an older nonce is a delayed answer to an earlier Hello and
    /// proves nothing about the peer's session. A Hello never proves it.
    pub fn proof_answers_current_hello(&self, proof: &Proof) -> bool {
        proof.n_echo == self.nonce && self.accept_proof(proof)
    }

    /// A Proof counts only if it is from the opposite role, echoes our
    /// current or previous nonce, does not reuse our own nonce as its
    /// sender nonce, and its MAC (constant-time) verifies with the sender's
    /// key as `pk_self` and ours as `pk_peer`.
    pub fn accept_proof(&self, proof: &Proof) -> bool {
        if proof.role == self.role
            || !self.is_own_nonce(&proof.n_echo)
            || self.is_own_nonce(&proof.n_self)
        {
            return false;
        }
        proof_mac(
            &self.secret,
            proof.role,
            &proof.n_self,
            &proof.n_echo,
            &self.peer_pk,
            &self.own_pk,
            &ProofTarget {
                port: proof.port,
                addr: proof.peer_addr,
            },
        )
        .verify_slice(&proof.mac)
        .is_ok()
    }
}

fn ipv4_addr(addr: SocketAddr) -> [u8; 4] {
    match addr {
        SocketAddr::V4(v4) => v4.ip().octets(),
        SocketAddr::V6(_) => [0u8; 4],
    }
}

fn fresh_nonce() -> [u8; 16] {
    let mut n = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

/// Hello cadence while searching / while connected; nonce rotation period.
pub const HELLO_INTERVAL_SEARCHING: Duration = Duration::from_secs(1);
pub const HELLO_INTERVAL_CONNECTED: Duration = Duration::from_secs(5);
pub const NONCE_ROTATE_INTERVAL: Duration = Duration::from_secs(30);

/// Frames handled per `poll`, so a flood cannot starve the caller's loop.
const MAX_FRAMES_PER_POLL: usize = 32;

/// The 0x03 endpoint of the process's 9799 socket (same group and port as
/// 0x01/0x02; only 0x03 frames reach it).
pub struct KnownSocket {
    ep: Endpoint,
    /// Test seam: when set, every send fails with this kind.
    send_fault: Option<std::io::ErrorKind>,
}

impl KnownSocket {
    pub fn bind(cfg: &DiscoveryConfig) -> Result<Self, KnownError> {
        let ep =
            Endpoint::open(cfg, Accept::Known).map_err(|e| KnownError::Socket(e.to_string()))?;
        Ok(Self {
            ep,
            send_fault: None,
        })
    }

    pub fn send(&self, frame: &[u8]) -> Result<(), KnownError> {
        let sent = match self.send_fault {
            Some(kind) => Err(std::io::Error::from(kind)),
            None => self.ep.send(frame),
        };
        sent.map_err(|e| {
            if is_no_network_kind(e.kind()) {
                KnownError::NoNetwork(e.to_string())
            } else {
                KnownError::Socket(e.to_string())
            }
        })
    }

    /// Test seam: make every later send fail with `kind` (None restores).
    #[doc(hidden)]
    pub fn fail_sends(&mut self, kind: Option<std::io::ErrorKind>) {
        self.send_fault = kind;
    }

    /// Remembers a verified peer's address: later frames go to it by unicast.
    pub fn learn(&self, from: SocketAddr) {
        self.ep.learn(from);
    }

    /// Next 0x03 datagram and its source, undecoded, or None on timeout.
    pub fn receive_raw(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, Vec<u8>)>, KnownError> {
        let d = self
            .ep
            .recv(timeout)
            .map_err(|e| KnownError::Socket(e.to_string()))?;
        Ok(d.map(|d| (d.from, d.frame)))
    }

    /// Next well-formed 0x03 frame and its source, or None on timeout.
    pub fn receive(&self, timeout: Duration) -> Result<Option<(SocketAddr, Frame)>, KnownError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some((from, bytes)) = self.receive_raw(left)? else {
                return Ok(None);
            };
            if let Ok(frame) = Frame::decode(&bytes) {
                return Ok(Some((from, frame)));
            }
        }
    }
}

/// What one `poll_detailed` learned. `endpoint` is the peer's WireGuard
/// endpoint from a verified Proof; `session_reset` carries the sender nonce of
/// a valid Proof that arrived suspiciously early (a restarted peer), at most
/// once per nonce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnownPoll {
    pub endpoint: Option<SocketAddr>,
    pub session_reset: Option<[u8; 16]>,
    /// The peer said Bye: it is stopping its session.
    pub peer_bye: bool,
    /// While connected, the peer announced a session newer than the one this
    /// track connected with: the peer restarted, so this side's session
    /// (clock, counters) starts again even if data never stopped.
    pub peer_restarted: bool,
    /// A Fresh from a peer session this track is not connected with arrived
    /// (in any state): the peer's WireGuard state is new, so a tunnel kept
    /// from before holds stale session keys and must be set up again.
    pub peer_fresh: bool,
    /// The peer answered the Hello this side sent last with a Proof that
    /// verifies under the pair secret: it holds the paired key and is live
    /// now, so its session is newer than the one this side holds. Only a
    /// track that asked for it reports this; the rules of a healthy known
    /// session stay unchanged.
    pub peer_new_session: bool,
}

/// One known-peer search, driven by the caller's loop: sends Hellos, answers
/// the peer's, rotates the nonce, and reports the source of an accepted Proof.
pub struct KnownTrack {
    session: KnownSession,
    socket: KnownSocket,
    own_addrs: Vec<std::net::Ipv4Addr>,
    connected: bool,
    last_hello: Option<Instant>,
    rotated_at: Instant,
    old_peer: bool,
    /// Set while sends fail because this device has no usable network (the
    /// error text); cleared by the next send that goes out.
    no_network: Option<String>,
    /// Fresh copies still to send after accepted Proofs.
    fresh_left: u8,
    /// The own nonce the Fresh copies carry (fixed at the first send).
    fresh_nonce: Option<[u8; 16]>,
    /// Peer nonces that belong to the session this track is connected with.
    session_nonces: Vec<[u8; 16]>,
    /// Report a new session of the paired device (see
    /// [`KnownTrack::watch_new_session`]).
    watch_new_session: bool,
}

impl KnownTrack {
    pub fn open(
        cfg: &DiscoveryConfig,
        own: &KeyPair,
        peer_public_b64: &str,
        role: Role,
        now: Instant,
    ) -> Result<Self, KnownError> {
        Ok(Self {
            session: KnownSession::new(own, peer_public_b64, role)?,
            socket: KnownSocket::bind(cfg)?,
            own_addrs: cfg.interfaces(),
            connected: false,
            no_network: None,
            last_hello: None,
            rotated_at: now,
            old_peer: false,
            fresh_left: FRESH_COPIES,
            fresh_nonce: None,
            session_nonces: Vec::new(),
            watch_new_session: false,
        })
    }

    /// Report a new session of the paired device: a Proof that answers the
    /// Hello this side sent last sets `peer_new_session` in the poll.
    ///
    /// For a side that holds no session with the peer (the code track
    /// watching for a paired device that came back), every verified Proof
    /// proves the peer is live now. A side that is already in a known session
    /// keeps its own rules: a normal nonce rotation is not a new session.
    pub fn watch_new_session(&mut self) {
        self.watch_new_session = true;
    }

    /// This side keeps its WireGuard state: the peer must not set up its
    /// tunnel again because of a Fresh from here.
    pub fn suppress_fresh(&mut self) {
        self.fresh_left = 0;
    }

    /// The port our Proofs announce (the sharer's gateway port).
    pub fn set_own_port(&mut self, port: u16) {
        self.session.set_port(port);
    }

    /// A version-1 Hello of the stored peer arrived with a valid tag: the peer
    /// runs an older Jafnify. Sticky for this track.
    pub fn old_peer_seen(&self) -> bool {
        self.old_peer
    }

    /// Connected tracks Hello every 5 s instead of every 1 s. Becoming
    /// connected records the peer nonces of this session, so a later Fresh
    /// from a new peer session can be told apart from this one.
    /// While this device has no usable network: the send error text. A poll
    /// does not fail for it; the caller decides (keep searching, or drop a
    /// connected session). Cleared by the next send that goes out.
    pub fn no_network(&self) -> Option<&str> {
        self.no_network.as_deref()
    }

    /// Test seam: make every later send fail with `kind` (None restores).
    #[doc(hidden)]
    pub fn fail_sends(&mut self, kind: Option<std::io::ErrorKind>) {
        self.socket.fail_sends(kind);
    }

    /// One send; "no network" is recorded instead of failing the poll.
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), KnownError> {
        match self.socket.send(frame) {
            Ok(()) => {
                self.no_network = None;
                Ok(())
            }
            Err(KnownError::NoNetwork(text)) => {
                self.no_network = Some(text);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn set_connected(&mut self, connected: bool) {
        if connected && !self.connected {
            self.session_nonces = [self.session.peer_nonce, self.session.peer_prev_nonce]
                .into_iter()
                .flatten()
                .collect();
        } else if !connected {
            self.session_nonces.clear();
        }
        self.connected = connected;
    }

    /// Tell the peer this side is stopping. Best effort: a few copies, no
    /// wait for an answer, nothing sent before a peer nonce is known. Sent
    /// under the current and the previous nonce: right after a rotation the
    /// peer may not have seen a Hello with the new one yet.
    pub fn send_bye(&mut self) -> Result<(), KnownError> {
        let nonces = [Some(self.session.nonce), self.session.prev_nonce];
        for n_self in nonces.into_iter().flatten() {
            let Some(bye) = self.session.signal(SignalKind::Bye, n_self) else {
                return Ok(());
            };
            let frame = bye.encode();
            for _ in 0..BYE_COPIES {
                self.socket.send(&frame)?;
            }
        }
        Ok(())
    }

    /// The peer's WireGuard endpoint (its Proof's source address with the port
    /// the Proof announced) once a verified Proof arrived.
    pub fn poll(&mut self, now: Instant) -> Result<Option<SocketAddr>, KnownError> {
        Ok(self.poll_detailed(now)?.endpoint)
    }

    /// `poll` plus evidence that the peer's known-peer session was restarted:
    /// a valid Proof whose nonce arrived suspiciously early (well inside one
    /// rotation interval). All Hellos are processed before any Proof so a
    /// normal rotation seen in the same poll sets its timing first.
    pub fn poll_detailed(&mut self, now: Instant) -> Result<KnownPoll, KnownError> {
        if now.saturating_duration_since(self.rotated_at) >= NONCE_ROTATE_INTERVAL {
            self.session.rotate();
            self.rotated_at = now;
        }
        let every = if self.connected {
            HELLO_INTERVAL_CONNECTED
        } else {
            HELLO_INTERVAL_SEARCHING
        };
        if self
            .last_hello
            .is_none_or(|t| now.saturating_duration_since(t) >= every)
        {
            // Counted as sent even when the send fails: with no network the
            // next attempt waits for the Hello pace instead of every tick.
            self.last_hello = Some(now);
            let hello = self.session.hello().encode();
            self.send_frame(&hello)?;
        }
        // Bounded, but collect first: the peer's Hello must teach us its new
        // nonce before its Proof is classified.
        let mut frames = Vec::new();
        for _ in 0..MAX_FRAMES_PER_POLL {
            let Some((from, bytes)) = self.socket.receive_raw(Duration::ZERO)? else {
                break;
            };
            match Frame::decode(&bytes) {
                Ok(frame) => frames.push((from, frame)),
                Err(_) => {
                    if self.session.legacy_hello_is_ours(&bytes)
                        || self.session.legacy_proof_is_ours(&bytes)
                    {
                        self.old_peer = true;
                    }
                }
            }
        }
        for (from, frame) in &frames {
            if let Frame::Hello(h) = frame {
                if let Some(proof) = self.session.on_hello_from(h, ipv4_addr(*from)) {
                    // A Hello teaches the new nonce (and its suspicion) but is
                    // never reset evidence by itself.
                    self.session.note_peer_nonce(h.nonce, now);
                    self.socket.learn(*from);
                    self.send_frame(&proof.encode())?;
                }
            }
        }
        let mut poll = KnownPoll::default();
        for (from, frame) in &frames {
            if let Frame::Proof(p) = frame {
                let peer_addr_is_ours = self.own_addrs.iter().any(|a| a.octets() == p.peer_addr);
                if peer_addr_is_ours && self.session.accept_proof(p) {
                    if poll.endpoint.is_none() {
                        self.socket.learn(*from);
                        poll.endpoint = Some(SocketAddr::new(from.ip(), p.port));
                    }
                    // A Proof that answers the Hello we sent last: the peer
                    // holds the paired key and is live now.
                    if self.watch_new_session && self.session.proof_answers_current_hello(p) {
                        poll.peer_new_session = true;
                    }
                    // Classify every accepted Proof so the nonce set stays
                    // current; only the first reset in a poll is reported.
                    let reset = self.session.proof_nonce_is_reset(p.n_self, now);
                    if poll.session_reset.is_none() {
                        poll.session_reset = reset;
                    }
                }
            }
        }
        // The peer accepted us and we accepted it: tell it this is a new
        // session, a few times across the next Proofs.
        if poll.endpoint.is_some() && self.fresh_left > 0 {
            let n_self = *self.fresh_nonce.get_or_insert(self.session.nonce);
            if let Some(fresh) = self.session.signal(SignalKind::Fresh, n_self) {
                self.send_frame(&fresh.encode())?;
                self.fresh_left -= 1;
            }
        }
        for (_, frame) in &frames {
            let Frame::Signal(sig) = frame else {
                continue;
            };
            if !self.session.accept_signal(sig) {
                continue;
            }
            match sig.kind {
                SignalKind::Bye => poll.peer_bye = true,
                SignalKind::Fresh => {
                    if !self.connected {
                        poll.peer_fresh = true;
                        continue;
                    }
                    if self.session_nonces.is_empty() {
                        // Connected on data before any peer nonce was known:
                        // this Fresh belongs to the same session.
                        self.session_nonces.push(sig.n_self);
                    } else if !self.session_nonces.contains(&sig.n_self) {
                        self.session_nonces.push(sig.n_self);
                        poll.peer_restarted = true;
                        poll.peer_fresh = true;
                    }
                }
            }
        }
        Ok(poll)
    }

    /// Send one Hello immediately (the explicit nudge after a short data
    /// silence). Same frame as the regular cadence; only the timestamp moves.
    pub fn probe(&mut self, now: Instant) -> Result<(), KnownError> {
        self.last_hello = Some(now);
        let hello = self.session.hello().encode();
        self.send_frame(&hello)?;
        Ok(())
    }
}

/// The record persisted after a code pairing's first rx > 0: this pair's own
/// private key, the peer's public key, a hint endpoint. `role` is the PEER's
/// role (the opposite of `own_role`). Secret: holds a private key.
pub fn pair_record(
    own: &KeyPair,
    peer_public_b64: &str,
    own_role: Role,
    last_endpoint: Option<String>,
    now_unix: u64,
) -> Result<Peer, KnownError> {
    let id = PairSecret::derive(own, peer_public_b64)?.pair_id();
    let peer_role = match own_role {
        Role::Sharer => Role::Joiner,
        Role::Joiner => Role::Sharer,
    };
    Ok(Peer {
        name: display_name(peer_role, &id),
        id,
        public_key: peer_public_b64.to_string(),
        role: peer_role,
        last_connected: Some(now_unix),
        private_key: Some(own.private_key_b64()),
        last_endpoint,
    })
}

/// v1 keeps one known peer per device: the first record that has a private
/// key (a record without one is not paired).
pub fn stored_pair(peers: &[Peer]) -> Option<&Peer> {
    peers.iter().find(|p| p.private_key.is_some())
}
