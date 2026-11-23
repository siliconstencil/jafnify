//! The guard of the one window: every tile of a home screen talks about one
//! window.
//!
//! A tile shows a number, marks and a line of facts. When the three do not
//! come from the same window, the screen says one thing and draws another:
//! the number of the session next to the drops of yesterday. The test walks
//! every tile the home model can build, with the events and the samples
//! inside and outside the window, and checks that they agree.

use jafnify_core::drop_history::{DropRecord, Event};
use jafnify_core::phone_stats::{NetworkKind, PhoneStats};
use jafnify_core::scope::{in_scope, ScopeSession, ScopeTotals, TileScope};
use jafnify_core::tiles::{
    dns_tile, drops_tile, heat_tile, phone_home, phone_tile, signal_tile, speed_tile, today_tile,
    wifi_tile, CellMark, Measure, PhoneHomeInput, PhoneShareState, Tile, TrendBuffer, TrendKind,
    Trends,
};
use jafnify_core::usage::{local_day, UsageSummary, UsageWindow};
use jafnify_core::wifi_leg::WifiLeg;
use jafnify_core::wifi_range::{range_tile, RangeReport};

/// 2026-01-01T00:00:00Z.
const BASE: i64 = 1_767_225_600_000;
const HOUR: i64 = 3_600_000;
/// The moment the tiles are built at: 2026-01-02T02:00:00Z.
const NOW: i64 = BASE + 26 * HOUR;
/// The start of the sharing session: 2026-01-01T10:00:00Z.
const SESSION_START: i64 = BASE + 10 * HOUR;
/// The months of the fact times.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn stats() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        rtt_ms: Some(34),
        dbm: Some(-97),
        battery: Some(74),
        temp_c_x10: Some(315),
        ..Default::default()
    }
}

fn usage() -> UsageSummary {
    UsageSummary {
        window: UsageWindow::Today,
        up_bytes: 208_000_000,
        down_bytes: 695_000_000,
        connected_ms: 3_600_000,
        average_up_bps: 57_778,
        average_down_bps: 193_055,
        peak_up_bps: 120_000,
        peak_down_bps: 49_900_000,
        lowest_up_bps: 0,
        lowest_down_bps: 0,
        busiest: Vec::new(),
        network: Some(NetworkKind::Mobile5gAdv),
    }
}

/// The stats of a phone that moves bytes: the speed tile needs a rate.
fn moving_stats() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        rtt_ms: Some(34),
        dbm: Some(-97),
        battery: Some(74),
        temp_c_x10: Some(315),
        up_bps: 300_000,
        down_bps: 1_200_000,
        ..Default::default()
    }
}

fn drop(at_ms: i64) -> DropRecord {
    DropRecord {
        at_ms,
        reason: "no-data".to_string(),
        phone: None,
        wifi: None,
        down_ms: Some(113_000),
        exit: None,
        link: Default::default(),
    }
}

fn session_at(started_at_ms: i64) -> ScopeSession {
    ScopeSession {
        started_at_ms,
        up_bytes: 0,
        down_bytes: 0,
        peak_down_bps: 0,
    }
}

/// The cells of the hour strip of a tile: the hour, the mark and whether the
/// hour lies inside the window of the tile.
fn cells_of(tile: &Tile) -> Vec<(u8, CellMark, bool)> {
    match &tile
        .trend
        .as_ref()
        .expect("the tile draws the hour strip")
        .kind
    {
        TrendKind::Hours { cells } => cells
            .iter()
            .map(|cell| (cell.hour, cell.mark, cell.in_scope))
            .collect(),
        other => panic!("the tile draws the hour strip, got {other:?}"),
    }
}

/// The facts of a tile.
fn facts_of(tile: &Tile) -> Vec<String> {
    tile.facts
        .as_ref()
        .map(|facts| facts.lines.clone())
        .unwrap_or_default()
}

/// The local hour of a moment, in the local time of the tests.
fn local_hour_of(at_ms: i64) -> u8 {
    ((at_ms / HOUR) % 24) as u8
}

/// Whether a fact line carries the day of its time.
fn carries_a_day(line: &str) -> bool {
    line.starts_with("yesterday ")
        || MONTHS
            .iter()
            .any(|month| line.starts_with(&format!("{month} ")))
}

