//! SPAKE2 pairing over the shared 9799 socket (frames 0x05/0x06).
//!
//! The 4-digit code is the SPAKE2 password. The Hello carries a fresh nonce
//! and the SPAKE2 share; the Confirm authenticates the WireGuard public key
//! and UDP port under the derived key. A captured transcript cannot be used
//! to test a code offline, and an active guess is bounded by the per-source
//! limiter and the one-Hello-per-session rule.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::discovery::{DiscoveryConfig, DiscoveryError};
use crate::keyexchange::KeyPair;
use crate::pairing::PairingCode;
use crate::state::Role;
use crate::wire::{Accept, Datagram, Endpoint, Path};
use crate::ANNOUNCE_INTERVAL;

type HmacSha256 = Hmac<Sha256>;

pub const PAKE_VERSION: u8 = 2;
pub const HELLO_FRAME: u8 = 0x05;
pub const CONFIRM_FRAME: u8 = 0x06;
pub const HELLO_LEN: usize = 52;
pub const CONFIRM_LEN: usize = 101;

/// PAKE frames allowed from one IPv4 source in `PAKE_RATE_WINDOW`.
pub const PAKE_FRAME_LIMIT: u32 = 10;
pub const PAKE_RATE_WINDOW: Duration = Duration::from_secs(10);
/// How often a Confirm is sent again to Hellos of the already accepted peer.
pub const LATE_CONFIRM_LIMIT: u8 = 5;

const JOINER_ID: &[u8] = b"jafnify/v2/joiner";
const SHARER_ID: &[u8] = b"jafnify/v2/sharer";
const CONFIRM_KEY_LABEL: &[u8] = b"jafnify/v2/confirm-key";
const CONFIRM_LABEL: &[u8] = b"jafnify/v2/confirm";
const REPLAY_TTL: Duration = Duration::from_secs(60);
const REPLAY_CAP: usize = 64;
const MAX_FRAMES_PER_POLL: usize = 32;
const SIDE_A: u8 = 0x41;
const SIDE_B: u8 = 0x42;

fn role_byte(role: Role) -> u8 {
    match role {
        Role::Sharer => 0,
        Role::Joiner => 1,
    }
}

fn role_from_byte(b: u8) -> Option<Role> {
    match b {
        0 => Some(Role::Sharer),
        1 => Some(Role::Joiner),
        _ => None,
    }
}

