//! The good hours of the laptop: the hours in which the phone's network was
//! usually quick and the hours in which it was usually busy. The learning
//! lives in a private file of this computer; nothing goes to the peer.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jafnify_core::good_hours::{HourRecord, Sample};
use jafnify_core::phone_stats::NetworkKind;
use jafnify_linux::app::AppBuilder;
use jafnify_linux::control::{Request, Response, Snapshot};
use jafnify_linux::flags::good_hours_lines;
use jafnify_linux::good_hours_store::{GoodHoursStore, MAX_LINES};

/// One finished hour.
fn record(hour_start_ms: i64, dns_ms: Option<u32>) -> HourRecord {
    HourRecord {
        hour_start_ms,
        samples: 12,
        dns_ms,
        five_g_pct: 100,
        signal: Some(3),
        temp_c_x10: None,
        thr: None,
        partial: false,
    }
}

/// One sample of the phone.
fn sample(at_ms: i64, dns_ms: Option<u32>) -> Sample {
    Sample {
        at_ms,
        net: NetworkKind::Mobile5gAdv,
        signal: Some(3),
        dbm: Some(-97),
        dns_ms,
        temp_c_x10: None,
        thr: None,
    }
}

/// The hour of a timestamp, floored.
fn hour_start(at_ms: i64) -> i64 {
    at_ms.div_euclid(3_600_000) * 3_600_000
}

#[test]
fn the_file_is_private_and_holds_the_finished_hours() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good_hours.tsv");
    let mut store = GoodHoursStore::load(&path);
    store
        .append(record(1_700_000_000_000, Some(42)).line())
        .unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the good-hours file must be private");
    assert_eq!(store.lines().len(), 1, "{:?}", store.lines());

    let reread = GoodHoursStore::load(&path);
    assert_eq!(reread.records().len(), 1, "{:?}", reread.lines());
    assert_eq!(reread.records()[0].dns_ms, Some(42));
}

#[test]
fn the_file_keeps_the_newest_lines_and_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good_hours.tsv");
    let mut store = GoodHoursStore::load(&path);
    for index in 0..MAX_LINES * 3 {
        store
            .append(record(index as i64 * 3_600_000, Some(40)).line())
            .unwrap();
    }
    assert_eq!(store.lines().len(), MAX_LINES);
    assert_eq!(
        store.lines()[MAX_LINES - 1],
        record((MAX_LINES * 3 - 1) as i64 * 3_600_000, Some(40)).line()
    );

    let text = fs::read_to_string(&path).unwrap();
    let written: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        written.len() <= 2 * MAX_LINES,
        "the file must be compacted, it holds {} lines",
        written.len()
    );
    assert_eq!(
        written[written.len() - 1],
        record((MAX_LINES * 3 - 1) as i64 * 3_600_000, Some(40)).line()
    );
}

#[test]
fn a_crash_loses_at_most_the_current_hour() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    let path = config.join("good_hours.tsv");
    let mut app = AppBuilder::new(config.clone()).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(true, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    assert!(
        !path.exists(),
        "the current hour is only in memory, a crash loses it"
    );
    assert_eq!(app.good_hours_records().len(), 1, "the hour still counts");

    // The next hour finishes the first one: it is written to the file.
    app.good_hours_sample(sample(base + 3_600_000, Some(40)));
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("rtt=40"), "{text}");
}

#[test]
fn a_clean_stop_keeps_the_hour_in_the_making_as_a_half_hour() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    let path = config.join("good_hours.tsv");
    let mut app = AppBuilder::new(config.clone()).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(true, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    app.flush_good_hours();

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("part=1"), "the hour is not over: {text}");
    assert_eq!(
        app.good_hours_lines(base, 0),
        vec!["Learning the busy hours: 0 of 3 days so far".to_string()],
        "a half hour is not a day"
    );
}

