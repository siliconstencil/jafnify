use std::time::{Duration, Instant};

use jafnify_core::orchestrator::{ConnectionState, Event, Orchestrator};
use jafnify_core::pairing::{DiscoveryToken, PairingCode, PairingError};
use jafnify_core::RETRY_INTERVAL;

fn code(s: &str) -> PairingCode {
    PairingCode::parse(s).unwrap()
}

fn legacy_token(code: &PairingCode, nonce: [u8; 16]) -> DiscoveryToken {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_str().as_bytes()).unwrap();
    mac.update(&nonce);
    DiscoveryToken {
        nonce,
        mac: mac.finalize().into_bytes().into(),
    }
}

#[test]
fn starts_in_idle_and_searches_on_start() {
    let mut o = Orchestrator::new();
    assert_eq!(o.state(), ConnectionState::Idle);
    o.start_search(code("1234"));
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn a_v2_token_moves_to_pairing() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    let token = DiscoveryToken::derive(&[1u8; 16]);
    assert!(o.on_discovery_event(&token));
    assert_eq!(o.state(), ConnectionState::Pairing);
}

#[test]
fn a_legacy_token_sets_the_old_peer_flag_without_pairing() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    let token = legacy_token(&code("1234"), [1u8; 16]);
    assert!(!o.on_discovery_event(&token));
    assert_eq!(o.state(), ConnectionState::Searching);
    assert!(o.old_peer_seen());
}

// Multicast loop is on, so a device hears its own discovery token. It must
// never pair with itself (lab report, finding 2).
#[test]
fn own_discovery_token_is_ignored() {
    let mut o = Orchestrator::new();
    assert!(o.own_token().is_none());
    o.start_search(code("1234"));
    let own = o.own_token().expect("token after start_search");
    assert!(own.verify());
    assert!(!o.on_discovery_event(&own));
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn own_token_is_stable_and_fresh_per_search() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    let first = o.own_token().unwrap();
    assert_eq!(o.own_token().unwrap(), first);
    o.start_search(code("1234"));
    assert_ne!(o.own_token().unwrap().nonce, first.nonce);
}

#[test]
fn peer_token_with_other_nonce_is_accepted() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    let own = o.own_token().unwrap();
    let mut other_nonce = own.nonce;
    other_nonce[0] ^= 0xff;
    let peer = DiscoveryToken::derive(&other_nonce);
    assert!(o.on_discovery_event(&peer));
    assert_eq!(o.state(), ConnectionState::Pairing);
}

#[test]
fn accepted_code_then_connect_cycle() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    assert!(o.try_accept_code("1234").is_ok());
    assert_eq!(o.state(), ConnectionState::Connecting);
    assert!(matches!(
        o.try_accept_code("12ab"),
        Err(PairingError::Malformed)
    ));
    o.confirm_connected();
    assert_eq!(o.state(), ConnectionState::Connected);
    o.on_disconnect("link down".into());
    assert_eq!(o.state(), ConnectionState::Retry);
    assert_eq!(o.last_disconnect_reason(), Some("link down"));
}

#[test]
fn wrong_code_rejected_keeps_searching() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    assert!(matches!(
        o.try_accept_code("5678"),
        Err(PairingError::CodeMismatch)
    ));
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn retry_auto_researches_after_ten_seconds() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    o.confirm_connected();
    o.on_disconnect("link down".into());
    assert_eq!(o.state(), ConnectionState::Retry);
    let now = Instant::now();
    o.tick(now + Duration::from_secs(11));
    assert_eq!(o.state(), ConnectionState::Searching);
}

