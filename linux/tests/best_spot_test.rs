//! The best-spot session of the laptop: the samples of the phone, the marks
//! of the user and the verdict. The session lives in memory only: nothing is
//! written to the disk.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use jafnify_core::best_spot::{Mark, Sample};
use jafnify_core::phone_stats::NetworkKind;
use jafnify_linux::app::AppBuilder;
use jafnify_linux::flags::best_spot_lines;

/// The wall clock in epoch milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Five samples, one second apart, ending at `end_at_ms`.
fn five_samples(end_at_ms: i64, net: NetworkKind, level: u8, dbm: Option<i16>) -> Vec<Sample> {
    (0..5)
        .map(|second| Sample {
            at_ms: end_at_ms - (4 - second) * 1_000,
            net,
            level,
            dbm,
            rtt_ms: Some(40),
            wifi_ms: None,
        })
        .collect()
}

#[test]
fn nothing_is_added_while_the_switch_is_off() {
    let samples = five_samples(20_000, NetworkKind::Mobile5gAdv, 3, Some(-97));
    let marks = vec![Mark {
        at_ms: 20_000,
        number: 1,
    }];
    let lines = best_spot_lines(
        vec!["Phone measurements".to_string()],
        false,
        &samples,
        &marks,
        20_000,
    );
    assert_eq!(
        lines,
        vec!["Phone measurements".to_string()],
        "the block adds nothing while the switch is off"
    );
}

#[test]
fn the_live_line_the_spot_line_and_the_verdict_appear_while_the_switch_is_on() {
    let samples = five_samples(20_000, NetworkKind::Mobile5gAdv, 3, Some(-97));
    let marks = vec![Mark {
        at_ms: 20_000,
        number: 1,
    }];
    let lines = best_spot_lines(Vec::new(), true, &samples, &marks, 20_000);

    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].starts_with("Now: ") && lines[0].contains("5G (advanced)"),
        "the live line: {lines:?}"
    );
    assert!(
        lines[1].starts_with("Spot 1: ") && lines[1].contains("5G (advanced)"),
        "the spot line: {lines:?}"
    );
    assert_eq!(lines[2], "Mark another spot to compare", "{lines:?}");
}

#[test]
fn a_mark_with_fewer_than_five_samples_makes_no_spot() {
    let samples = five_samples(20_000, NetworkKind::Mobile5gAdv, 3, Some(-97));
    let marks = vec![Mark {
        at_ms: 20_000,
        number: 1,
    }];
    // Four samples in the window: the spot cannot be scored.
    let lines = best_spot_lines(Vec::new(), true, &samples[1..], &marks, 20_000);
    assert!(
        !lines.iter().any(|line| line.starts_with("Spot 1: ")),
        "four samples made a spot: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Hold the phone still a few seconds longer")),
        "the hold line is missing: {lines:?}"
    );
}

#[test]
fn the_hold_line_is_said_once_for_several_short_windows() {
    let samples = five_samples(20_000, NetworkKind::Mobile5gAdv, 3, Some(-97));
    let marks = vec![
        Mark {
            at_ms: 20_000,
            number: 1,
        },
        Mark {
            at_ms: 21_000,
            number: 2,
        },
    ];
    // Four samples in the window of both marks: neither spot can be scored.
    let lines = best_spot_lines(Vec::new(), true, &samples[1..], &marks, 21_000);
    let holds = lines
        .iter()
        .filter(|line| line.contains("Hold the phone still a few seconds longer"))
        .count();
    assert_eq!(holds, 1, "the hold line is said once: {lines:?}");
}

/// The Stop button of the best-spot block stays available while the session
/// shows its marks: after the two minutes are over, the marks and the
/// verdict still have to be clearable.
#[test]
fn the_stop_button_stays_available_while_the_session_shows() {
    let gui = std::fs::read_to_string("src/bin/jafnify-gui.rs").unwrap();
    let after = gui
        .split("let stop = egui::Button::new(egui::RichText::new(\"Stop\")")
        .nth(1)
        .expect("the window has a Stop button for the best-spot session");
    let enabled = &after[..after.len().min(400)];
    assert!(
        enabled.contains("add_enabled(self.snapshot.best_spot_showing(), stop)"),
        "the Stop button follows the session that shows: {enabled}"
    );
    assert!(
        !enabled.contains("best_spot_running"),
        "the Stop button dies with the two minutes: {enabled}"
    );
}

