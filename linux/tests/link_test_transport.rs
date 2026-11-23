use std::collections::VecDeque;
use std::time::Duration;

use jafnify_core::link_test::{LinkOutcome, LinkTestConfig};
use jafnify_linux::link_test::{run, run_default, EchoProbe, EchoReply};

struct FakeProbe {
    replies: VecDeque<EchoReply>,
    calls: Vec<(u32, Duration)>,
}

impl FakeProbe {
    fn new(replies: &[EchoReply]) -> Self {
        Self {
            replies: replies.iter().copied().collect(),
            calls: Vec::new(),
        }
    }
}

impl EchoProbe for FakeProbe {
    fn echo(&mut self, seq: u32, timeout: Duration) -> EchoReply {
        self.calls.push((seq, timeout));
        self.replies.pop_front().unwrap_or(EchoReply::Timeout)
    }
}

fn config(probes: u32) -> LinkTestConfig {
    LinkTestConfig {
        probes,
        interval: Duration::ZERO,
        timeout: Duration::from_millis(10),
    }
}

#[test]
fn a_scripted_run_aggregates_min_average_max_and_loss() {
    let mut probe = FakeProbe::new(&[
        EchoReply::Reply(Duration::from_millis(5)),
        EchoReply::Reply(Duration::from_millis(15)),
        EchoReply::Timeout,
    ]);
    let result = run(&mut probe, config(3));
    assert_eq!(result.sent, 3);
    assert_eq!(result.received, 2);
    assert_eq!(result.min_ms, Some(5));
    assert_eq!(result.avg_ms, Some(10));
    assert_eq!(result.max_ms, Some(15));
    assert_eq!(result.loss_pct, 33);
    assert_eq!(result.outcome, LinkOutcome::Measured);
    assert_eq!(probe.calls.len(), 3);
}

#[test]
fn the_runner_asks_for_the_configured_probe_count_and_timeout() {
    let mut probe = FakeProbe::new(&[
        EchoReply::Reply(Duration::from_millis(1)),
        EchoReply::Reply(Duration::from_millis(2)),
        EchoReply::Reply(Duration::from_millis(3)),
        EchoReply::Reply(Duration::from_millis(4)),
    ]);
    let config = config(4);
    let result = run(&mut probe, config);
    assert_eq!(result.sent, 4);
    assert_eq!(result.received, 4);
    assert_eq!(probe.calls.len(), 4);
    for (expected, (seq, timeout)) in probe.calls.iter().enumerate() {
        assert_eq!(*seq, expected as u32);
        assert_eq!(*timeout, config.timeout);
    }
}

#[test]
fn a_denied_socket_stops_the_run_and_says_not_permitted() {
    let mut probe = FakeProbe::new(&[
        EchoReply::Reply(Duration::from_millis(5)),
        EchoReply::NotPermitted,
        EchoReply::Reply(Duration::from_millis(9)),
    ]);
    let result = run(&mut probe, config(3));
    assert_eq!(result.outcome, LinkOutcome::NotPermitted);
    assert_eq!(result.line(), "Ping is not permitted on this system.");
    assert_eq!(probe.calls.len(), 2, "the run stopped at the denial");
}

#[test]
fn all_timeouts_say_no_reply() {
    let mut probe = FakeProbe::new(&[EchoReply::Timeout, EchoReply::Timeout]);
    let result = run(&mut probe, config(2));
    assert_eq!(result.received, 0);
    assert_eq!(result.outcome, LinkOutcome::NoReply);
    assert_eq!(result.line(), "No reply.");
}

#[test]
fn a_disconnected_run_sends_nothing_and_says_connect_first() {
    let result = run_default(false);
    assert_eq!(result.outcome, LinkOutcome::ConnectFirst);
    assert_eq!(result.sent, 0);
    assert_eq!(result.line(), "Connect first.");
}
