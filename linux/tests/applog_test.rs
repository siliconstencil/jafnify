//! The app event log: timestamps, rotation, no secrets.

use std::time::{Duration, UNIX_EPOCH};

use jafnify_linux::applog::{format_utc, Logger, MAX_BYTES};

#[test]
fn the_diagnostic_path_is_disabled_in_a_test_process() {
    std::env::set_var("JAFNIFY_TEST_NO_ROOT", "1");
    assert!(jafnify_linux::applog::diag_path_unless_test().is_none());
    std::env::remove_var("JAFNIFY_TEST_NO_ROOT");
}

#[test]
fn timestamps_are_utc() {
    assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        format_utc(UNIX_EPOCH + Duration::from_secs(946_684_800)),
        "2000-01-01T00:00:00Z"
    );
}

#[test]
fn the_ui_timestamp_uses_an_explicit_local_offset_and_abbreviation() {
    assert_eq!(
        jafnify_linux::applog::format_millis_with_zone(0, 0, "UTC"),
        "1970-01-01 00:00:00 UTC"
    );
    assert_eq!(
        jafnify_linux::applog::format_millis_with_zone(1_700_000_000_000, -8 * 3_600, "PST"),
        "2023-11-14 14:13:20 PST"
    );
}

#[test]
fn a_key_is_fully_removed() {
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let text = jafnify_linux::applog::redact(&format!("peer {key} failed"));
    assert!(text.contains("[redacted]"), "{text}");
    assert!(!text.contains('A'), "{text}");
    assert!(!text.contains(key), "{text}");
}

#[test]
fn an_event_has_a_timestamp_and_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.log");
    let mut log = Logger::open(path.clone()).unwrap().without_stderr();
    let key = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
    log.event("INFO", &format!("connected key={key}"));
    log.flush();
    drop(log);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(" INFO connected key=[redacted]"), "{text}");
    assert!(!text.contains('B'), "{text}");
    assert!(!text.contains(key), "{text}");
    assert!(text.starts_with("20"), "{text}");
    assert!(text.contains('T') && text.contains('Z'), "{text}");
}

#[test]
fn a_long_running_logger_rotates_before_exceeding_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.log");
    let mut log = Logger::open(path.clone()).unwrap().without_stderr();
    let chunk = "x".repeat(3 * 1024 * 1024);
    log.event("INFO", &chunk);
    log.event("INFO", &chunk);
    log.flush();
    drop(log);

    assert!(std::fs::metadata(&path).unwrap().len() <= MAX_BYTES);
    let first = dir.path().join("jafnify.log.1");
    assert!(first.exists());
    assert!(std::fs::metadata(&first).unwrap().len() <= MAX_BYTES);
}

#[test]
fn rotation_keeps_three_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.log");
    let rotated = |n: usize| dir.path().join(format!("jafnify.log.{n}"));
    std::fs::write(rotated(1), "one").unwrap();
    std::fs::write(rotated(2), "two").unwrap();
    std::fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();

    let log = Logger::open(path.clone()).unwrap().without_stderr();
    drop(log);

    assert!(path.exists());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    assert_eq!(
        std::fs::read_to_string(rotated(1)).unwrap(),
        "x".repeat(MAX_BYTES as usize)
    );
    assert_eq!(std::fs::read_to_string(rotated(2)).unwrap(), "one");
    assert!(!rotated(3).exists());
}