// The retry deadline is measured from the caller's clock, so a frontend with
// an injected clock gets exactly RETRY_INTERVAL (10 s), not a wall-clock one.
#[test]
fn retry_waits_ten_seconds_from_injected_disconnect_time() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    o.confirm_connected();
    let t0 = Instant::now() + Duration::from_secs(1000);
    o.on_disconnect_at("link down".into(), t0);
    assert_eq!(o.state(), ConnectionState::Retry);
    o.tick(t0 + Duration::from_millis(9_900));
    assert_eq!(o.state(), ConnectionState::Retry);
    o.tick(t0 + Duration::from_secs(10));
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn a_custom_retry_keeps_its_interval_and_remaining_time() {
    let mut o = Orchestrator::new();
    let t0 = Instant::now();
    o.on_disconnect_at_with_retry("peer session reset".into(), t0, Duration::from_secs(2));
    assert_eq!(o.state(), ConnectionState::Retry);
    assert_eq!(o.retry_interval(), Duration::from_secs(2));
    assert_eq!(o.retry_remaining_at(t0), Some(Duration::from_secs(2)));
    assert_eq!(
        o.retry_remaining_at(t0 + Duration::from_secs(1)),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        o.retry_remaining_at(t0 + Duration::from_secs(3)),
        Some(Duration::ZERO)
    );

    // The ordinary path still uses the 10 s default.
    o.on_disconnect_at("link down".into(), t0);
    assert_eq!(o.retry_interval(), RETRY_INTERVAL);
}

// Known track: a stored peer
// reconnects with no pairing code at all.
#[test]
fn known_search_starts_without_a_code() {
    let mut o = Orchestrator::new();
    assert_eq!(o.state(), ConnectionState::Idle);
    o.start_known_search();
    assert_eq!(o.state(), ConnectionState::Searching);
    // No code => no 0x01 token to announce, and no code to accept.
    assert!(o.own_token().is_none());
    assert!(o.try_accept_code("1234").is_err());
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn known_search_ignores_pairing_tokens() {
    let mut o = Orchestrator::new();
    o.start_known_search();
    let token = DiscoveryToken::derive(&[1u8; 16]);
    assert!(!o.on_discovery_event(&token));
    assert_eq!(o.state(), ConnectionState::Searching);
}

#[test]
fn known_disconnect_retries_then_searches_again_after_ten_seconds() {
    let mut o = Orchestrator::new();
    o.start_known_search();
    o.confirm_connected();
    assert_eq!(o.state(), ConnectionState::Connected);
    let t0 = Instant::now() + Duration::from_secs(1000);
    o.on_disconnect_at("no verified frame for 15 s".into(), t0);
    assert_eq!(o.state(), ConnectionState::Retry);
    assert_eq!(
        o.last_disconnect_reason(),
        Some("no verified frame for 15 s")
    );
    o.tick(t0 + Duration::from_millis(9_900));
    assert_eq!(o.state(), ConnectionState::Retry);
    o.tick(t0 + Duration::from_secs(10));
    assert_eq!(o.state(), ConnectionState::Searching);
    // Still the known track after the retry: no code appeared.
    assert!(o.own_token().is_none());
    assert!(o.try_accept_code("1234").is_err());
}

#[test]
fn known_search_reports_no_peer_found_after_timeout() {
    let mut o = Orchestrator::new();
    o.start_known_search();
    let ev = o.tick(Instant::now() + Duration::from_secs(61));
    assert!(matches!(ev, Some(Event::NoPeerFound)));
    assert_eq!(o.state(), ConnectionState::Searching);
}

// A verified Proof (or the sharer seeing its peer) moves a known search to
// Connecting, where the usual connect timeout and loss rules apply.
#[test]
fn known_peer_found_moves_searching_to_connecting_only() {
    let mut o = Orchestrator::new();
    o.known_peer_found();
    assert_eq!(o.state(), ConnectionState::Idle);
    o.start_known_search();
    o.known_peer_found();
    assert_eq!(o.state(), ConnectionState::Connecting);
    o.confirm_connected();
    o.known_peer_found();
    assert_eq!(o.state(), ConnectionState::Connected);
}

// Pairing a new device after a known search must switch back to the code track.
#[test]
fn start_search_after_known_search_uses_the_code_again() {
    let mut o = Orchestrator::new();
    o.start_known_search();
    o.start_search(code("1234"));
    assert!(o.own_token().is_some());
    assert!(o.try_accept_code("1234").is_ok());
}

#[test]
fn no_peer_found_reported_after_search_timeout() {
    let mut o = Orchestrator::new();
    o.start_search(code("1234"));
    let now = Instant::now();
    let ev = o.tick(now + Duration::from_secs(61));
    assert!(matches!(ev, Some(Event::NoPeerFound)));
    assert_eq!(o.state(), ConnectionState::Searching);
}
