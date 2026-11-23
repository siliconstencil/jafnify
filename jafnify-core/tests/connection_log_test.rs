//! The bounded, redacted connection log ring, its exact templates and the
//! local clock choice. The file behaviour stays in the Linux suite.

use jafnify_core::connection_log::{self, ConnectionLog, MAX_LINES};
use jafnify_core::ui::time::{format_local_millis, format_millis_with_zone};

/// 2023-11-14 22:13:20 UTC.
const T: i64 = 1_700_000_000_000;

#[test]
fn timestamps_are_local_with_the_zone_abbreviation() {
    assert_eq!(
        format_millis_with_zone(0, 0, "UTC"),
        "1970-01-01 00:00:00 UTC"
    );
    assert_eq!(
        format_millis_with_zone(T, -8 * 3_600, "PST"),
        "2023-11-14 14:13:20 PST"
    );

    let mut log = ConnectionLog::new();
    log.push("state", "state Connecting -> Connected", T);
    assert_eq!(
        log.tail(1),
        vec![format!(
            "{} state Connecting -> Connected",
            format_local_millis(T)
        )]
    );
    assert_eq!(log.last_kind(), Some("state"));
}

#[test]
fn state_line_is_pinned() {
    assert_eq!(
        connection_log::state_line("Connected", "Retry"),
        "state Connected -> Retry"
    );
}

#[test]
fn drop_line_uses_the_event_time_and_the_detection_delay() {
    assert_eq!(
        connection_log::drop_line("no data from peer for 35 s", T, T + 35_000),
        format!(
            "disconnected at {}: no data from peer for 35 s (detected after 35 s)",
            format_local_millis(T)
        )
    );
}

#[test]
fn drop_line_with_hint_names_the_range_after_the_reason() {
    let line = connection_log::drop_line_with_hint(
        "no network",
        T,
        T + 35_000,
        Some(jafnify_core::drop_history::HINT_OUT_OF_RANGE),
    );
    assert!(
        line.contains("likely out of Wi-Fi range of the phone"),
        "{line}"
    );
    assert!(line.contains("(detected after 35 s)"), "{line}");
    // Without a hint the line is the plain one, unchanged.
    assert_eq!(
        connection_log::drop_line_with_hint("no network", T, T + 35_000, None),
        connection_log::drop_line("no network", T, T + 35_000)
    );
}

#[test]
fn the_reconnected_line_has_the_time_and_no_endpoint() {
    assert_eq!(
        connection_log::reconnected_line(T + 65_000),
        format!("reconnected at {}", format_local_millis(T + 65_000))
    );
}

#[test]
fn privileged_and_session_lines_are_pinned() {
    assert_eq!(
        connection_log::privileged_line("up", true, true),
        "privileged up: ok (polkit rule)"
    );
    assert_eq!(
        connection_log::privileged_line("down", false, false),
        "privileged down: denied (password prompt)"
    );
    assert_eq!(
        connection_log::session_started_line("sharer"),
        "session started (sharer)"
    );
    assert_eq!(
        connection_log::session_ended_line("stopped"),
        "session ended (stopped)"
    );
    assert_eq!(connection_log::SUPPRESS_RESTORED, "suppress rule restored");
}

#[test]
fn the_ring_drops_the_oldest_past_five_hundred() {
    let mut log = ConnectionLog::new();
    for i in 0..=MAX_LINES {
        log.push("state", &format!("event {i}"), T + i as i64 * 1_000);
    }
    assert_eq!(log.len(), MAX_LINES);
    let all = log.tail(MAX_LINES);
    assert_eq!(all.len(), MAX_LINES);
    assert!(all[0].ends_with("event 1"), "{}", all[0]);
    assert!(
        all[MAX_LINES - 1].ends_with(&format!("event {MAX_LINES}")),
        "{}",
        all[MAX_LINES - 1]
    );
    assert!(!all.iter().any(|line| line.ends_with("event 0")));
}

#[test]
fn tail_is_chronological_and_copy_text_has_no_trailing_newline() {
    let mut log = ConnectionLog::new();
    for i in 1..=5 {
        log.push("state", &format!("line {i}"), T + i as i64 * 1_000);
    }
    let stamp = |i: i64| format_local_millis(T + i * 1_000);
    assert_eq!(
        log.tail(3),
        vec![
            format!("{} line 3", stamp(3)),
            format!("{} line 4", stamp(4)),
            format!("{} line 5", stamp(5)),
        ]
    );
    assert_eq!(
        log.copy_text(3),
        format!(
            "{} line 3\n{} line 4\n{} line 5",
            stamp(3),
            stamp(4),
            stamp(5)
        )
    );
    assert_eq!(log.copy_text(0), "");
    assert_eq!(log.tail(99).len(), 5);
}

#[test]
fn key_shaped_text_is_redacted_on_push_and_in_copy_text() {
    let key = format!("{}=", "A".repeat(43));
    let mut log = ConnectionLog::new();
    log.push("privileged", &format!("peer {key} denied"), T);

    let line = &log.tail(1)[0];
    assert!(line.contains("[redacted]"), "{line}");
    assert!(!line.contains('A'), "{line}");
    assert!(!line.contains(&key), "{line}");
    assert!(!line.contains(&"A".repeat(43)), "{line}");
    let copied = log.copy_text(MAX_LINES);
    assert!(!copied.contains(&key), "{copied}");
}

#[test]
fn a_twelve_hour_locale_gets_am_and_pm() {
    use jafnify_core::ui::time::{format_millis_with_clock, time_format_is_twelve_hour};
    // The same instant is the evening before in PST.
    let t = 1_794_201_528_000;
    let pst = -8 * 3600;
    assert_eq!(
        format_millis_with_clock(t, pst, "PST", true),
        "2026-11-08 9:18:48 PM PST"
    );
    assert_eq!(
        format_millis_with_clock(t, pst, "PST", false),
        "2026-11-08 21:18:48 PST"
    );
    // Midnight and noon.
    let midnight = 1_794_182_400_000 + 300_000; // 2026-11-09 00:05:00 UTC
    assert_eq!(
        format_millis_with_clock(midnight, 0, "UTC", true),
        "2026-11-09 12:05:00 AM UTC"
    );
    assert_eq!(
        format_millis_with_clock(midnight + 12 * 3_600_000, 0, "UTC", true),
        "2026-11-09 12:05:00 PM UTC"
    );
    // en_US glibc says "%r"; C and de_DE say "%H:%M:%S"; en_GB "%T".
    assert!(time_format_is_twelve_hour("%r"));
    assert!(time_format_is_twelve_hour("%I:%M:%S %p"));
    assert!(!time_format_is_twelve_hour("%H:%M:%S"));
    assert!(!time_format_is_twelve_hour("%T"));
}