#[test]
fn the_file_holds_no_location_no_ip_no_endpoint_and_no_peer_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good_hours.tsv");
    let mut store = GoodHoursStore::load(&path);
    store
        .append(record(1_700_000_000_000, Some(42)).line())
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();

    for word in ["peer", "endpoint", "location", "addr", "host", "phone"] {
        assert!(
            !text.contains(word),
            "the file must not hold {word}: {text}"
        );
    }
    let looks_like_an_address = text.split_whitespace().any(|token| {
        let parts: Vec<&str> = token.split('.').collect();
        parts.len() == 4 && parts.iter().all(|part| part.parse::<u8>().is_ok())
    });
    assert!(
        !looks_like_an_address,
        "the file must not hold an address: {text}"
    );
}

#[test]
fn the_file_holds_nothing_but_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good_hours.tsv");
    let mut store = GoodHoursStore::load(&path);
    store
        .append(record(1_700_000_000_000, Some(42)).line())
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();

    // The prefix and the version come first; every other token is a key and
    // a number, so no name, no address and no id can hide in the file.
    for token in text.split_whitespace().skip(2) {
        let Some((key, value)) = token.split_once('=') else {
            panic!("the file must hold key=value pairs only: {token} in {text}");
        };
        assert!(!key.is_empty() && !value.is_empty(), "{token} in {text}");
        assert!(
            key.chars().all(|letter| letter.is_ascii_alphanumeric()),
            "the key must be a plain name: {token} in {text}"
        );
        assert!(
            value.chars().all(|digit| digit.is_ascii_digit()),
            "the value must be a number: {token} in {text}"
        );
    }
}

#[test]
fn the_file_keeps_one_line_per_hour() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good_hours.tsv");
    let mut store = GoodHoursStore::load(&path);
    let hour = 1_700_000_000_000;
    store.append(record(hour, Some(40)).line()).unwrap();
    // The same hour again, with a better number: the old line goes.
    store.append(record(hour, Some(55)).line()).unwrap();
    assert_eq!(store.lines().len(), 1, "{:?}", store.lines());
    assert_eq!(
        store.lines()[0],
        record(hour, Some(55)).line(),
        "the newer line wins"
    );

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    let reread = GoodHoursStore::load(&path);
    assert_eq!(reread.records().len(), 1);
    assert_eq!(reread.records()[0].dns_ms, Some(55));
}

#[test]
fn the_downgrade_guard_accepts_the_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    fs::create_dir_all(&config).unwrap();
    let path = config.join("good_hours.tsv");
    fs::write(
        &path,
        "JFG1 1 hour=1700000000000 n=12 rtt=42 g5=100 sig=3\n",
    )
    .unwrap();

    // A build that does not know the file starts with it there and leaves it
    // alone.
    let app = AppBuilder::new(config.clone()).build();
    assert_eq!(app.good_hours_records().len(), 1, "the file is read");
    assert!(path.exists(), "the file must survive");
    let lines = app.good_hours_lines(1_700_000_000_000, 0);
    assert!(
        lines
            .first()
            .is_some_and(|line| line.starts_with("Learning the busy hours:")),
        "{lines:?}"
    );
}

#[test]
fn nothing_is_sampled_while_the_flag_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(false, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    assert!(
        app.good_hours_records().is_empty(),
        "a sample while the switch is off must not land"
    );
}

#[test]
fn nothing_is_sampled_while_the_link_is_not_connected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(true, false, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    assert!(
        app.good_hours_records().is_empty(),
        "a sample while the link is down must not land"
    );
}

#[test]
fn nothing_is_sampled_when_the_phone_summary_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);

    // The summary is older than the freshness rule: it says nothing about the
    // phone now.
    app.set_good_hours_gate(true, true, Some(31));
    app.good_hours_sample(sample(base, Some(40)));
    assert!(
        app.good_hours_records().is_empty(),
        "a sample from a stale summary must not land"
    );
}

#[test]
fn a_summary_of_exactly_thirty_seconds_still_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);

    // The edge of the freshness rule: thirty seconds is still now.
    app.set_good_hours_gate(true, true, Some(30));
    app.good_hours_sample(sample(base, Some(40)));
    assert_eq!(app.good_hours_records().len(), 1, "the sample landed");
}

