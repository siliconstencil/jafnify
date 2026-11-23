use std::time::Duration;

use jafnify_core::link_test::{
    connect_first_line, LinkOutcome, LinkSample, LinkTestConfig, LinkTestResult, INTERVAL, PROBES,
    TIMEOUT,
};

fn sample(seq: u32, rtt_ms: Option<u64>) -> LinkSample {
    LinkSample { seq, rtt_ms }
}

#[test]
fn fixed_plan_is_ten_probes_a_hundred_ms_apart_with_a_one_second_timeout() {
    assert_eq!(PROBES, 10);
    assert_eq!(INTERVAL, Duration::from_millis(100));
    assert_eq!(TIMEOUT, Duration::from_secs(1));
    assert_eq!(
        LinkTestConfig::DEFAULT,
        LinkTestConfig {
            probes: 10,
            interval: Duration::from_millis(100),
            timeout: Duration::from_secs(1),
        }
    );
}

#[test]
fn zero_loss_reports_min_average_and_max() {
    let samples: Vec<_> = (0..10)
        .map(|i| sample(i, Some(if i < 5 { 10 } else { 20 })))
        .collect();
    let result = LinkTestResult::from_samples(&samples);
    assert_eq!(result.sent, 10);
    assert_eq!(result.received, 10);
    assert_eq!(result.min_ms, Some(10));
    assert_eq!(result.avg_ms, Some(15));
    assert_eq!(result.max_ms, Some(20));
    assert_eq!(result.loss_pct, 0);
    assert_eq!(result.outcome, LinkOutcome::Measured);
    assert_eq!(
        result.line(),
        "Round trip: min 10 ms, average 15 ms, max 20 ms; 0% loss."
    );
}

#[test]
fn one_lost_probe_reports_ten_percent_loss() {
    let mut samples: Vec<_> = (0..9).map(|i| sample(i, Some(10))).collect();
    samples.push(sample(9, None));
    let result = LinkTestResult::from_samples(&samples);
    assert_eq!(result.received, 9);
    assert_eq!(result.loss_pct, 10);
    assert_eq!(result.outcome, LinkOutcome::Measured);
}

#[test]
fn all_lost_says_no_reply() {
    let samples: Vec<_> = (0..10).map(|i| sample(i, None)).collect();
    let result = LinkTestResult::from_samples(&samples);
    assert_eq!(result.received, 0);
    assert_eq!(result.min_ms, None);
    assert_eq!(result.avg_ms, None);
    assert_eq!(result.max_ms, None);
    assert_eq!(result.loss_pct, 100);
    assert_eq!(result.outcome, LinkOutcome::NoReply);
    assert_eq!(result.line(), "No reply.");
}

#[test]
fn a_duplicate_late_reply_does_not_count_twice() {
    let samples = [sample(0, Some(10)), sample(0, Some(99)), sample(1, None)];
    let result = LinkTestResult::from_samples(&samples);
    assert_eq!(result.sent, 2);
    assert_eq!(result.received, 1);
    assert_eq!(result.min_ms, Some(10));
    assert_eq!(result.max_ms, Some(10));
    assert_eq!(result.loss_pct, 50);
}

#[test]
fn the_denied_and_not_connected_lines_are_fixed() {
    let denied = LinkTestResult::not_permitted();
    assert_eq!(denied.outcome, LinkOutcome::NotPermitted);
    assert_eq!(denied.line(), "Ping is not permitted on this system.");
    let first = LinkTestResult::connect_first();
    assert_eq!(first.outcome, LinkOutcome::ConnectFirst);
    assert_eq!(first.line(), "Connect first.");
    assert_eq!(connect_first_line(), "Connect first.");
}

#[test]
fn a_huge_rtt_does_not_overflow_the_average() {
    let samples = [sample(0, Some(u64::MAX)), sample(1, Some(u64::MAX))];
    let result = LinkTestResult::from_samples(&samples);
    assert_eq!(result.avg_ms, Some(u64::MAX));
}

#[test]
fn an_empty_plan_has_no_measurement() {
    let result = LinkTestResult::from_samples(&[]);
    assert_eq!(result.sent, 0);
    assert_eq!(result.received, 0);
    assert_eq!(result.loss_pct, 100);
    assert_eq!(result.outcome, LinkOutcome::NoReply);
}
