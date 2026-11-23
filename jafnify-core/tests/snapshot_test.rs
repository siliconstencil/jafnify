//! The words on the status card and the verdicts behind them live in the
//! core, so the phone and the laptop say the same thing.

use jafnify_core::orchestrator::ConnectionState;
use jafnify_core::snapshot::{
    classify_path, state_glyph, CodeView, Freshness, Health, PeerPath, RetryView, Snapshot,
    HANDSHAKE_FRESH_SECS,
};

fn snapshot() -> Snapshot {
    Snapshot {
        state: ConnectionState::Connected,
        code: None,
        retry: None,
        health: Health::default(),
    }
}

#[test]
fn a_fresh_handshake_means_the_peer_is_directly_reachable() {
    assert_eq!(
        classify_path(Freshness::Age(HANDSHAKE_FRESH_SECS), None),
        PeerPath::DirectHotspot
    );
}

#[test]
fn an_old_handshake_means_the_peer_cannot_be_reached() {
    assert_eq!(
        classify_path(Freshness::Age(HANDSHAKE_FRESH_SECS + 1), None),
        PeerPath::Unreachable
    );
}

#[test]
fn a_handshake_that_never_happened_waits_for_the_first_one() {
    assert_eq!(
        classify_path(Freshness::Never, None),
        PeerPath::WaitingForHandshake
    );
}

#[test]
fn an_unknown_handshake_falls_back_to_the_last_data() {
    assert_eq!(
        classify_path(Freshness::Unknown, Some(5)),
        PeerPath::DirectHotspot
    );
    assert_eq!(
        classify_path(Freshness::Unknown, Some(HANDSHAKE_FRESH_SECS + 1)),
        PeerPath::Unreachable
    );
}

#[test]
fn an_unknown_handshake_without_data_has_no_verdict() {
    assert_eq!(classify_path(Freshness::Unknown, None), PeerPath::Unknown);
}

#[test]
fn the_health_line_says_when_the_last_handshake_was() {
    let mut s = snapshot();
    s.health.handshake = Freshness::Age(12);
    s.health.path = PeerPath::DirectHotspot;

    assert_eq!(
        s.health_line(),
        Some("Last handshake 12 s ago - Direct over hotspot".to_string())
    );
}

#[test]
fn the_health_line_says_when_the_peer_cannot_be_reached() {
    let mut s = snapshot();
    s.health.path = PeerPath::Unreachable;

    assert_eq!(s.health_line(), Some("Not reachable".to_string()));
}

#[test]
fn the_health_line_waits_for_the_first_handshake() {
    let mut s = snapshot();
    s.health.handshake = Freshness::Never;
    s.health.path = classify_path(Freshness::Never, None);

    assert_eq!(
        s.health_line(),
        Some("Waiting for the first handshake".to_string())
    );
}

#[test]
fn the_health_line_names_the_old_peer_reason() {
    let mut s = snapshot();
    s.health.old_peer = true;

    assert_eq!(
        s.health_line(),
        Some("Update Jafnify on the other device".to_string())
    );
}

#[test]
fn the_health_line_is_empty_when_there_is_nothing_to_say() {
    let s = snapshot();
    assert_eq!(s.health_line(), None);
}

#[test]
fn the_main_verdict_has_no_moving_handshake_age() {
    let mut s = snapshot();
    s.health.handshake = Freshness::Age(12);
    s.health.path = PeerPath::DirectHotspot;

    assert_eq!(s.health_verdict(), Some("Direct over hotspot".to_string()));
    assert_eq!(
        s.handshake_detail(),
        Some("Last handshake 12 s ago".to_string())
    );
}

#[test]
fn the_details_age_is_absent_when_there_is_none() {
    let mut s = snapshot();
    s.health.handshake = Freshness::Never;
    s.health.path = PeerPath::WaitingForHandshake;
    assert_eq!(s.handshake_detail(), None);
    assert_eq!(
        s.health_verdict(),
        Some("Waiting for the first handshake".to_string())
    );

    s.health.handshake = Freshness::Unknown;
    s.health.path = PeerPath::Unknown;
    assert_eq!(s.handshake_detail(), None);
    assert_eq!(s.health_verdict(), None);
}

#[test]
fn the_old_peer_verdict_is_the_reason() {
    let mut s = snapshot();
    s.health.old_peer = true;
    assert_eq!(
        s.health_verdict(),
        Some("Update Jafnify on the other device".to_string())
    );
}

