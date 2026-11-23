//! Known-peer reconnect, step 2b core part: the shared `KnownTrack` driver
//! (Hello cadence, nonce rotation, Proof handling over loopback multicast),
//! `pair_record` and `stored_pair`. Keys are generated per test.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{
    pair_record, stored_pair, Frame, KnownSession, KnownSocket, KnownTrack, PairSecret, Proof,
    NONCE_ROTATE_INTERVAL,
};
use jafnify_core::names::pair_words;
use jafnify_core::state::{Peer, Role};

// Tests here share the multicast group; run them one at a time.
static MULTICAST: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

const TEST_PORT: u16 = 49802;

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

fn track(own: &KeyPair, peer: &KeyPair, role: Role, now: Instant) -> KnownTrack {
    KnownTrack::open(&cfg(), own, &peer.public_key_b64(), role, now).unwrap()
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

fn hello_nonces(frames: &[Frame], role: Role) -> Vec<[u8; 16]> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Hello(h) if h.role == role => Some(h.nonce),
            _ => None,
        })
        .collect()
}

// ---- cadence ---------------------------------------------------------------

#[test]
fn a_self_echoed_hello_is_never_a_peer() {
    let _g = lock();
    let own = KeyPair::generate();
    let peer = KeyPair::generate();
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = KnownSocket::bind(&cfg()).unwrap();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    drain(&listener);

    // Our own Hello, arriving back on the multicast group. A host with a
    // second network sends the search there and hears itself; the same role
    // and the same key mean this can never be the stored peer.
    let own_hello = KnownSession::new(&own, &peer.public_key_b64(), Role::Joiner)
        .unwrap()
        .hello();
    echo.send(&own_hello.encode()).unwrap();

    let poll = t.poll(t0).unwrap();
    assert!(poll.is_none(), "an own Hello became a peer");

    let frames = drain(&listener);
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::Proof(_))),
        "a self echo was answered: {frames:?}"
    );
}

#[test]
fn hello_goes_out_once_per_second_while_searching() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&a, &b, Role::Joiner, t0);
    drain(&listener);

    t.poll(t0).unwrap();
    assert_eq!(hello_nonces(&drain(&listener), Role::Joiner).len(), 1);
    t.poll(t0 + Duration::from_millis(900)).unwrap();
    assert!(hello_nonces(&drain(&listener), Role::Joiner).is_empty());
    t.poll(t0 + Duration::from_secs(1)).unwrap();
    assert_eq!(hello_nonces(&drain(&listener), Role::Joiner).len(), 1);
}

#[test]
fn hello_slows_to_five_seconds_while_connected() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&a, &b, Role::Sharer, t0);
    t.set_connected(true);
    drain(&listener);

    t.poll(t0).unwrap();
    assert_eq!(hello_nonces(&drain(&listener), Role::Sharer).len(), 1);
    t.poll(t0 + Duration::from_secs(4)).unwrap();
    assert!(hello_nonces(&drain(&listener), Role::Sharer).is_empty());
    t.poll(t0 + Duration::from_secs(5)).unwrap();
    assert_eq!(hello_nonces(&drain(&listener), Role::Sharer).len(), 1);
}

#[test]
fn hello_nonce_rotates_after_thirty_seconds() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&a, &b, Role::Joiner, t0);
    drain(&listener);

    t.poll(t0).unwrap();
    let n1 = hello_nonces(&drain(&listener), Role::Joiner)[0];
    t.poll(t0 + Duration::from_secs(29)).unwrap();
    let still = hello_nonces(&drain(&listener), Role::Joiner)[0];
    assert_eq!(n1, still, "rotated early");
    t.poll(t0 + Duration::from_secs(30)).unwrap();
    let n2 = hello_nonces(&drain(&listener), Role::Joiner)[0];
    assert_ne!(n1, n2, "nonce never rotated");
}

// ---- the exchange ----------------------------------------------------------

#[test]
fn two_tracks_with_the_same_pair_accept_each_others_proof() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut sharer = track(&a, &b, Role::Sharer, t0);
    let mut joiner = track(&b, &a, Role::Joiner, t0);
    let (mut s_found, mut j_found) = (None, None);
    let end = Instant::now() + Duration::from_secs(3);
    while (s_found.is_none() || j_found.is_none()) && Instant::now() < end {
        let now = Instant::now();
        s_found = s_found.or(sharer.poll(now).unwrap());
        j_found = j_found.or(joiner.poll(now).unwrap());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        j_found.expect("joiner got no Proof").ip(),
        Ipv4Addr::LOCALHOST
    );
    assert_eq!(
        s_found.expect("sharer got no Proof").ip(),
        Ipv4Addr::LOCALHOST
    );
}