fn fresh_nonce() -> [u8; 16] {
    let mut n = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PakeFrameError {
    #[error("not a PAKE frame")]
    NotPake,
    #[error("bad frame length")]
    Length,
    #[error("unsupported version {0}")]
    Version(u8),
    #[error("unknown role {0}")]
    Role(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PakeHello {
    pub role: Role,
    pub nonce: [u8; 16],
    pub share: [u8; 33],
}

impl PakeHello {
    pub fn encode(&self) -> [u8; HELLO_LEN] {
        let mut out = [0u8; HELLO_LEN];
        out[0] = HELLO_FRAME;
        out[1] = PAKE_VERSION;
        out[2] = role_byte(self.role);
        out[3..19].copy_from_slice(&self.nonce);
        out[19..].copy_from_slice(&self.share);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PakeFrameError> {
        if buf.first() != Some(&HELLO_FRAME) {
            return Err(PakeFrameError::NotPake);
        }
        if buf.len() < 3 {
            return Err(PakeFrameError::Length);
        }
        if buf[1] != PAKE_VERSION {
            return Err(PakeFrameError::Version(buf[1]));
        }
        if buf.len() != HELLO_LEN {
            return Err(PakeFrameError::Length);
        }
        let role = role_from_byte(buf[2]).ok_or(PakeFrameError::Role(buf[2]))?;
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&buf[3..19]);
        let mut share = [0u8; 33];
        share.copy_from_slice(&buf[19..]);
        Ok(Self { role, nonce, share })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PakeConfirm {
    pub role: Role,
    pub n_self: [u8; 16],
    pub n_echo: [u8; 16],
    pub public_key: [u8; 32],
    pub port: u16,
    pub mac: [u8; 32],
}

impl PakeConfirm {
    pub fn new(
        role: Role,
        n_self: [u8; 16],
        n_echo: [u8; 16],
        public_key: [u8; 32],
        port: u16,
        key: &[u8; 32],
    ) -> Self {
        let mut confirm = Self {
            role,
            n_self,
            n_echo,
            public_key,
            port,
            mac: [0u8; 32],
        };
        confirm.mac = confirm.compute_mac(key);
        confirm
    }

    fn mac_input(&self) -> impl FnOnce(&mut HmacSha256) + '_ {
        move |mac: &mut HmacSha256| {
            mac.update(CONFIRM_LABEL);
            mac.update(&[PAKE_VERSION]);
            mac.update(&[role_byte(self.role)]);
            mac.update(&self.n_self);
            mac.update(&self.n_echo);
            mac.update(&self.public_key);
            mac.update(&self.port.to_be_bytes());
        }
    }

    fn mac_for(&self, key: &[u8; 32]) -> HmacSha256 {
        let mut key_hmac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        key_hmac.update(CONFIRM_KEY_LABEL);
        let mac_key = key_hmac.finalize().into_bytes();
        let mut mac = HmacSha256::new_from_slice(&mac_key).expect("HMAC accepts any key length");
        self.mac_input()(&mut mac);
        mac
    }

    fn compute_mac(&self, key: &[u8; 32]) -> [u8; 32] {
        self.mac_for(key).finalize().into_bytes().into()
    }

    pub fn verify_with_key(&self, key: &[u8; 32]) -> bool {
        self.mac_for(key).verify_slice(&self.mac).is_ok()
    }

    pub fn encode(&self) -> [u8; CONFIRM_LEN] {
        let mut out = [0u8; CONFIRM_LEN];
        out[0] = CONFIRM_FRAME;
        out[1] = PAKE_VERSION;
        out[2] = role_byte(self.role);
        out[3..19].copy_from_slice(&self.n_self);
        out[19..35].copy_from_slice(&self.n_echo);
        out[35..67].copy_from_slice(&self.public_key);
        out[67..69].copy_from_slice(&self.port.to_be_bytes());
        out[69..].copy_from_slice(&self.mac);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PakeFrameError> {
        if buf.first() != Some(&CONFIRM_FRAME) {
            return Err(PakeFrameError::NotPake);
        }
        if buf.len() < 3 {
            return Err(PakeFrameError::Length);
        }
        if buf[1] != PAKE_VERSION {
            return Err(PakeFrameError::Version(buf[1]));
        }
        if buf.len() != CONFIRM_LEN {
            return Err(PakeFrameError::Length);
        }
        let role = role_from_byte(buf[2]).ok_or(PakeFrameError::Role(buf[2]))?;
        let mut n_self = [0u8; 16];
        n_self.copy_from_slice(&buf[3..19]);
        let mut n_echo = [0u8; 16];
        n_echo.copy_from_slice(&buf[19..35]);
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&buf[35..67]);
        let port = u16::from_be_bytes([buf[67], buf[68]]);
        let mut mac = [0u8; 32];
        mac.copy_from_slice(&buf[69..]);
        Ok(Self {
            role,
            n_self,
            n_echo,
            public_key,
            port,
            mac,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PakePeer {
    pub public_key: [u8; 32],
    pub endpoint: SocketAddr,
    /// How the peer's Hello reached us (multicast or unicast).
    pub path: Path,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PakePoll {
    pub peer: Option<PakePeer>,
    pub old_peer: bool,
    pub wrong_code: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PakeError {
    #[error("pake socket error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
}

#[derive(Debug)]
struct PeerHello {
    nonce: [u8; 16],
    addr: SocketAddr,
    path: Path,
}

type ReplayEntry = (([u8; 16], [u8; 16]), Instant);

struct ReplayCache {
    seen: VecDeque<ReplayEntry>,
}

impl ReplayCache {
    fn new() -> Self {
        Self {
            seen: VecDeque::new(),
        }
    }

    fn accept(&mut self, pair: ([u8; 16], [u8; 16]), now: Instant) -> bool {
        self.seen
            .retain(|(_, at)| now.saturating_duration_since(*at) < REPLAY_TTL);
        if self.seen.iter().any(|(p, _)| *p == pair) {
            return false;
        }
        if self.seen.len() == REPLAY_CAP {
            self.seen.pop_front();
        }
        self.seen.push_back((pair, now));
        true
    }
}

struct PerSourceLimiter {
    seen: HashMap<Ipv4Addr, (Instant, u32)>,
}

impl PerSourceLimiter {
    fn new() -> Self {
        Self {
            seen: HashMap::new(),
        }
    }

    fn allow(&mut self, source: Ipv4Addr, now: Instant) -> bool {
        if self.seen.len() > 64 {
            self.seen
                .retain(|_, (at, _)| now.saturating_duration_since(*at) <= PAKE_RATE_WINDOW);
        }
        let entry = self.seen.entry(source).or_insert((now, 0));
        if now.saturating_duration_since(entry.0) > PAKE_RATE_WINDOW {
            *entry = (now, 1);
            true
        } else {
            entry.1 = entry.1.saturating_add(1);
            entry.1 <= PAKE_FRAME_LIMIT
        }
    }
}

pub struct PakeLink {
    ep: Endpoint,
    role: Role,
    own_key: [u8; 32],
    own_port: u16,
    own_nonce: [u8; 16],
    spake: Option<Spake2<Ed25519Group>>,
    own_share: [u8; 33],
    peer: Option<PeerHello>,
    key: Option<[u8; 32]>,
    own_confirm: Option<PakeConfirm>,
    accepted: Option<PakePeer>,
    /// Confirms resent to the accepted peer's repeated Hellos.
    late_confirms: u8,
    old_peer: bool,
    wrong_code_seen: bool,
    /// Opposite-role Hellos accepted for processing; test/diagnostic counter.
    hellos_seen: u64,
    /// Hellos we sent; test/diagnostic counter.
    hellos_sent: u64,
    last_hello: Option<Instant>,
    limiter: PerSourceLimiter,
    replay: ReplayCache,
    /// Our own addresses: a Confirm may leave from one of them.
    own_addrs: Vec<Ipv4Addr>,
}

impl PakeLink {
    pub fn open(
        cfg: &DiscoveryConfig,
        role: Role,
        code: &PairingCode,
        own: &KeyPair,
        own_port: u16,
    ) -> Result<Self, PakeError> {
        if cfg.interfaces().is_empty() {
            return Err(DiscoveryError::NoLanInterface.into());
        }
        let ep = Endpoint::open(cfg, Accept::Pake)?;
        let password = code.as_str().as_bytes().to_vec();
        let id_a = Identity::new(JOINER_ID);
        let id_b = Identity::new(SHARER_ID);
        let (spake, share) = match role {
            Role::Joiner => {
                Spake2::<Ed25519Group>::start_a(&Password::new(&password), &id_a, &id_b)
            }
            Role::Sharer => {
                Spake2::<Ed25519Group>::start_b(&Password::new(&password), &id_a, &id_b)
            }
        };
        let mut own_share = [0u8; 33];
        own_share.copy_from_slice(&share);
        Ok(Self {
            ep,
            role,
            own_key: own.public_key_bytes(),
            own_port,
            own_nonce: fresh_nonce(),
            spake: Some(spake),
            own_share,
            peer: None,
            key: None,
            own_confirm: None,
            accepted: None,
            late_confirms: 0,
            old_peer: false,
            wrong_code_seen: false,
            hellos_seen: 0,
            hellos_sent: 0,
            last_hello: None,
            limiter: PerSourceLimiter::new(),
            replay: ReplayCache::new(),
            own_addrs: cfg.interfaces(),
        })
    }

    pub fn hello(&self) -> PakeHello {
        PakeHello {
            role: self.role,
            nonce: self.own_nonce,
            share: self.own_share,
        }
    }

    pub fn own_confirm(&self) -> Option<PakeConfirm> {
        self.own_confirm.clone()
    }

    pub fn accepted(&self) -> Option<PakePeer> {
        self.accepted
    }

    pub fn old_peer_seen(&self) -> bool {
        self.old_peer
    }

    /// Opposite-role Hellos this link has processed. Used by integration tests
    /// to check retransmission without exposing frame contents.
    pub fn hellos_seen(&self) -> u64 {
        self.hellos_seen
    }

    /// Hellos this link has sent. Used by integration tests to check the
    /// retransmission without exposing frame contents.
    pub fn hellos_sent(&self) -> u64 {
        self.hellos_sent
    }

    pub fn poll(&mut self, now: Instant) -> Result<PakePoll, PakeError> {
        let mut poll = PakePoll {
            peer: self.accepted,
            old_peer: self.old_peer,
            wrong_code: self.wrong_code_seen,
        };
        // The Hello is also liveness: it keeps flowing while the tunnel is
        // up but no data has arrived yet.
        if self
            .last_hello
            .is_none_or(|t| now.saturating_duration_since(t) >= ANNOUNCE_INTERVAL)
        {
            self.ep.send(&self.hello().encode())?;
            self.hellos_sent = self.hellos_sent.saturating_add(1);
            self.last_hello = Some(now);
        }
        // The frames keep being drained after the peer was accepted: the
        // Hello is the liveness of the attempt, the session itself is done.
        for _ in 0..MAX_FRAMES_PER_POLL {
            let Some(datagram) = self.ep.recv(Duration::ZERO)? else {
                break;
            };
            self.handle(datagram, now, &mut poll);
        }
        Ok(poll)
    }

    fn handle(&mut self, datagram: Datagram, now: Instant, poll: &mut PakePoll) {
        let SocketAddr::V4(from) = datagram.from else {
            return;
        };
        // Our own frames come back through the multicast loop. They must not
        // spend the budget of the per-source limiter.
        if datagram.frame.len() >= 3
            && datagram.frame[1] == PAKE_VERSION
            && datagram.frame[2] == role_byte(self.role)
        {
            return;
        }
        // The peer of an established session is not junk: its liveness Hellos
        // and its Confirm must not spend the budget of the limiter.
        let session_source = self.key.is_some()
            && self
                .peer
                .as_ref()
                .is_some_and(|peer| peer.addr.ip() == *from.ip());
        match datagram.frame.first().copied() {
            Some(HELLO_FRAME) => match PakeHello::decode(&datagram.frame) {
                Ok(hello) => {
                    if !session_source && !self.limiter.allow(*from.ip(), now) {
                        return;
                    }
                    if hello.role != self.role {
                        self.hellos_seen = self.hellos_seen.saturating_add(1);
                    }
                    if self.accepted.is_some() {
                        self.resend_confirm_after_accept(&hello, datagram.from);
                        return;
                    }
                    self.on_hello(hello, datagram.from, datagram.path);
                }
                Err(PakeFrameError::Version(_)) => {
                    self.limiter.allow(*from.ip(), now);
                    self.old_peer = true;
                    poll.old_peer = true;
                }
                Err(_) => {
                    self.limiter.allow(*from.ip(), now);
                }
            },
            Some(CONFIRM_FRAME) => {
                if self.accepted.is_some() {
                    return;
                }
                match PakeConfirm::decode(&datagram.frame) {
                    Ok(confirm) => {
                        // A Confirm that does not verify is junk (a wrong
                        // code, a replay, a stranger): it spends the budget.
                        if !self.on_confirm(confirm, datagram.from, now, poll) {
                            self.limiter.allow(*from.ip(), now);
                        }
                    }
                    Err(PakeFrameError::Version(_)) => {
                        self.limiter.allow(*from.ip(), now);
                        self.old_peer = true;
                        poll.old_peer = true;
                    }
                    Err(_) => {
                        self.limiter.allow(*from.ip(), now);
                    }
                }
            }
            _ => {}
        }
    }

    /// Our Confirm is sent once per Hello of the session peer. If the peer
    /// had not processed our Hello yet when the first Confirm arrived, it
    /// dropped that Confirm; when we then accepted the peer's Confirm first,
    /// its repeated Hellos were ignored and it waited for ever. A Hello that
    /// still arrives from the accepted peer means exactly that, so the same
    /// Confirm is sent again, at most [`LATE_CONFIRM_LIMIT`] times. No state
    /// changes and nothing new is revealed: it is the frame already sent.
    fn resend_confirm_after_accept(&mut self, hello: &PakeHello, from: SocketAddr) {
        if hello.role == self.role || self.late_confirms >= LATE_CONFIRM_LIMIT {
            return;
        }
        let Some(peer) = &self.peer else {
            return;
        };
        if peer.nonce != hello.nonce || peer.addr.ip() != from.ip() {
            return;
        }
        if let Some(confirm) = &self.own_confirm {
            self.late_confirms += 1;
            let _ = self.ep.send_to(&confirm.encode(), from);
        }
    }

    fn on_hello(&mut self, hello: PakeHello, from: SocketAddr, path: Path) {
        if hello.role == self.role || hello.nonce == self.own_nonce {
            return;
        }
        if let Some(peer) = &self.peer {
            if peer.nonce == hello.nonce && peer.addr.ip() == from.ip() {
                if let Some(confirm) = &self.own_confirm {
                    let _ = self.ep.send_to(&confirm.encode(), from);
                }
            }
            return;
        }
        if self.key.is_some() {
            return;
        }
        let expected_side = match self.role {
            Role::Joiner => SIDE_B,
            Role::Sharer => SIDE_A,
        };
        if hello.share[0] != expected_side {
            return;
        }
        let Some(spake) = self.spake.take() else {
            return;
        };
        let Ok(key) = spake.finish(&hello.share) else {
            return;
        };
        let Ok(key) = <[u8; 32]>::try_from(key) else {
            return;
        };
        self.key = Some(key);
        self.peer = Some(PeerHello {
            nonce: hello.nonce,
            addr: from,
            path,
        });
        let confirm = PakeConfirm::new(
            self.role,
            self.own_nonce,
            hello.nonce,
            self.own_key,
            self.own_port,
            &key,
        );
        self.own_confirm = Some(confirm.clone());
        let _ = self.ep.send_to(&confirm.encode(), from);
    }

    /// A Confirm that verifies the key of the session peer. Returns false for
    /// every Confirm that must not be accepted (a wrong code, a replay, a
    /// stranger): the caller counts those as junk.
    fn on_confirm(
        &mut self,
        confirm: PakeConfirm,
        from: SocketAddr,
        now: Instant,
        poll: &mut PakePoll,
    ) -> bool {
        let (Some(peer), Some(key)) = (&self.peer, self.key) else {
            return false;
        };
        if confirm.role == self.role
            || confirm.n_echo != self.own_nonce
            || confirm.n_self != peer.nonce
            || confirm.public_key == self.own_key
            || !self.source_may_be_the_peer(from.ip(), peer.addr.ip())
        {
            return false;
        }
        if !confirm.verify_with_key(&key) {
            self.wrong_code_seen = true;
            poll.wrong_code = true;
            return false;
        }
        if !self.replay.accept((confirm.n_self, confirm.n_echo), now) {
            return false;
        }
        let accepted = PakePeer {
            public_key: confirm.public_key,
            // The peer's address is the one its Hello came from: that is where
            // our Confirm went. The port is under the MAC.
            endpoint: SocketAddr::new(peer.addr.ip(), confirm.port),
            path: peer.path,
        };
        self.accepted = Some(accepted);
        poll.peer = Some(accepted);
        true
    }

    /// Whether a Confirm may come from this source address.
    ///
    /// The rule of the design is "the Confirm comes from the address the
    /// Hello came from". A unicast reply takes its source address from the
    /// route, not from the interface the multicast went out on, so the two
    /// differ whenever the device has more than one address for the peer
    /// (and always on one host, where the reply leaves from the primary
    /// loopback address). The Confirm is still authenticated by the session
    /// MAC, so the address only has to be one this device could have sent
    /// from: the peer's own address, one of our own addresses, or the
    /// loopback (the same-host case).
    fn source_may_be_the_peer(&self, from: IpAddr, peer: IpAddr) -> bool {
        let (IpAddr::V4(from), IpAddr::V4(peer)) = (from, peer) else {
            return false;
        };
        from == peer || from.is_loopback() || self.own_addrs.contains(&from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveryConfig;
    use crate::keyexchange::KeyPair;
    use std::net::Ipv4Addr;

    const TEST_PORT: u16 = 49805;

    fn cfg() -> DiscoveryConfig {
        DiscoveryConfig {
            iface: Some(Ipv4Addr::LOCALHOST),
            addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
            ..Default::default()
        }
    }

    fn link(role: Role, code: &str) -> (PakeLink, KeyPair) {
        link_with(&cfg(), role, code)
    }

    fn link_with(cfg: &DiscoveryConfig, role: Role, code: &str) -> (PakeLink, KeyPair) {
        let key = KeyPair::generate();
        let code = PairingCode::parse(code).unwrap();
        let link = PakeLink::open(cfg, role, &code, &key, 0).unwrap();
        (link, key)
    }

    fn datagram(from: &str, frame: &[u8]) -> Datagram {
        Datagram {
            from: from.parse().unwrap(),
            to: Ipv4Addr::LOCALHOST,
            path: crate::wire::Path::Unicast,
            frame: frame.to_vec(),
        }
    }

    #[test]
    fn junk_hello_with_the_wrong_side_does_not_consume_the_session() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let junk = PakeHello {
            role: Role::Sharer,
            nonce: [1; 16],
            share: [SIDE_A; 33],
        };
        let mut poll = PakePoll::default();
        joiner.handle(datagram("127.0.0.1:9799", &junk.encode()), now, &mut poll);
        assert!(joiner.spake.is_some());
        assert!(joiner.key.is_none());

        joiner.handle(
            datagram("127.0.0.1:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        assert!(joiner.key.is_some());
        assert!(joiner.own_confirm.is_some());
    }

    #[test]
    fn a_confirm_from_a_different_source_is_rejected() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        joiner.handle(
            datagram("127.0.0.1:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        let mut sharer_state = sharer;
        sharer_state.handle(
            datagram("127.0.0.1:9799", &joiner.hello().encode()),
            now,
            &mut poll,
        );
        let own = sharer_state.own_confirm().expect("no confirm");
        // A stranger on the LAN: not the peer, not one of our addresses.
        joiner.handle(datagram("192.168.7.7:9799", &own.encode()), now, &mut poll);
        assert!(joiner.accepted().is_none());
    }

    /// The reply leaves from the address the route picks, which is not always
    /// the address the peer's Hello came from. Such a Confirm is accepted, and
    /// the tunnel still points at the address the Hello came from.
    #[test]
    fn a_confirm_from_another_local_address_is_accepted_and_keeps_the_hello_source() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        joiner.handle(
            datagram("127.0.0.2:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        let mut sharer_state = sharer;
        sharer_state.handle(
            datagram("127.0.0.1:9799", &joiner.hello().encode()),
            now,
            &mut poll,
        );
        let own = sharer_state.own_confirm().expect("no confirm");
        joiner.handle(datagram("127.0.0.1:9799", &own.encode()), now, &mut poll);
        let peer = joiner.accepted().expect("the confirm was rejected");
        assert_eq!(peer.endpoint.ip(), "127.0.0.2".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn a_version_mismatch_reports_an_old_peer() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let now = Instant::now();
        let mut frame = joiner.hello().encode();
        frame[1] = 1;
        let mut poll = PakePoll::default();
        joiner.handle(datagram("127.0.0.1:9799", &frame), now, &mut poll);
        assert!(poll.old_peer);
        assert!(joiner.old_peer_seen() || poll.old_peer);
    }

    #[test]
    fn a_replayed_confirm_is_rejected() {
        let now = Instant::now();
        let mut replay = ReplayCache::new();
        let pair = ([1u8; 16], [2u8; 16]);
        assert!(replay.accept(pair, now));
        assert!(!replay.accept(pair, now));
    }

    #[test]
    fn the_hello_keeps_flowing_after_the_peer_is_accepted() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        joiner.handle(
            datagram("127.0.0.1:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        let mut sharer_state = sharer;
        sharer_state.handle(
            datagram("127.0.0.1:9799", &joiner.hello().encode()),
            now,
            &mut poll,
        );
        let own = sharer_state.own_confirm().expect("no confirm");
        joiner.handle(datagram("127.0.0.1:9799", &own.encode()), now, &mut poll);
        assert!(joiner.accepted().is_some());
        // The Hello is the liveness of the attempt: it keeps flowing while
        // the tunnel is up but no data has arrived yet.
        let before = joiner.ep.stats().tx_packets;
        let _ = joiner.poll(now + ANNOUNCE_INTERVAL + Duration::from_millis(10));
        assert!(
            joiner.ep.stats().tx_packets > before,
            "the Hello stopped after the peer was accepted"
        );
    }

    /// The peer dropped our first Confirm (it had not seen our Hello yet) and
    /// keeps sending its Hello after we accepted its Confirm.
    #[test]
    fn a_hello_from_the_accepted_peer_gets_the_confirm_again_a_few_times() {
        // Endpoints on one port share one socket and one counter: a test
        // running in parallel on TEST_PORT would add its sends to this
        // exact count. This joiner gets a port of its own.
        let own = DiscoveryConfig {
            iface: Some(Ipv4Addr::LOCALHOST),
            addr: "239.255.42.99:49815".parse().unwrap(),
            ..Default::default()
        };
        let (mut joiner, _) = link_with(&own, Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        let sharer_hello = sharer.hello().encode();
        joiner.handle(datagram("127.0.0.1:9799", &sharer_hello), now, &mut poll);
        let mut sharer_state = sharer;
        sharer_state.handle(
            datagram("127.0.0.1:9799", &joiner.hello().encode()),
            now,
            &mut poll,
        );
        let own = sharer_state.own_confirm().expect("no confirm");
        joiner.handle(datagram("127.0.0.1:9799", &own.encode()), now, &mut poll);
        assert!(joiner.accepted().is_some());

        // A stranger's Hello, or one from another address, gets nothing.
        let before = joiner.ep.stats().tx_packets;
        let stranger = PakeHello {
            role: Role::Sharer,
            nonce: [7; 16],
            share: [SIDE_A; 33],
        };
        joiner.handle(
            datagram("127.0.0.1:9799", &stranger.encode()),
            now,
            &mut poll,
        );
        joiner.handle(datagram("192.168.7.7:9799", &sharer_hello), now, &mut poll);
        assert_eq!(joiner.ep.stats().tx_packets, before);

        // The accepted peer's Hello gets the same Confirm, a bounded number
        // of times, and the accepted state does not change.
        let accepted = joiner.accepted();
        for _ in 0..(LATE_CONFIRM_LIMIT as usize + 3) {
            joiner.handle(datagram("127.0.0.1:9799", &sharer_hello), now, &mut poll);
        }
        assert_eq!(
            joiner.ep.stats().tx_packets,
            before + u64::from(LATE_CONFIRM_LIMIT)
        );
        assert_eq!(joiner.accepted(), accepted);
    }

    #[test]
    fn our_own_frames_do_not_spend_the_rate_budget() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        // Our own frames come back through the multicast loop. They must not
        // spend the budget of the per-source limiter.
        for _ in 0..PAKE_FRAME_LIMIT + 2 {
            joiner.handle(
                datagram("127.0.0.1:9799", &joiner.hello().encode()),
                now,
                &mut poll,
            );
        }
        joiner.handle(
            datagram("127.0.0.1:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        assert!(
            joiner.spake.is_none(),
            "the peer's Hello was rate limited by our own frames"
        );
    }

    #[test]
    fn the_session_peer_is_not_rate_limited_and_a_source_without_a_session_is() {
        let (mut joiner, _) = link(Role::Joiner, "1234");
        let (sharer, _) = link(Role::Sharer, "1234");
        let now = Instant::now();
        let mut poll = PakePoll::default();
        joiner.handle(
            datagram("127.0.0.1:9799", &sharer.hello().encode()),
            now,
            &mut poll,
        );
        let sharer_hello = sharer.hello().encode();
        let mut sharer_state = sharer;
        sharer_state.handle(
            datagram("127.0.0.1:9799", &joiner.hello().encode()),
            now,
            &mut poll,
        );
        let own = sharer_state.own_confirm().expect("no confirm");
        joiner.handle(datagram("127.0.0.1:9799", &own.encode()), now, &mut poll);
        assert!(joiner.accepted().is_some());

        // The peer we share a key with keeps sending liveness Hellos: they
        // must never spend the budget of the junk limiter.
        let before = joiner.hellos_seen;
        for _ in 0..PAKE_FRAME_LIMIT + 5 {
            joiner.handle(datagram("127.0.0.1:9799", &sharer_hello), now, &mut poll);
        }
        assert_eq!(
            joiner.hellos_seen,
            before + PAKE_FRAME_LIMIT as u64 + 5,
            "the peer of the session was rate limited"
        );

        // A source we have no session with is limited.
        let before = joiner.hellos_seen;
        for _ in 0..PAKE_FRAME_LIMIT + 5 {
            joiner.handle(datagram("192.168.7.7:9799", &sharer_hello), now, &mut poll);
        }
        assert_eq!(
            joiner.hellos_seen,
            before + PAKE_FRAME_LIMIT as u64,
            "a source with no session was not limited"
        );
    }

    #[test]
    fn the_per_source_limiter_caps_frames() {
        let now = Instant::now();
        let mut limiter = PerSourceLimiter::new();
        let source = Ipv4Addr::new(192, 168, 1, 2);
        for _ in 0..PAKE_FRAME_LIMIT {
            assert!(limiter.allow(source, now));
        }
        assert!(!limiter.allow(source, now));
        assert!(limiter.allow(source, now + PAKE_RATE_WINDOW + Duration::from_secs(1)));
    }
}
