//! A new session of the paired device supersedes a stale one. Only a Proof
//! that answers the Hello this side sent last proves it: the peer holds the
//! paired key and is live now. A bare Hello, a stranger's Proof and a Proof
//! replayed from an earlier rotation never prove it.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{Frame, KnownSocket, KnownTrack, PairSecret};
use jafnify_core::state::Role;

// Tests here share the multicast group; run them one at a time.
static MULTICAST: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

const TEST_PORT: u16 = 49814;

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

/// The watching side: it holds the pair and watches for a new session of the
/// paired device.
fn watcher(own: &KeyPair, peer: &KeyPair, now: Instant) -> KnownTrack {
    KnownTrack::open(&cfg(), own, &peer.public_key_b64(), Role::Sharer, now).unwrap()
}

/// The peer's view of the pair secret: its own private key and ours public.
fn peer_secret(peer: &KeyPair, own: &KeyPair) -> PairSecret {
    PairSecret::derive(peer, &own.public_key_b64()).unwrap()
}

/// Every well-formed 0x03 frame the listener sees in a short window.
fn drain(listener: &KnownSocket) -> Vec<Frame> {
    let end = Instant::now() + Duration::from_millis(150);
    let mut out = Vec::new();
    while Instant::now() < end {
        if let Some((_, f)) = listener.receive(Duration::from_millis(20)).unwrap() {
            out.push(f);
        }
    }
    out
}

/// Poll once so the watcher sends a Hello, and return that Hello's nonce.
fn own_nonce(w: &mut KnownTrack, listener: &KnownSocket, now: Instant) -> [u8; 16] {
    w.poll(now).unwrap();
    drain(listener)
        .into_iter()
        .find_map(|f| match f {
            Frame::Hello(h) if h.role == Role::Sharer => Some(h.nonce),
            _ => None,
        })
        .expect("no Hello of the watcher")
}

fn deliver(listener: &KnownSocket, frame: &[u8]) {
    listener.send(frame).unwrap();
    std::thread::sleep(Duration::from_millis(50));
}

#[test]
fn a_proof_answering_the_current_hello_reports_a_new_session() {
    let _g = lock();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut w = watcher(&own, &peer, t0);
    w.watch_new_session();
    let nonce = own_nonce(&mut w, &listener, t0);

    // The paired device answered the Hello we just sent: it holds the paired
    // key and is live now.
    let secret = peer_secret(&peer, &own);
    let proof = secret.proof_with_addr(
        Role::Joiner,
        [3u8; 16],
        nonce,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        [127, 0, 0, 1],
    );
    deliver(&listener, &proof.encode());

    let poll = w.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert!(
        poll.peer_new_session,
        "a valid Proof did not report the new session"
    );
}

#[test]
fn a_bare_hello_is_not_a_new_session() {
    let _g = lock();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut w = watcher(&own, &peer, t0);
    w.watch_new_session();
    let _nonce = own_nonce(&mut w, &listener, t0);

    // A Hello alone: it carries the peer's nonce, not proof that the peer is
    // there now.
    let secret = peer_secret(&peer, &own);
    deliver(&listener, &secret.hello(Role::Joiner, [4u8; 16]).encode());

    let poll = w.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert!(
        !poll.peer_new_session,
        "a bare Hello reported a new session"
    );
}

#[test]
fn a_proof_from_a_stranger_is_not_a_new_session() {
    let _g = lock();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let stranger = KeyPair::generate();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut w = watcher(&own, &peer, t0);
    w.watch_new_session();
    let nonce = own_nonce(&mut w, &listener, t0);

    // A Proof signed with another pair's secret: the MAC does not verify.
    let other = PairSecret::derive(&stranger, &own.public_key_b64()).unwrap();
    let proof = other.proof_with_addr(
        Role::Joiner,
        [3u8; 16],
        nonce,
        &stranger.public_key_bytes(),
        &own.public_key_bytes(),
        [127, 0, 0, 1],
    );
    deliver(&listener, &proof.encode());

    let poll = w.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert!(
        !poll.peer_new_session,
        "a stranger's Proof reported a new session"
    );
    assert!(poll.endpoint.is_none(), "a stranger's Proof was accepted");
}

#[test]
fn a_proof_replayed_from_the_previous_rotation_is_not_a_new_session() {
    let _g = lock();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut w = watcher(&own, &peer, t0);
    w.watch_new_session();
    let old = own_nonce(&mut w, &listener, t0);

    // The nonce rotates: the Proof captured above echoes the old one.
    w.poll_detailed(t0 + Duration::from_secs(31)).unwrap();

    let secret = peer_secret(&peer, &own);
    let proof = secret.proof_with_addr(
        Role::Joiner,
        [3u8; 16],
        old,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        [127, 0, 0, 1],
    );
    deliver(&listener, &proof.encode());

    let poll = w.poll_detailed(t0 + Duration::from_secs(31)).unwrap();
    assert!(
        !poll.peer_new_session,
        "a replayed Proof reported a new session"
    );
}

#[test]
fn a_normal_track_does_not_report_a_new_session() {
    let _g = lock();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut w = watcher(&own, &peer, t0);
    let nonce = own_nonce(&mut w, &listener, t0);

    let secret = peer_secret(&peer, &own);
    let proof = secret.proof_with_addr(
        Role::Joiner,
        [3u8; 16],
        nonce,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        [127, 0, 0, 1],
    );
    deliver(&listener, &proof.encode());

    let poll = w.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert!(
        !poll.peer_new_session,
        "a track that did not ask for it reported a new session"
    );
}