#[test]
fn a_lone_track_never_accepts_its_own_multicast_loopback() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&a, &b, Role::Joiner, t0);
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end {
        assert_eq!(t.poll(Instant::now()).unwrap(), None);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_stranger_gets_no_proof_and_cannot_be_accepted() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let stranger = KeyPair::generate();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut joiner = track(&b, &a, Role::Joiner, t0);
    // Knows the joiner's public key, but not the pair secret.
    let mut fake_sharer = track(&stranger, &b, Role::Sharer, t0);
    drain(&listener);

    let end = Instant::now() + Duration::from_millis(1500);
    let mut frames = Vec::new();
    while Instant::now() < end {
        let now = Instant::now();
        assert_eq!(
            joiner.poll(now).unwrap(),
            None,
            "joiner accepted a stranger"
        );
        assert_eq!(fake_sharer.poll(now).unwrap(), None);
        frames.extend(drain(&listener));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::Proof(_))),
        "someone answered a Hello without the pair secret"
    );
}

#[test]
fn a_forged_proof_echoing_the_real_nonce_is_not_accepted() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let t0 = Instant::now();
    let mut joiner = track(&b, &a, Role::Joiner, t0);
    drain(&listener);
    joiner.poll(t0).unwrap();
    let nonce = hello_nonces(&drain(&listener), Role::Joiner)[0];

    let forged = Proof {
        role: Role::Sharer,
        n_self: [5u8; 16],
        n_echo: nonce,
        port: 9797,
        peer_addr: [127, 0, 0, 1],
        mac: [9u8; 32],
    };
    listener.send(&forged.encode()).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(joiner.poll(t0 + Duration::from_millis(10)).unwrap(), None);
}

// ---- peer-session reset ----------------------------------------------------

/// The peer's view of the pair secret: its own private key and ours public.
fn peer_secret(peer: &KeyPair, own: &KeyPair) -> PairSecret {
    PairSecret::derive(peer, &own.public_key_b64()).unwrap()
}

/// A valid Sharer Proof for the Joiner track under test.
fn peer_proof(
    secret: &PairSecret,
    peer: &KeyPair,
    own: &KeyPair,
    n_self: [u8; 16],
    n_echo: [u8; 16],
) -> Proof {
    secret.proof_on(
        Role::Sharer,
        n_self,
        n_echo,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        9797,
    )
}

/// Poll once so the track sends a Hello, and return that Hello's nonce.
fn own_nonce(t: &mut KnownTrack, listener: &KnownSocket, now: Instant) -> [u8; 16] {
    t.poll(now).unwrap();
    hello_nonces(&drain(listener), Role::Joiner)[0]
}

fn deliver(listener: &KnownSocket, frame: &[u8]) {
    listener.send(frame).unwrap();
    std::thread::sleep(Duration::from_millis(50));
}

#[test]
fn a_proof_bound_to_a_replayed_source_address_is_rejected() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let nonce = own_nonce(&mut t, &listener, t0);

    let wrong = secret.proof_with_addr(
        Role::Sharer,
        [1u8; 16],
        nonce,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        [192, 168, 1, 50],
    );
    deliver(&listener, &wrong.encode());
    assert_eq!(t.poll(t0 + Duration::from_millis(10)).unwrap(), None);

    let right = secret.proof_with_addr(
        Role::Sharer,
        [2u8; 16],
        nonce,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        [127, 0, 0, 1],
    );
    deliver(&listener, &right.encode());
    assert!(t.poll(t0 + Duration::from_millis(20)).unwrap().is_some());
}

#[test]
fn a_valid_proof_with_an_unknown_peer_nonce_reports_a_session_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    // Establish a known peer nonce first: a reset needs something to reset.
    let prior = [6u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    // Settle on one normal rotation (>= 25 s), then the early unknown Proof
    // is unambiguous restart evidence.
    let settle_at = t0 + NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
    let normal = [10u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, normal).encode());
    t.poll_detailed(settle_at).unwrap();
    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, normal, echo).encode(),
    );
    let settled = t
        .poll_detailed(settle_at + Duration::from_millis(10))
        .unwrap();
    assert_eq!(settled.session_reset, None);

    let n_self = [7u8; 16];
    let proof = peer_proof(&secret, &peer, &own, n_self, echo);
    deliver(&listener, &proof.encode());

    let polled = t.poll_detailed(settle_at + Duration::from_secs(1)).unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(polled.session_reset, Some(n_self));
}

#[test]
fn the_first_valid_proof_is_not_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    // A fresh track has no peer nonce yet: the first valid Proof only records
    // one, so a healthy new connection is never armed as a reset.
    let first = [11u8; 16];
    let proof = peer_proof(&secret, &peer, &own, first, echo);
    deliver(&listener, &proof.encode());

    let polled = t.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(polled.session_reset, None);
}