#[test]
fn the_session_shows_its_marks_after_the_two_minutes_are_over() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let now = Instant::now();
    let base = now_ms();

    app.best_spot_start(now);
    app.best_spot_mark(now);
    assert!(app.best_spot_showing(), "the session shows its marks");
    assert!(app.best_spot_running(base), "the session measures");

    // The two minutes are over: the session measures no more, but its marks
    // and its verdict stay on the screen until Stop clears them.
    assert!(
        !app.best_spot_running(base + 120_000),
        "the session is over"
    );
    assert!(app.best_spot_showing(), "the session still shows its marks");

    app.best_spot_stop();
    assert!(!app.best_spot_showing(), "the session is gone");
}

#[test]
fn the_verdict_names_the_best_spot() {
    let mut samples = five_samples(20_000, NetworkKind::Mobile4g, 2, Some(-110));
    let mut marks = vec![Mark {
        at_ms: 20_000,
        number: 1,
    }];
    // A second spot on 5G with a better signal.
    samples.extend(five_samples(40_000, NetworkKind::Mobile5gAdv, 4, Some(-90)));
    marks.push(Mark {
        at_ms: 40_000,
        number: 2,
    });

    let lines = best_spot_lines(Vec::new(), true, &samples, &marks, 40_000);
    assert!(
        lines.iter().any(|line| line.starts_with("Spot 1: ")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("Spot 2: ")),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "Best so far: Spot 2", "{lines:?}");
}

#[test]
fn the_session_keeps_the_samples_and_the_marks_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let now = Instant::now();
    let base = now_ms();

    app.best_spot_start(now);
    for second in 0..5 {
        app.best_spot_sample(Sample {
            at_ms: base + second * 1_000,
            net: NetworkKind::Mobile5gAdv,
            level: 3,
            dbm: Some(-97),
            rtt_ms: Some(40),
            wifi_ms: Some(30),
        });
    }
    app.best_spot_mark(now);

    let state = app.best_spot_state();
    assert_eq!(state.samples.len(), 5, "{state:?}");
    assert_eq!(state.marks.len(), 1, "{state:?}");
    assert_eq!(state.marks[0].number, 1, "{state:?}");
    assert!(state.started_at.is_some(), "{state:?}");
    assert!(app.best_spot_running(base + 1_000), "the session runs");
}

#[test]
fn a_sample_without_a_session_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = now_ms();

    app.best_spot_sample(Sample {
        at_ms: base,
        net: NetworkKind::Mobile5gAdv,
        level: 3,
        dbm: None,
        rtt_ms: None,
        wifi_ms: None,
    });
    assert!(app.best_spot_state().samples.is_empty());
    assert!(!app.best_spot_running(base));
}

#[test]
fn the_session_stops_after_two_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let now = Instant::now();
    let base = now_ms();

    app.best_spot_start(now);
    assert!(
        app.best_spot_running(base + 119_000),
        "one second before the end"
    );
    assert!(
        !app.best_spot_running(base + 120_000),
        "the session is over"
    );

    // A sample after the end of the session does not land.
    app.best_spot_sample(Sample {
        at_ms: base + 130_000,
        net: NetworkKind::Mobile5gAdv,
        level: 3,
        dbm: None,
        rtt_ms: None,
        wifi_ms: None,
    });
    assert!(app.best_spot_state().samples.is_empty());
}

#[test]
fn a_new_session_forgets_the_old_samples_and_marks() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let now = Instant::now();
    let base = now_ms();

    app.best_spot_start(now);
    app.best_spot_sample(Sample {
        at_ms: base,
        net: NetworkKind::Mobile5gAdv,
        level: 3,
        dbm: None,
        rtt_ms: None,
        wifi_ms: None,
    });
    app.best_spot_mark(now);
    assert_eq!(app.best_spot_state().samples.len(), 1);

    app.best_spot_start(now);
    assert!(app.best_spot_state().samples.is_empty());
    assert!(app.best_spot_state().marks.is_empty());
}
