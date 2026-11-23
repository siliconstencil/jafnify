//! The use-at-your-own-risk acceptance is stored once, next to the state file.
//! A missing, corrupt or older
//! record is not an acceptance.

use jafnify_core::terms::{
    accepted_version, is_accepted, record_acceptance, DISCLAIMER, HELP_FOOTER, TERMS_FILE,
    TERMS_VERSION,
};

#[test]
fn nothing_is_accepted_in_a_new_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(accepted_version(dir.path()), 0);
    assert!(!is_accepted(dir.path()));
}

#[test]
fn a_recorded_acceptance_is_seen_by_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    record_acceptance(dir.path(), 1_800_000_000).unwrap();
    assert!(dir.path().join(TERMS_FILE).exists());
    // A later start is a new call on the same directory.
    assert!(is_accepted(dir.path()));
    assert_eq!(accepted_version(dir.path()), TERMS_VERSION);
}

#[test]
fn a_corrupt_terms_file_is_not_an_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(TERMS_FILE), "version = [not a number").unwrap();
    assert!(!is_accepted(dir.path()));
}

#[test]
fn an_older_version_is_not_an_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(TERMS_FILE),
        "version = 0\naccepted_at = 1\n",
    )
    .unwrap();
    assert!(!is_accepted(dir.path()));
}

#[test]
fn the_disclaimer_carries_the_plan_text() {
    for part in [
        "Use at your own risk.",
        "You are responsible for how you use it.",
        "mobile plan's terms",
        "What it protects.",
        "If you turn the kill switch on, the internet is blocked while the tunnel is down.",
        "Report security problems privately as described in SECURITY.md.",
        "Do not use Jafnify for anything that is illegal where you are.",
    ] {
        assert!(DISCLAIMER.contains(part), "missing: {part}");
    }
}

#[test]
fn the_terms_version_matches_the_text_change() {
    assert_eq!(TERMS_VERSION, 3);
}

#[test]
fn the_disclaimer_does_not_talk_about_hiding_or_the_carrier() {
    for word in ["carrier", "hide", "hidden", "anonymity", "anonymous"] {
        assert!(
            !DISCLAIMER.to_lowercase().contains(word),
            "must not contain: {word}"
        );
    }
}

#[test]
fn the_help_footer_points_to_the_full_terms() {
    assert!(HELP_FOOTER.contains("Use at your own risk"));
    assert!(HELP_FOOTER.contains("README.md"));
    assert!(HELP_FOOTER.is_ascii());
    assert!(DISCLAIMER.is_ascii());
}