#[test]
fn a_peer_hello_before_the_proof_suppresses_the_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    // A normal rotation happens a full interval later: its Hello teaches the
    // new nonce, and the matching Proof is not reset evidence.
    let n_self = [8u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, n_self).encode());
    let rotated_at = t0 + NONCE_ROTATE_INTERVAL;
    t.poll_detailed(rotated_at).unwrap();

    let proof = peer_proof(&secret, &peer, &own, n_self, echo);
    deliver(&listener, &proof.encode());
    let polled = t
        .poll_detailed(rotated_at + Duration::from_millis(10))
        .unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(polled.session_reset, None);
}

#[test]
fn a_hello_and_its_proof_in_one_poll_are_a_normal_rotation() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    let n_self = [9u8; 16];
    let hello = secret.hello(Role::Sharer, n_self);
    let proof = peer_proof(&secret, &peer, &own, n_self, echo);
    deliver(&listener, &hello.encode());
    deliver(&listener, &proof.encode());

    let polled = t.poll_detailed(t0 + NONCE_ROTATE_INTERVAL).unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(
        polled.session_reset, None,
        "the Hello in the same poll must teach the new nonce first"
    );
}

#[test]
fn the_current_and_previous_peer_nonces_are_not_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let (n1, n2) = ([1u8; 16], [2u8; 16]);
    deliver(&listener, &secret.hello(Role::Sharer, n1).encode());
    t.poll_detailed(t0).unwrap();
    deliver(&listener, &secret.hello(Role::Sharer, n2).encode());
    let rotated_at = t0 + NONCE_ROTATE_INTERVAL;
    t.poll_detailed(rotated_at).unwrap();

    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, n2, echo).encode(),
    );
    let current = t
        .poll_detailed(rotated_at + Duration::from_secs(1))
        .unwrap();
    assert_eq!(current.session_reset, None, "current nonce is not a reset");

    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, n1, echo).encode(),
    );
    let previous = t
        .poll_detailed(rotated_at + Duration::from_secs(2))
        .unwrap();
    assert_eq!(
        previous.session_reset, None,
        "previous nonce is not a reset"
    );
}

#[test]
fn a_restart_hello_inside_one_rotation_then_its_proof_reports_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    // Settle on one normal rotation (>= 25 s) so a later early replacement is
    // unambiguous restart evidence.
    let settle_at = t0 + NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
    let normal = [10u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, normal).encode());
    t.poll_detailed(settle_at).unwrap();
    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, normal, echo).encode(),
    );
    let settled = t
        .poll_detailed(settle_at + Duration::from_millis(10))
        .unwrap();
    assert_eq!(settled.session_reset, None);

    // The peer restarted: a fresh Hello arrives early, but only its Proof may
    // report the reset.
    let restarted = [4u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, restarted).encode());
    let hello_at = settle_at + Duration::from_secs(1);
    let hello_poll = t.poll_detailed(hello_at).unwrap();
    assert_eq!(hello_poll.session_reset, None, "a Hello is not a reset");

    let proof = peer_proof(&secret, &peer, &own, restarted, echo);
    deliver(&listener, &proof.encode());
    let polled = t
        .poll_detailed(hello_at + Duration::from_millis(10))
        .unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(polled.session_reset, Some(restarted));

    // Reported once, not on the next Proof with the same nonce.
    deliver(&listener, &proof.encode());
    let again = t
        .poll_detailed(hello_at + Duration::from_millis(20))
        .unwrap();
    assert_eq!(again.session_reset, None);
}

// A fresh track opened mid-rotation sees the peer's first normal replacement
// early. Before any normal rotation has been observed this is ambiguous, so it
// is intentionally treated as normal: an idle healthy link must not be dropped.
// The ordinary 35 s stall / 10 s retry path still applies.
#[test]
fn a_mid_rotation_join_first_hello_rotation_is_not_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    let n_self = [12u8; 16];
    let early = t0 + Duration::from_secs(1);
    deliver(&listener, &secret.hello(Role::Sharer, n_self).encode());
    let hello_poll = t.poll_detailed(early).unwrap();
    assert_eq!(hello_poll.session_reset, None, "a Hello is not a reset");

    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, n_self, echo).encode(),
    );
    let proof_poll = t.poll_detailed(early + Duration::from_millis(10)).unwrap();
    assert_eq!(proof_poll.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(
        proof_poll.session_reset, None,
        "the first observed replacement is ambiguous, not a restart"
    );
}

