//! One fact reads the same on every page. The session rows and the
//! session tiles must use one field, one unit and one set of words.

use jafnify_core::drop_history::{DropRecord, Event};
use jafnify_core::scope::{ScopeSession, TileScope};
use jafnify_core::session::SessionStats;
use jafnify_core::tiles::{average_tile, session_drop_count};
use jafnify_core::ui::details::{session_rows, session_rows_with_drops};
use jafnify_core::usage::{UsageSummary, UsageWindow};

const START: i64 = 1_700_000_000_000;

fn stats() -> SessionStats {
    SessionStats {
        started_at: START,
        connected_ms: 2_000_000,
        up_bytes: 3_400_000_000,
        down_bytes: 2_500_000_000,
        avg_up_bps: 355_900,
        avg_down_bps: 233_600,
        peak_up_bps: 51_300_000,
        peak_down_bps: 50_300_000,
        drops: 6,
        peer_endpoint: "10.0.0.2:9797".to_string(),
        ..SessionStats::default()
    }
}

fn value<'a>(rows: &'a [jafnify_core::ui::details::DetailRow], label: &str) -> &'a str {
    rows.iter()
        .find(|row| row.label == label)
        .unwrap_or_else(|| panic!("the rows have {label}: {rows:?}"))
        .value
        .as_str()
}

#[test]
fn the_session_rows_read_down_and_up_in_the_tile_units() {
    let rows = session_rows(&stats());
    // Down comes first, the same order as the tile's facts.
    let down = rows.iter().position(|row| row.label == "Down").unwrap();
    let up = rows.iter().position(|row| row.label == "Up").unwrap();
    assert!(down < up, "down comes first: {rows:?}");
    assert_eq!(value(&rows, "Down"), "2.5 GB");
    assert_eq!(value(&rows, "Up"), "3.4 GB");
    assert_eq!(
        value(&rows, "Average"),
        "233.6 KB/s down, 355.9 KB/s up"
    );
    assert_eq!(
        value(&rows, "Peak"),
        "50.3 MB/s down, 51.3 MB/s up"
    );
    assert!(
        !rows.iter().any(|row| row.label == "Sent" || row.label == "Received"),
        "the old sent/received words are gone: {rows:?}"
    );
}

#[test]
fn the_session_drops_come_from_the_drop_events_not_the_tracker() {
    // The tracker counted a stop on the phone as a drop, and the drop history
    // has all six drops from before this session. The rows must show the drop
    // events of this session (none), not the tracker's counter (6).
    let rows = session_rows_with_drops(&stats(), 0);
    assert_eq!(value(&rows, "Drops"), "0");
}

#[test]
fn the_session_drop_count_reads_only_the_drops_inside_the_session() {
    let session = ScopeSession {
        started_at_ms: START,
        up_bytes: 0,
        down_bytes: 0,
        peak_down_bps: 0,
    };
    let now = START + 60_000;
    let events = vec![
        Event::Drop(DropRecord {
            at_ms: START - 3_600_000,
            reason: "test".to_string(),
            phone: None,
            wifi: None,
            down_ms: Some(1_000),
            exit: None,
            link: Default::default(),
        }),
        Event::Stopped {
            at_ms: START + 10_000,
            down_ms: Some(2_000),
        },
        Event::Away {
            at_ms: START + 20_000,
            down_ms: Some(3_000),
        },
        Event::Drop(DropRecord {
            at_ms: START + 30_000,
            reason: "test".to_string(),
            phone: None,
            wifi: None,
            down_ms: Some(4_000),
            exit: None,
            link: Default::default(),
        }),
    ];
    assert_eq!(session_drop_count(&events, &session, now), 1);
}

#[test]
fn the_average_tile_names_the_scope_of_its_peak() {
    let summary = UsageSummary {
        window: UsageWindow::Today,
        up_bytes: 3_400_000_000,
        down_bytes: 2_500_000_000,
        connected_ms: 2_000_000,
        average_up_bps: 355_900,
        average_down_bps: 233_600,
        peak_up_bps: 51_300_000,
        peak_down_bps: 50_300_000,
        lowest_up_bps: 0,
        lowest_down_bps: 0,
        busiest: Vec::new(),
        network: None,
    };
    let tile = average_tile(&summary, None, START, TileScope::Today).expect("the average tile");
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts[0], "Peak 50.3 MB/s down, 51.3 MB/s up today");

    let session = ScopeSession {
        started_at_ms: START,
        up_bytes: 3_400_000_000,
        down_bytes: 2_500_000_000,
        peak_down_bps: 50_300_000,
    };
    let tile = average_tile(&summary, Some(&session), START + 2_000_000, TileScope::Session)
        .expect("the session average tile");
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts[0], "Peak 50.3 MB/s down in session");
}