#[test]
fn a_sample_lands_while_the_switch_is_on_and_the_link_is_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(true, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    assert_eq!(app.good_hours_records().len(), 1, "the sample landed");
}

#[test]
fn the_clear_command_deletes_the_file_and_the_current_hour() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    let path = config.join("good_hours.tsv");
    let mut app = AppBuilder::new(config.clone()).build();
    let base = hour_start(1_700_000_000_000);

    app.set_good_hours_gate(true, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));
    app.good_hours_sample(sample(base + 3_600_000, Some(40)));
    assert!(path.exists(), "the first hour is written");

    app.clear_good_hours();
    assert!(!path.exists(), "the file is deleted");
    assert!(
        app.good_hours_records().is_empty(),
        "the current hour is gone too"
    );
}

#[test]
fn the_get_command_returns_the_lines() {
    let text = serde_json::to_string(&Request::GetGoodHours).unwrap();
    assert_eq!(text, r#"{"cmd":"get_good_hours"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&text).unwrap(),
        Request::GetGoodHours
    );

    let response = Response::GoodHours {
        lines: vec![
            "Learning the busy hours: 2 of 3 days so far".to_string(),
            "Usually quick: 02:00-07:00 (DNS about 28 ms)".to_string(),
        ],
    };
    let text = serde_json::to_string(&response).unwrap();
    assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), response);
}

#[test]
fn an_older_snapshot_without_the_good_hours_field_still_loads() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("good_hours_lines");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(old.good_hours_lines.is_empty());
}

#[test]
fn the_good_hours_are_on_the_statistics_page_only_when_the_flag_is_on() {
    let gui = fs::read_to_string("src/bin/jafnify-gui.rs").unwrap();
    let body = gui
        .split("fn statistics_body")
        .nth(1)
        .expect("the window has a Statistics body");
    assert!(
        body.contains("if self.good_hours_on()"),
        "the good-hours section must follow its switch"
    );
    assert!(
        body.contains("self.good_hours_block(ui, colours)"),
        "the Statistics page must draw the good-hours section"
    );

    let on = gui
        .split("fn good_hours_on(&self) -> bool")
        .nth(1)
        .expect("the window knows the good-hours switch");
    assert!(on.contains("\"good_hours\""), "{on}");
}

#[test]
fn the_hours_command_prints_the_same_lines() {
    // The lines the CLI prints are the lines the daemon hands out.
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg")).build();
    let base = hour_start(1_700_000_000_000);
    app.set_good_hours_gate(true, true, Some(0));
    app.good_hours_sample(sample(base, Some(40)));

    let lines = good_hours_lines(true, app.good_hours_lines(1_700_000_000_000, 0));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0], "Learning the busy hours: 0 of 3 days so far",
        "{lines:?}"
    );
    assert_eq!(
        lines[1], "Places change the network too; this learns from where the laptop has been.",
        "{lines:?}"
    );

    let cli = fs::read_to_string("src/bin/jafnify.rs").unwrap();
    assert!(
        cli.contains("Command::Hours"),
        "the CLI needs the hours command"
    );
    assert!(
        cli.contains("Request::GetGoodHours"),
        "the CLI asks the daemon for the lines"
    );
}

#[test]
fn the_section_adds_nothing_while_the_switch_is_off() {
    let lines = good_hours_lines(false, vec!["Usually quick: 02:00".to_string()]);
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn the_section_shows_the_lines_and_the_note_about_the_places() {
    let lines = good_hours_lines(
        true,
        vec![
            "Learning the busy hours: 2 of 3 days so far".to_string(),
            "Usually quick: 02:00-07:00 (DNS about 28 ms)".to_string(),
        ],
    );
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[2], "Places change the network too; this learns from where the laptop has been.",
        "{lines:?}"
    );
}

/// The file name is fixed, so an old build that does not know it leaves it
/// alone.
#[test]
fn the_file_name_is_stable() {
    assert_eq!(jafnify_linux::good_hours_store::FILE_NAME, "good_hours.tsv");
    let _ = Path::new(jafnify_linux::good_hours_store::FILE_NAME);
}