#[test]
fn the_drops_tile_counts_and_draws_the_same_window() {
    // The walk: every scope of the tile and every place of the events in or
    // out of the window of the scope.
    let session = session_at(SESSION_START);
    let sets: [&[i64]; 3] = [
        // The events of the last day: one before the strip, one before the
        // session, one inside it and one of today.
        &[
            BASE + HOUR,
            BASE + 5 * HOUR,
            BASE + 12 * HOUR,
            BASE + 24 * HOUR,
        ],
        // Only the events before the session: the session and today have
        // nothing, the last day has one.
        &[BASE + HOUR, BASE + 5 * HOUR],
        // Only the events of today.
        &[BASE + 24 * HOUR],
    ];
    for scope in [TileScope::Session, TileScope::Today, TileScope::Last24Hours] {
        for moments in sets {
            let events: Vec<Event> = moments
                .iter()
                .map(|at_ms| Event::Drop(drop(*at_ms)))
                .collect();
            let tile = drops_tile(&events, NOW, 0, scope, Some(&session)).expect("the drops tile");
            let window: Vec<i64> = moments
                .iter()
                .copied()
                .filter(|at_ms| in_scope(*at_ms, scope, NOW, 0, Some(&session)))
                .collect();
            // The number counts the drops of the window.
            let number = tile.big.as_ref().expect("the tile leads with a number");
            assert_eq!(
                number.value,
                window.len().to_string(),
                "the number counts the drops of the window of {scope:?}"
            );
            // The red cells are the hours of the drops of the window, and no
            // red cell lies outside the window.
            let cells = cells_of(&tile);
            for (hour, mark, inside) in &cells {
                if *mark == CellMark::Drop {
                    assert!(
                        *inside,
                        "a red cell is a drop of the window of {scope:?}: {cells:?}"
                    );
                    assert!(
                        window.iter().any(|at_ms| local_hour_of(*at_ms) == *hour),
                        "a red cell is the hour of a drop of the window of {scope:?}: {cells:?}"
                    );
                }
            }
            // Every drop of the window has its red hour.
            for at_ms in &window {
                let hour = local_hour_of(*at_ms);
                assert!(
                    cells
                        .iter()
                        .any(|(cell_hour, mark, _)| *cell_hour == hour && *mark == CellMark::Drop),
                    "the drop of the window of {scope:?} has its red hour: {cells:?}"
                );
            }
            // The first fact line is about the newest event of the window.
            let facts = facts_of(&tile);
            match window.iter().max() {
                Some(newest) => {
                    let clock = format!("{:02}:00", local_hour_of(*newest));
                    assert!(
                        facts[0].contains(&clock),
                        "the first fact is the newest event of the window of {scope:?}: {facts:?}"
                    );
                    // A time of another day carries its day, a time of today
                    // carries none.
                    if local_day(*newest, 0) != local_day(NOW, 0) {
                        assert!(
                            carries_a_day(&facts[0]),
                            "the time of another day carries its day: {facts:?}"
                        );
                    } else {
                        assert!(
                            facts[0].starts_with(&clock),
                            "the time of today carries no day: {facts:?}"
                        );
                    }
                }
                None => {
                    assert_eq!(facts[0], format!("No drops {}", scope.fact_subject()));
                    assert!(
                        facts.len() > 1 && facts[1].starts_with("Last: "),
                        "the tile names the newest event it does not count: {facts:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_time_of_the_last_event_carries_its_day() {
    // The line that names the newest event the tile does not count says the
    // day of that event when the event is not of today.
    let session = session_at(SESSION_START);
    let events: Vec<Event> = vec![Event::Drop(drop(BASE + 5 * HOUR))];
    let tile = drops_tile(&events, NOW, 0, TileScope::Session, Some(&session)).expect("the tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops this session");
    assert!(
        facts[1].starts_with("Last: yesterday 05:00"),
        "the last event of yesterday says its day: {facts:?}"
    );
}

#[test]
fn every_line_that_shows_numbers_says_its_window() {
    // Every tile that draws a line of measurements says the window of that
    // line, the way the DNS tile does. The window is the caption of the
    // drawing above it.
    let mut trends = Trends::new();
    // The samples of the last five minutes and one of an hour ago: the ring
    // keeps the last five minutes only.
    for second in 0..300 {
        trends.push(NOW - 300_000 + second * 1_000, &stats(), Some(40));
    }
    trends.push(NOW - 3_600_000, &stats(), Some(40));
    let leg = WifiLeg::new(40, 3, 30);
    let laptop = vec![
        dns_tile(Some(34), trends.dns(), Some(100)).expect("the dns tile"),
        signal_tile(&stats(), trends.signal()).expect("the signal tile"),
        wifi_tile(&leg, trends.wifi(), None).expect("the wifi tile"),
        phone_tile(Some(315), Some(3), None, Some(74), trends.phone(), None)
            .expect("the phone tile"),
    ];
    for tile in &laptop {
        assert!(
            matches!(
                tile.trend.as_ref().map(|trend| &trend.kind),
                Some(TrendKind::Line { .. })
            ),
            "the tile draws a line: {}",
            tile.id
        );
        let facts = facts_of(tile);
        assert!(
            facts[0].contains("in 5 min"),
            "the first fact of {} says the window of its line: {facts:?}",
            tile.id
        );
    }
    // The home of the phone: every tile that draws a line says its window.
    let today = usage();
    let session = session_at(SESSION_START);
    let phone = phone_home(&PhoneHomeInput {
        state: PhoneShareState::Sharing,
        state_line: None,
        notice: None,
        uptime_secs: Some(600),
        today: Some(&today),
        stats: Some(&moving_stats()),
        network_permission: true,
        trends: &trends,
        charging: Some(false),
        battery_saver: Some(false),
        show_dns: true,
        show_battery: true,
        today_scope: TileScope::Today,
        speed_scope: TileScope::Session,
        average_scope: TileScope::Session,
        now_ms: SESSION_START + 10 * HOUR,
        session: Some(&session),
        range: None,
    });
    for tile in &phone.tiles {
        if !matches!(
            tile.trend.as_ref().map(|trend| &trend.kind),
            Some(TrendKind::Line { .. })
        ) {
            continue;
        }
        let facts = facts_of(tile);
        assert!(
            facts[0].contains("in 5 min"),
            "the first fact of {} says the window of its line: {facts:?}",
            tile.id
        );
    }
}

#[test]
fn the_home_of_the_laptop_keeps_the_one_window() {
    // The tiles of the laptop: the number, the marks and the facts of each
    // tile talk about the same window.
    let mut trends = Trends::new();
    for second in 0..300 {
        trends.push(NOW - 300_000 + second * 1_000, &stats(), Some(40));
    }
    let totals = ScopeTotals {
        up_bytes: 208_000_000,
        down_bytes: 695_000_000,
        peak_down_bps: 49_900_000,
    };
    let tiles = vec![
        dns_tile(Some(34), trends.dns(), Some(100)).expect("the dns tile"),
        signal_tile(&stats(), trends.signal()).expect("the signal tile"),
        wifi_tile(&WifiLeg::new(40, 3, 30), trends.wifi(), None).expect("the wifi tile"),
        today_tile(&totals, TileScope::Today, Measure::Laptop).expect("the today tile"),
    ];
    for tile in &tiles {
        // The today tile sums over its scope and names it in its label, not
        // in a line of facts: it draws no line of measurements.
        if tile.id == "today" {
            assert!(
                !matches!(
                    tile.trend.as_ref().map(|trend| &trend.kind),
                    Some(TrendKind::Line { .. })
                ),
                "the today tile draws no line of measurements"
            );
            continue;
        }
        let facts = facts_of(tile);
        assert!(
            facts[0].contains("in 5 min"),
            "the first fact of {} says the window of its line: {facts:?}",
            tile.id
        );
    }
}

#[test]
fn an_outage_left_open_before_the_session_is_not_still_open() {
    // An older daemon left the reset of an app update open
    // and a new session began right after. The tile must not say "not back
    // yet" while the link is up: the outage ended when the session began.
    let start = SESSION_START;
    let mut record = drop(start - 5_000);
    record.down_ms = None;
    let events = vec![Event::Drop(record)];
    let session = session_at(start);
    for scope in [TileScope::Session, TileScope::Today, TileScope::Last24Hours] {
        let tile = drops_tile(&events, NOW, 0, scope, Some(&session)).expect("the drops tile");
        let facts = facts_of(&tile);
        assert!(
            facts.iter().all(|line| !line.contains("not back yet")),
            "an outage before the session is over in {scope:?}: {facts:?}"
        );
    }
    // An outage inside the session that is still open stays open.
    let mut open = drop(start + 60_000);
    open.down_ms = None;
    let tile = drops_tile(
        &[Event::Drop(open)],
        NOW,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    assert!(
        facts_of(&tile)[0].contains("not back yet"),
        "an outage of the session can still be open: {:?}",
        facts_of(&tile)
    );
}

#[test]
fn the_phone_heat_and_speed_tiles_say_their_window() {
    // The heat tile draws the temperatures of the last five minutes and the
    // speed tile the rates of the last five minutes: both say the window of
    // their line, the way the laptop tiles do.
    let mut trends = Trends::new();
    for second in 0..300 {
        trends.push(NOW - 300_000 + second * 1_000, &moving_stats(), Some(40));
    }
    let heat = heat_tile(Some(315), Some(2), None, trends.phone()).expect("the heat tile");
    let facts = facts_of(&heat);
    assert!(
        facts[0].contains("in 5 min"),
        "the heat tile says the window of its line: {facts:?}"
    );
    assert!(
        facts[0].contains(" C"),
        "the heat tile says the unit of its line: {facts:?}"
    );
    let speed =
        speed_tile(&moving_stats(), trends.speed(), TileScope::Session, 0).expect("the speed tile");
    let facts = facts_of(&speed);
    assert!(
        facts[0].contains("in 5 min"),
        "the speed tile says the window of its line: {facts:?}"
    );
}

#[test]
fn the_range_tile_says_its_window_and_a_flat_line_is_one_value() {
    let report = RangeReport {
        level_dbm: -60,
        rate_100k: Some(1300),
        missed_beacons: false,
    };

    // A moving line: the window names both ends.
    let mut moving = TrendBuffer::new();
    for (i, dbm) in [-80, -76, -72, -66, -60].iter().enumerate() {
        moving.push(NOW - 240_000 + i as i64 * 60_000, *dbm);
    }
    let tile = range_tile(&report, &moving);
    let facts = facts_of(&tile);
    assert!(facts[0].contains("in 5 min"), "{facts:?}");
    assert!(facts[0].starts_with("-80 to -60 dBm"), "{facts:?}");

    // A flat line is one value, never a range.
    let mut flat = TrendBuffer::new();
    for second in 0..60 {
        flat.push(NOW - 60_000 + second * 1_000, -60);
    }
    let flat_tile = range_tile(&report, &flat);
    assert_eq!(facts_of(&flat_tile)[0], "-60 dBm in 5 min");

    // The home of the phone draws the same tile while it shares the range.
    let trends = Trends::new();
    let home = phone_home(&PhoneHomeInput {
        state: PhoneShareState::Sharing,
        state_line: None,
        notice: None,
        uptime_secs: Some(600),
        today: None,
        stats: Some(&stats()),
        network_permission: true,
        trends: &trends,
        charging: Some(false),
        battery_saver: Some(false),
        show_dns: false,
        show_battery: false,
        today_scope: TileScope::Today,
        speed_scope: TileScope::Session,
        average_scope: TileScope::Today,
        now_ms: BASE,
        session: None,
        range: Some((report, &moving)),
    });
    let tile = home
        .tiles
        .iter()
        .find(|tile| tile.id == "range")
        .expect("the phone range tile");
    let facts = facts_of(tile);
    assert!(facts[0].contains("in 5 min"), "{facts:?}");
    assert!(facts[0].starts_with("-80 to -60 dBm"), "{facts:?}");
}

#[test]
fn the_drops_tile_names_the_range_when_the_link_was_out_of_range() {
    use jafnify_core::drop_history::HINT_OUT_OF_RANGE;
    let session = session_at(SESSION_START);
    let mut record = drop(NOW - 60_000);
    record.link.weakest_dbm = Some(-81);
    assert_eq!(record.hint(), HINT_OUT_OF_RANGE);
    let events = vec![Event::Drop(record)];
    let tile = drops_tile(&events, NOW, 0, TileScope::Session, Some(&session)).expect("the tile");
    let facts = facts_of(&tile);
    assert!(
        facts.iter().any(|fact| fact == HINT_OUT_OF_RANGE),
        "the drop fact says the range: {facts:?}"
    );
}

#[test]
fn the_wifi_tile_note_is_the_phone_wireguard_loss() {
    let leg = WifiLeg::new(40, 3, 30);
    let trend = TrendBuffer::new();
    let with = wifi_tile(&leg, &trend, Some(3)).expect("the tile");
    assert_eq!(with.note, Some("WireGuard loss 3%".to_string()));
    let without = wifi_tile(&leg, &trend, None).expect("the tile");
    assert_eq!(without.note, None);
}

#[test]
fn the_heat_and_phone_tiles_add_the_forecast_line() {
    let trend = TrendBuffer::new();
    // The phone HEAT tile and the laptop PHONE tile use the same core words.
    let heat = heat_tile(Some(315), Some(0), Some(105), &trend).expect("the heat tile");
    assert!(
        facts_of(&heat)
            .iter()
            .any(|fact| fact == "Likely throttling within 30 s"),
        "{:?}",
        facts_of(&heat)
    );
    let phone = phone_tile(Some(315), Some(0), Some(105), Some(74), &trend, None)
        .expect("the phone tile");
    let facts = facts_of(&phone);
    assert!(
        facts.iter().any(|fact| fact == "Likely throttling within 30 s"),
        "{facts:?}"
    );
    // Below the threshold the forecast says nothing.
    let quiet = heat_tile(Some(315), Some(0), Some(84), &trend).expect("the heat tile");
    assert!(
        !facts_of(&quiet).iter().any(|fact| fact.contains("Likely")),
        "{:?}",
        facts_of(&quiet)
    );
}