// Same mid-rotation join, but the peer's rotation Hello was lost.
#[test]
fn a_mid_rotation_join_first_lost_hello_proof_is_not_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    let n_self = [13u8; 16];
    let early = t0 + Duration::from_secs(1);
    deliver(
        &listener,
        &peer_proof(&secret, &peer, &own, n_self, echo).encode(),
    );
    let polled = t.poll_detailed(early).unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(
        polled.session_reset, None,
        "an unsettled early Proof is a mid-rotation join, not a restart"
    );
}

#[test]
fn a_lost_hello_rotation_at_25s_or_more_is_not_a_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let secret = peer_secret(&peer, &own);
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let prior = [1u8; 16];
    deliver(&listener, &secret.hello(Role::Sharer, prior).encode());
    t.poll_detailed(t0).unwrap();

    // A normal rotation whose Hello was lost: the Proof's new nonce appears a
    // full interval later, so it is a rotation, not a restart.
    let rotated = [5u8; 16];
    let proof = peer_proof(&secret, &peer, &own, rotated, echo);
    deliver(&listener, &proof.encode());
    let polled = t.poll_detailed(t0 + NONCE_ROTATE_INTERVAL).unwrap();
    assert_eq!(polled.endpoint, Some("127.0.0.1:9797".parse().unwrap()));
    assert_eq!(polled.session_reset, None);
}

#[test]
fn a_forged_proof_with_an_unknown_nonce_reports_no_reset() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    let echo = own_nonce(&mut t, &listener, t0);

    let forged = Proof {
        role: Role::Sharer,
        n_self: [3u8; 16],
        n_echo: echo,
        port: 9797,
        peer_addr: [127, 0, 0, 1],
        mac: [6u8; 32],
    };
    deliver(&listener, &forged.encode());

    let polled = t.poll_detailed(t0 + Duration::from_millis(10)).unwrap();
    assert_eq!(polled.endpoint, None);
    assert_eq!(polled.session_reset, None);
}

#[test]
fn probe_sends_a_hello_immediately() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let t0 = Instant::now();
    let mut t = track(&own, &peer, Role::Joiner, t0);
    t.poll(t0).unwrap();
    drain(&listener);

    t.probe(t0 + Duration::from_millis(100)).unwrap();
    assert_eq!(
        hello_nonces(&drain(&listener), Role::Joiner).len(),
        1,
        "the probe did not send a Hello before the cadence"
    );
}

// ---- persisted record ------------------------------------------------------

#[test]
fn pair_record_holds_the_pair_key_and_matches_on_both_sides() {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let rec_a = pair_record(
        &a,
        &b.public_key_b64(),
        Role::Sharer,
        Some("10.1.0.2:9797".into()),
        42,
    )
    .unwrap();
    let rec_b = pair_record(&b, &a.public_key_b64(), Role::Joiner, None, 43).unwrap();

    assert_eq!(rec_a.id, rec_b.id, "pairId differs between the two sides");
    assert_eq!(
        rec_a.id,
        PairSecret::derive(&a, &b.public_key_b64())
            .unwrap()
            .pair_id()
    );
    assert_eq!(rec_a.public_key, b.public_key_b64());
    assert_eq!(
        rec_a.private_key.as_deref(),
        Some(a.private_key_b64().as_str())
    );
    // `role` is the PEER's role.
    assert_eq!(rec_a.role, Role::Joiner);
    assert_eq!(rec_b.role, Role::Sharer);
    assert_eq!(rec_a.last_endpoint.as_deref(), Some("10.1.0.2:9797"));
    assert_eq!(rec_b.last_endpoint, None);
    assert_eq!(rec_a.last_connected, Some(42));
    // The name is the other device's role plus the pair's two words.
    assert_eq!(rec_a.name, format!("Laptop - {}", pair_words(&rec_a.id)));
    assert_eq!(rec_b.name, format!("Phone - {}", pair_words(&rec_b.id)));
}

#[test]
fn pair_record_rejects_a_low_order_peer_key() {
    let a = KeyPair::generate();
    let zero = base64_zero();
    assert!(pair_record(&a, &zero, Role::Joiner, None, 1).is_err());
}

fn base64_zero() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode([0u8; 32])
}

#[test]
fn stored_pair_is_the_first_peer_that_has_a_private_key() {
    let legacy = Peer {
        id: "old".into(),
        name: "old".into(),
        public_key: "k".into(),
        role: Role::Sharer,
        last_connected: None,
        private_key: None,
        last_endpoint: None,
    };
    assert!(stored_pair(&[]).is_none());
    assert!(stored_pair(std::slice::from_ref(&legacy)).is_none());
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let rec = pair_record(&a, &b.public_key_b64(), Role::Joiner, None, 1).unwrap();
    let peers = [legacy, rec.clone()];
    assert_eq!(stored_pair(&peers), Some(&rec));
}
