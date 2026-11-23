//! The pairing code lifetime lives in the core and follows the instant the
//! code was issued. Re-arming the code track (a retry does that) must not
//! hand the user a fresh window.

use std::time::{Duration, Instant};

use jafnify_core::orchestrator::Orchestrator;
use jafnify_core::pairing::{CodeSession, CodeValidator, PairingCode, PairingError};
use jafnify_core::CODE_TTL;

fn code(s: &str) -> PairingCode {
    PairingCode::parse(s).unwrap()
}

#[test]
fn a_code_session_expires_one_ttl_after_it_was_issued() {
    let issued = Instant::now();
    let session = CodeSession::issue_at(code("1234"), issued);

    assert!(!session.is_expired(issued + Duration::from_secs(599)));
    assert!(session.is_expired(issued + Duration::from_secs(600)));
    assert_eq!(
        session.remaining_at(issued + Duration::from_secs(300)),
        Duration::from_secs(300)
    );
    assert_eq!(session.expires_at(), issued + CODE_TTL);
}

#[test]
fn a_validator_built_from_a_session_uses_the_issue_instant() {
    let issued = Instant::now();
    let validator = CodeValidator::new_at(issued);

    assert!(!validator.is_expired(issued + Duration::from_secs(599)));
    assert!(validator.is_expired(issued + Duration::from_secs(600)));
}

#[test]
fn re_arming_the_code_track_does_not_extend_the_window() {
    let issued = Instant::now();
    let mut orch = Orchestrator::new();
    orch.start_search_at(code("1234"), issued);

    // The retry path re-arms the track five minutes later with the same code.
    let rearmed = issued + Duration::from_secs(300);
    orch.rearm_code_track_at(rearmed);
    assert_eq!(
        orch.code_session().map(|s| s.remaining_at(rearmed)),
        Some(Duration::from_secs(300))
    );

    // The window still ends one lifetime after the code was issued.
    assert!(!orch.code_expired_at(issued + Duration::from_secs(599)));
    assert!(orch.code_expired_at(issued + Duration::from_secs(600)));
    assert!(orch
        .try_accept_code_at("1234", issued + Duration::from_secs(599))
        .is_ok());
    assert_eq!(
        orch.try_accept_code_at("1234", issued + Duration::from_secs(600)),
        Err(PairingError::Expired)
    );
}

#[test]
fn a_code_is_accepted_one_second_before_it_expires() {
    let issued = Instant::now();
    let mut orch = Orchestrator::new();
    orch.start_search_at(code("1234"), issued);

    assert!(orch
        .try_accept_code_at("1234", issued + Duration::from_secs(599))
        .is_ok());
}

#[test]
fn a_code_is_rejected_once_it_expired() {
    let issued = Instant::now();
    let mut orch = Orchestrator::new();
    orch.start_search_at(code("1234"), issued);

    assert_eq!(
        orch.try_accept_code_at("1234", issued + Duration::from_secs(600)),
        Err(PairingError::Expired)
    );
}

#[test]
fn a_new_code_starts_a_new_window() {
    let issued = Instant::now();
    let mut orch = Orchestrator::new();
    orch.start_search_at(code("1234"), issued);

    // A different code is a different session with its own window.
    orch.start_search(code("5678"));

    assert!(!orch.code_expired_at(issued + Duration::from_secs(600)));
    assert_eq!(
        orch.code_session().map(|s| s.code().as_str().to_string()),
        Some("5678".to_string())
    );
}
