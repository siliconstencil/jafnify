//! Bye and Fresh session Signals between two known-peer tracks over loopback
//! multicast: a stopping peer is seen at once, and a restarted peer starts a
//! new session on the other side even while data keeps flowing.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{KnownPoll, KnownSocket, KnownTrack, PairSecret, SignalKind};
use jafnify_core::state::Role;

static MULTICAST: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

const TEST_PORT: u16 = 49812;

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.98:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

fn track(own: &KeyPair, peer: &KeyPair, role: Role, now: Instant) -> KnownTrack {
    KnownTrack::open(&cfg(), own, &peer.public_key_b64(), role, now).unwrap()
}

/// Poll both tracks for `span` of simulated time (100 ms steps) and merge
/// what each side learned.
fn run(
    a: &mut KnownTrack,
    b: &mut KnownTrack,
    clock: &mut Instant,
    span: Duration,
) -> (KnownPoll, KnownPoll) {
    let (mut pa, mut pb) = (KnownPoll::default(), KnownPoll::default());
    let end = *clock + span;
    while *clock < end {
        *clock += Duration::from_millis(100);
        let qa = a.poll_detailed(*clock).unwrap();
        let qb = b.poll_detailed(*clock).unwrap();
        merge(&mut pa, qa);
        merge(&mut pb, qb);
        std::thread::sleep(Duration::from_millis(3));
    }
    (pa, pb)
}

fn merge(into: &mut KnownPoll, p: KnownPoll) {
    into.endpoint = into.endpoint.or(p.endpoint);
    into.peer_bye |= p.peer_bye;
    into.peer_restarted |= p.peer_restarted;
}

/// Two tracks of one pair, both past the Proof exchange and connected.
fn connected_pair(
    a: &KeyPair,
    b: &KeyPair,
    clock: &mut Instant,
) -> (KnownTrack, KnownTrack, KnownPoll, KnownPoll) {
    let mut sharer = track(a, b, Role::Sharer, *clock);
    let mut joiner = track(b, a, Role::Joiner, *clock);
    let (ps, pj) = run(&mut sharer, &mut joiner, clock, Duration::from_secs(2));
    assert!(
        ps.endpoint.is_some() && pj.endpoint.is_some(),
        "no Proof exchange"
    );
    sharer.set_connected(true);
    joiner.set_connected(true);
    (sharer, joiner, ps, pj)
}

#[test]
fn a_new_connection_is_not_a_restart() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let mut clock = Instant::now();
    let (mut s, mut j, ps, pj) = connected_pair(&a, &b, &mut clock);
    // The Fresh copies of the first session keep arriving after connect.
    let (ps2, pj2) = run(&mut s, &mut j, &mut clock, Duration::from_secs(12));
    for p in [ps, pj, ps2, pj2] {
        assert!(!p.peer_restarted, "the first session is not a restart");
        assert!(!p.peer_bye);
    }
}

#[test]
fn a_bye_is_seen_by_the_peer() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let mut clock = Instant::now();
    let (mut s, mut j, _, _) = connected_pair(&a, &b, &mut clock);
    j.send_bye().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    clock += Duration::from_millis(100);
    let p = s.poll_detailed(clock).unwrap();
    assert!(p.peer_bye, "the sharer must see the joiner's Bye");
}

#[test]
fn a_restarted_peer_starts_a_new_session_once() {
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let mut clock = Instant::now();
    let (mut s, j, _, _) = connected_pair(&a, &b, &mut clock);
    drop(j);
    // The joiner process restarts: a new track, same pair, within seconds.
    let mut j2 = track(&b, &a, Role::Joiner, clock);
    let (ps, pj) = run(&mut s, &mut j2, &mut clock, Duration::from_secs(12));
    assert!(pj.endpoint.is_some());
    assert!(ps.peer_restarted, "the sharer must start a new session");
    assert!(
        !pj.peer_restarted,
        "the restarted side was not connected yet"
    );
    // Later Fresh copies of the same new session do not count again.
    let (ps2, _) = run(&mut s, &mut j2, &mut clock, Duration::from_secs(12));
    assert!(!ps2.peer_restarted);
}

#[test]
fn a_restart_late_in_the_rotation_window_is_still_seen() {
    // The nonce heuristic alone misses a restart >= 25 s after the last
    // peer nonce; the Fresh signal does not depend on timing.
    let _g = lock();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let mut clock = Instant::now();
    let (mut s, mut j, _, _) = connected_pair(&a, &b, &mut clock);
    run(&mut s, &mut j, &mut clock, Duration::from_secs(28));
    drop(j);
    let mut j2 = track(&b, &a, Role::Joiner, clock);
    let (ps, _) = run(&mut s, &mut j2, &mut clock, Duration::from_secs(12));
    assert!(ps.peer_restarted);
}

#[test]
fn a_forged_or_misaddressed_signal_is_ignored() {
    let _g = lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let (a, b, stranger) = (
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    );
    let mut clock = Instant::now();
    let (mut s, _j, _, _) = connected_pair(&a, &b, &mut clock);
    // A stranger's secret, and the right secret with made-up nonces.
    let wrong = PairSecret::derive(&stranger, &a.public_key_b64()).unwrap();
    let right = PairSecret::derive(&b, &a.public_key_b64()).unwrap();
    for secret in [&wrong, &right] {
        let bye = secret.signal(
            SignalKind::Bye,
            Role::Joiner,
            [9u8; 16],
            [8u8; 16],
            &b.public_key_bytes(),
            &a.public_key_bytes(),
        );
        listener.send(&bye.encode()).unwrap();
    }
    std::thread::sleep(Duration::from_millis(30));
    clock += Duration::from_millis(100);
    let p = s.poll_detailed(clock).unwrap();
    assert!(!p.peer_bye);
    assert!(!p.peer_restarted);
}
