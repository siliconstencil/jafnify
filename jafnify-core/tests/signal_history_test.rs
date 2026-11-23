//! The signal history: a small ring of samples, a five minute gate and the
//! one line the phone and the laptop show.

use jafnify_core::signal_history::{SignalHistory, SignalSample, INTERVAL_MS, MAX_SAMPLES};

#[test]
fn the_ring_keeps_288_samples() {
    let mut history = SignalHistory::new();
    for i in 0..(MAX_SAMPLES + 12) {
        history.add(SignalSample {
            at_ms: i as i64 * 60_000,
            level: 3,
        });
    }
    assert_eq!(history.len(), MAX_SAMPLES);
    assert_eq!(history.oldest_at_ms(), Some(12 * 60_000));
}

#[test]
fn the_gate_is_five_minutes() {
    let mut history = SignalHistory::new();
    assert!(history.should_sample(0), "the first sample is taken");
    history.add(SignalSample { at_ms: 0, level: 3 });
    assert!(!history.should_sample(60_000));
    assert!(!history.should_sample(INTERVAL_MS - 1));
    assert!(history.should_sample(INTERVAL_MS));
}

#[test]
fn the_summary_says_the_now_and_the_weakest() {
    let mut history = SignalHistory::new();
    history.add(SignalSample { at_ms: 0, level: 1 });
    history.add(SignalSample {
        at_ms: 300_000,
        level: 3,
    });
    assert_eq!(
        history.summary_line(300_000, 0),
        "Signal: 3/4 now; weakest 1/4 at 00:00."
    );
}

#[test]
fn no_samples_mean_unavailable() {
    let history = SignalHistory::new();
    assert_eq!(history.summary_line(0, 0), "Signal: unavailable.");
}

#[test]
fn an_unknown_level_is_not_a_level() {
    let mut history = SignalHistory::new();
    history.add(SignalSample {
        at_ms: 0,
        level: 255,
    });
    assert_eq!(history.summary_line(0, 0), "Signal: unavailable.");
}
