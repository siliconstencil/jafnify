// Routing follows tunnel liveness (replaces the earlier 20-30 s grace): with
// the kill switch off the system's own routing
// must win as soon as the tunnel is known dead, and a link change is instant.

use std::time::Duration;

use jafnify_core::timing::{
    LINK_BACK_ANSWER, LIVENESS_DEAD_AFTER, LIVENESS_DEAD_AFTER_NO_PING, LIVENESS_PROBE_IDLE,
    MIN_ROUTE_TOGGLE, RETURN_CONNECT_TIMEOUT,
};

#[test]
fn a_dead_tunnel_is_declared_within_a_few_missed_probes() {
    // At least three probes go unanswered before the tunnel counts as dead,
    // so one lost packet on a phone hotspot does not flip the routing.
    assert!(LIVENESS_DEAD_AFTER >= LIVENESS_PROBE_IDLE * 3);
    assert!(LIVENESS_DEAD_AFTER <= Duration::from_secs(10));
}

#[test]
fn without_ping_the_wireguard_keepalive_reply_must_fit() {
    // UDP probe: the peer answers with a passive keepalive within 10 s.
    assert!(LIVENESS_DEAD_AFTER_NO_PING > Duration::from_secs(10) + LIVENESS_PROBE_IDLE);
}

#[test]
fn routing_does_not_flap_faster_than_the_toggle_floor() {
    assert!(MIN_ROUTE_TOGGLE >= Duration::from_secs(2));
    assert!(MIN_ROUTE_TOGGLE < LIVENESS_DEAD_AFTER);
}

#[test]
fn a_return_after_fallback_outlasts_the_wireguard_rehandshake() {
    // WireGuard: no reply for KEEPALIVE_TIMEOUT (10 s) + REKEY_TIMEOUT (5 s)
    // plus jitter triggers a new handshake; leave room for one retry.
    let rehandshake = Duration::from_secs(10 + 5 + 3);
    assert!(RETURN_CONNECT_TIMEOUT >= rehandshake + Duration::from_secs(5));
    assert!(RETURN_CONNECT_TIMEOUT > jafnify_core::CONNECT_TIMEOUT);
}

#[test]
fn a_network_switch_is_detected_within_seconds() {
    assert!(LINK_BACK_ANSWER <= Duration::from_secs(3));
    assert!(LINK_BACK_ANSWER < LIVENESS_DEAD_AFTER);
}
