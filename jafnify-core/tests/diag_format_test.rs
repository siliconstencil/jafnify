//! The diagnostic journal's pure line format and cadence.

use std::time::{Duration, Instant};

use jafnify_core::diag::{format_line, format_utc_iso_ms, stats_due};

#[test]
fn a_timestamp_is_utc_iso_with_milliseconds() {
    assert_eq!(format_utc_iso_ms(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        format_utc_iso_ms(1_793_450_096_789),
        "2026-10-31T12:34:56.789Z"
    );
    // Leading zeroes are kept, not dropped.
    assert_eq!(
        format_utc_iso_ms(1_793_450_096_007),
        "2026-10-31T12:34:56.007Z"
    );
}

#[test]
fn a_line_has_ts_then_kind_then_fields_in_order() {
    let line = format_line(
        1_793_450_096_789,
        "STATS",
        &[("control_rx_packets", "7"), ("peer", "1.2.3.4:9797")],
    );
    assert_eq!(
        line,
        "ts=2026-10-31T12:34:56.789Z kind=STATS control_rx_packets=7 peer=1.2.3.4:9797"
    );
    assert!(!line.ends_with('\n'), "{line}");
}

#[test]
fn values_are_single_whitespace_free_tokens() {
    let line = format_line(
        0,
        "EVENT",
        &[("reason", "lost link\tagain\r\nlater\u{a0}end")],
    );
    assert!(
        !line.contains('\n') && !line.contains('\r') && !line.contains('\t'),
        "{line}"
    );
    assert_eq!(
        line,
        "ts=1970-01-01T00:00:00.000Z kind=EVENT reason=lost_link_again__later_end"
    );

    let mut tokens = line.split_whitespace();
    assert_eq!(tokens.next(), Some("ts=1970-01-01T00:00:00.000Z"));
    assert_eq!(tokens.next(), Some("kind=EVENT"));
    let field = tokens.next().expect("one field token");
    assert_eq!(field, "reason=lost_link_again__later_end");
    assert_eq!(tokens.next(), None);
    let (key, value) = field.split_once('=').unwrap();
    assert_eq!(key, "reason");
    assert!(!value.chars().any(char::is_whitespace), "{value:?}");
}

#[test]
fn a_key_shaped_base64_value_is_redacted() {
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let line = format_line(0, "EVENT", &[("key", key)]);
    assert!(!line.contains(key), "{line}");
    assert!(!line.contains('A'), "{line}");
    assert!(line.contains("key=[redacted]"), "{line}");
}

#[test]
fn the_first_sample_is_due() {
    let base = Instant::now();
    assert!(stats_due(base, None, None));
    assert!(stats_due(base, None, Some(base)));
}

#[test]
fn stats_run_every_five_seconds_while_peer_rx_is_fresh() {
    let base = Instant::now();
    // Peer rx one second before the 5 s boundary: still fresh.
    let peer_rx = Some(base + Duration::from_secs(4));
    assert!(!stats_due(
        base + Duration::from_millis(4_999),
        Some(base),
        peer_rx
    ));
    assert!(stats_due(
        base + Duration::from_secs(5),
        Some(base),
        peer_rx
    ));
}

#[test]
fn stats_run_every_second_when_peer_rx_is_flat_or_unknown() {
    let base = Instant::now();

    // Peer rx was four seconds before the last sample: flat, sample at 1 s.
    let flat = Some(base - Duration::from_secs(4));
    assert!(!stats_due(
        base + Duration::from_millis(999),
        Some(base),
        flat
    ));
    assert!(stats_due(base + Duration::from_secs(1), Some(base), flat));

    // Exactly three seconds is not older than three seconds: still 5 s.
    assert!(!stats_due(
        base + Duration::from_secs(1),
        Some(base),
        Some(base - Duration::from_secs(2))
    ));

    // Unknown peer rx also samples at 1 s.
    assert!(!stats_due(
        base + Duration::from_millis(999),
        Some(base),
        None
    ));
    assert!(stats_due(base + Duration::from_secs(1), Some(base), None));
}

#[test]
fn stats_return_to_five_seconds_after_peer_rx_resumes() {
    let base = Instant::now();
    // The last sample was taken during a flat period.
    let last_stats = base + Duration::from_secs(4);
    // Peer rx has just resumed.
    let peer_rx = base + Duration::from_secs(5);

    // One second later would be due on the flat cadence, but not on 5 s.
    assert!(!stats_due(
        last_stats + Duration::from_secs(1),
        Some(last_stats),
        Some(peer_rx)
    ));
    assert!(stats_due(
        last_stats + Duration::from_secs(5),
        Some(last_stats),
        Some(peer_rx)
    ));
}