#[test]
fn the_retry_line_has_the_real_seconds_left_and_the_reason() {
    let mut s = snapshot();
    s.state = ConnectionState::Retry;
    s.retry = Some(RetryView {
        remaining_secs: 7,
        reason: Some("Wi-Fi link lost".to_string()),
    });

    assert_eq!(
        s.retry_line(),
        Some("Connection lost - retrying in 7 s - Wi-Fi link lost".to_string())
    );
}

#[test]
fn the_retry_line_without_a_reason_still_has_the_seconds() {
    let mut s = snapshot();
    s.state = ConnectionState::Retry;
    s.retry = Some(RetryView {
        remaining_secs: 10,
        reason: None,
    });

    assert_eq!(
        s.retry_line(),
        Some("Connection lost - retrying in 10 s".to_string())
    );
}

#[test]
fn the_code_line_says_minutes_until_the_last_minute() {
    let line = |secs| {
        CodeView {
            digits: "1234".to_string(),
            expires_in_secs: Some(secs),
            expired: false,
        }
        .expiry_line()
    };

    assert_eq!(line(600), "Expires in 10 minutes");
    assert_eq!(line(599), "Expires in 10 minutes");
    assert_eq!(line(61), "Expires in 2 minutes");
    assert_eq!(line(60), "Expires in 1 minute");
}

#[test]
fn the_code_line_switches_to_mmss_in_the_last_minute() {
    let line = |secs| {
        CodeView {
            digits: "1234".to_string(),
            expires_in_secs: Some(secs),
            expired: false,
        }
        .expiry_line()
    };

    assert_eq!(line(59), "Expires in 0:59");
    assert_eq!(line(1), "Expires in 0:01");
    assert_eq!(line(0), "Expires in 0:00");
}

#[test]
fn every_state_has_its_words_in_the_core() {
    let mut s = snapshot();
    for (state, words) in [
        (ConnectionState::Idle, "Not sharing"),
        (ConnectionState::Searching, "Searching for the other device"),
        (ConnectionState::Pairing, "Pairing"),
        (ConnectionState::Connecting, "Connecting"),
        (ConnectionState::Connected, "Connected"),
    ] {
        s.state = state;
        assert_eq!(s.status_line(), words, "{state:?} is worded by the core");
    }

    s.state = ConnectionState::Retry;
    s.retry = Some(RetryView {
        remaining_secs: 7,
        reason: None,
    });
    assert_eq!(s.status_line(), "Connection lost - retrying in 7 s");
}

#[test]
fn the_peer_name_is_only_a_documented_suffix_on_the_core_line() {
    let mut s = snapshot();
    assert_eq!(
        s.status_line_with_peer(Some("Laptop")),
        "Connected - Laptop"
    );
    assert_eq!(s.status_line_with_peer(None), s.status_line());
    assert_eq!(s.status_line_with_peer(Some("")), s.status_line());

    s.state = ConnectionState::Retry;
    s.retry = Some(RetryView {
        remaining_secs: 7,
        reason: Some("Wi-Fi link lost".to_string()),
    });
    assert_eq!(
        s.status_line_with_peer(Some("Phone")),
        "Connection lost - retrying in 7 s - Wi-Fi link lost - Phone"
    );
}

#[test]
fn every_state_has_a_distinct_glyph_name_and_a_shape_not_just_a_colour() {
    use std::collections::HashSet;

    let states = [
        ConnectionState::Idle,
        ConnectionState::Searching,
        ConnectionState::Pairing,
        ConnectionState::Connecting,
        ConnectionState::Connected,
        ConnectionState::Retry,
    ];
    let mut names = HashSet::new();
    for state in states {
        let glyph = state_glyph(state);
        assert!(!glyph.name.is_empty(), "{state:?} has a glyph name");
        assert!(!glyph.shape.name().is_empty(), "{state:?} has a shape name");
        assert_ne!(
            glyph.name, glyph.colour,
            "{state:?} is never drawn with colour only"
        );
        assert!(
            jafnify_core::tokens::light().role(glyph.colour).is_some(),
            "{state:?} uses a real colour role"
        );
        assert!(
            names.insert(glyph.name),
            "{state:?} has a glyph name shared with another state"
        );
    }
}

#[test]
fn an_expired_code_says_so() {
    let code = CodeView {
        digits: "1234".to_string(),
        expires_in_secs: Some(0),
        expired: true,
    };
    assert_eq!(code.expiry_line(), "Code expired");
}
