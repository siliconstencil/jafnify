//! Wording and formatting shared by the GUI and the CLI. The Rust twin of the
//! phone's UiText.java: same outputs for the same inputs, no I/O.

use jafnify_linux::ui_text::{
    format_ago, format_bytes, format_elapsed, key_prefix, problem, retry_reason, spaced_code,
    Problem,
};

#[test]
fn spaced_code_separates_the_digits() {
    assert_eq!(spaced_code("4321"), "4 3 2 1");
    assert_eq!(spaced_code(""), "");
}

#[test]
fn elapsed_is_minutes_seconds_then_hours() {
    assert_eq!(format_elapsed(0), "0:00");
    assert_eq!(format_elapsed(59), "0:59");
    assert_eq!(format_elapsed(12 * 60 + 34), "12:34");
    assert_eq!(format_elapsed(3600 + 2 * 60 + 3), "1:02:03");
}

#[test]
fn bytes_use_b_kb_mb() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1 KB");
    assert_eq!(format_bytes(340 * 1024), "340 KB");
    assert_eq!(format_bytes(1536 * 1024), "1.5 MB");
}

#[test]
fn key_prefix_is_the_first_twelve_characters() {
    assert_eq!(key_prefix("AbCdEfGhIjKlMnOp="), "AbCdEfGhIjKl");
    assert_eq!(key_prefix("short"), "short");
    assert_eq!(key_prefix(""), "");
}

#[test]
fn ago_is_relative_and_english() {
    assert_eq!(format_ago(0), "just now");
    assert_eq!(format_ago(59), "just now");
    assert_eq!(format_ago(60), "1 min ago");
    assert_eq!(format_ago(5 * 60 + 30), "5 min ago");
    assert_eq!(format_ago(3600), "1 h ago");
    assert_eq!(format_ago(3 * 3600 + 100), "3 h ago");
    assert_eq!(format_ago(86_400), "1 day ago");
    assert_eq!(format_ago(2 * 86_400 + 5), "2 days ago");
}

#[test]
fn retry_reason_is_the_text_after_the_retry_prefix() {
    assert_eq!(
        retry_reason("Connection lost - retrying in 9 s - no data from peer for 35 s"),
        "no data from peer for 35 s"
    );
    assert_eq!(
        retry_reason("Retrying (10s) - no data from peer for 35 s"),
        "no data from peer for 35 s",
        "an older daemon still finds its reason"
    );
    assert_eq!(retry_reason("Connected"), "");
    assert_eq!(retry_reason("Connection lost - retrying"), "");
    assert_eq!(retry_reason("Retrying (10s)"), "");
}

#[test]
fn network_failures_are_recognised_and_everything_else_is_other() {
    assert_eq!(
        problem("no LAN interface found - connect to a network (tunnel and loopback interfaces are ignored)"),
        Problem::NoNetwork
    );
    assert_eq!(
        problem("discovery error: bind failed: x"),
        Problem::NoNetwork
    );
    assert_eq!(problem("Network is unreachable"), Problem::NoNetwork);
    assert_eq!(
        problem("Root privileges required - wg-quick: boom"),
        Problem::Other
    );
    assert_eq!(problem(""), Problem::None);
}
