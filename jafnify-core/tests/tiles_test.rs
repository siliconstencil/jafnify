//! The tile home: the tiles of the laptop's home screen.
//!
//! Pure model: the daemon hands over what it already knows and gets back the
//! tiles to draw. The trend buffer is a ring of the last five minutes that
//! lives in memory only.

use jafnify_core::drop_history::{DropRecord, Event};
use jafnify_core::good_hours::{LocalHour, Summary, Verdict};
use jafnify_core::phone_stats::{NetworkKind, PhoneStats};
use jafnify_core::scope::{ScopeSession, ScopeTotals, TileScope};
use jafnify_core::tiles::{
    dns_tile, drops_tile, hours_strip, phone_tile, signal_tile, status_card, today_tile, wifi_tile,
    CellMark, Facts, Measure, PhoneShareState, StatusInput, Tile, Tone, TrendBuffer, TrendKind,
    Trends, MAX_TREND_POINTS, TREND_SAMPLES, TREND_WINDOW_MS,
};
use jafnify_core::usage::{UsageSummary, UsageWindow};
use jafnify_core::wifi_leg::WifiLeg;

/// 2026-01-01T00:00:00Z.
const BASE: i64 = 1_767_225_600_000;
const HOUR: i64 = 3_600_000;

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

fn drop(at_ms: i64, down_ms: Option<u64>) -> DropRecord {
    DropRecord {
        at_ms,
        reason: "no-data".to_string(),
        phone: None,
        wifi: None,
        down_ms,
        exit: None,
        link: Default::default(),
    }
}

/// The numbers of the today tile.
fn totals() -> ScopeTotals {
    ScopeTotals {
        up_bytes: 208_000_000,
        down_bytes: 695_000_000,
        peak_down_bps: 49_900_000,
    }
}

/// The numbers of a summary, over a scope.
fn totals_of(summary: &UsageSummary) -> ScopeTotals {
    ScopeTotals {
        up_bytes: summary.up_bytes,
        down_bytes: summary.down_bytes,
        peak_down_bps: summary.peak_down_bps,
    }
}

/// The events of the drop file.
fn events(records: Vec<DropRecord>) -> Vec<Event> {
    records.into_iter().map(Event::Drop).collect()
}

#[test]
fn a_tile_without_data_is_not_shown() {
    assert!(dns_tile(None, &TrendBuffer::new(), None).is_none());
}

#[test]
fn the_dns_tile_leads_with_the_answer_time() {
    let tile = dns_tile(Some(34), &TrendBuffer::new(), None).expect("the tile has data");
    assert_eq!(tile.id, "dns");
    assert_eq!(tile.label, "DNS ANSWER");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "34");
    assert_eq!(big.unit, Some("ms".to_string()));
    assert_eq!(big.tone, Tone::Muted);
}

#[test]
fn the_dns_tile_shows_the_range_of_the_last_five_minutes() {
    let mut trend = TrendBuffer::new();
    trend.push(0, 28);
    trend.push(1_000, 61);
    let tile = dns_tile(Some(34), &trend, None).expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the range");
    assert_eq!(facts.lines, vec!["28-61 ms in 5 min".to_string()]);
}

#[test]
fn the_dns_tile_has_no_badge_without_an_overall_median() {
    let tile = dns_tile(Some(34), &TrendBuffer::new(), None).expect("the tile has data");
    assert!(tile.badge.is_none());
}

#[test]
fn the_dns_tile_is_marked_quick_when_the_answer_time_is_below_the_median() {
    let tile = dns_tile(Some(30), &TrendBuffer::new(), Some(100)).expect("the tile has data");
    let badge = tile.badge.expect("the badge says the hour is quick");
    assert_eq!(badge.text, "Quick");
    assert_eq!(badge.tone, Tone::Ok);
}

#[test]
fn the_dns_tile_is_marked_busy_when_the_answer_time_is_above_the_median() {
    let tile = dns_tile(Some(200), &TrendBuffer::new(), Some(100)).expect("the tile has data");
    let badge = tile.badge.expect("the badge says the hour is busy");
    assert_eq!(badge.text, "Busy");
    assert_eq!(badge.tone, Tone::Wait);
}

#[test]
fn the_dns_tile_has_no_badge_when_the_answer_time_is_in_the_middle() {
    let tile = dns_tile(Some(100), &TrendBuffer::new(), Some(100)).expect("the tile has data");
    assert!(tile.badge.is_none());
}

#[test]
fn the_trend_keeps_five_minutes_of_samples() {
    let mut trend = TrendBuffer::new();
    for value in 0..TREND_SAMPLES as i64 {
        trend.push(value * 1_000, value);
    }
    assert_eq!(
        trend.len(),
        (TREND_WINDOW_MS / 1_000) as usize,
        "only the last five minutes stay in the ring"
    );
    let points = trend.points();
    assert_eq!(points.len(), MAX_TREND_POINTS);
    assert_eq!(points[0], 602, "the first bucket holds the oldest minute");
}

#[test]
fn the_trend_forgets_the_samples_older_than_five_minutes() {
    let mut trend = TrendBuffer::new();
    trend.push(0, 10);
    trend.push(TREND_WINDOW_MS, 20);
    assert_eq!(
        trend.len(),
        1,
        "the sample of five minutes ago left the ring"
    );
    let stats = trend.stats().expect("the trend has values");
    assert_eq!(stats.min, 20);
}

#[test]
fn the_trend_never_grows_over_its_bound() {
    let mut trend = TrendBuffer::new();
    for value in 0..=TREND_SAMPLES as i64 {
        trend.push(0, value);
    }
    assert_eq!(trend.len(), TREND_SAMPLES);
}

#[test]
fn the_trend_can_be_cleared() {
    let mut trend = TrendBuffer::new();
    trend.push(0, 10);
    trend.clear();
    assert!(trend.is_empty());
    assert!(trend.points().is_empty());
    assert!(trend.stats().is_none());
}

#[test]
fn the_trend_gives_the_smallest_the_middle_and_the_largest_value() {
    let mut trend = TrendBuffer::new();
    for (index, value) in [30i64, 10, 20].into_iter().enumerate() {
        trend.push(index as i64 * 1_000, value);
    }
    let stats = trend.stats().expect("the trend has values");
    assert_eq!(stats.min, 10);
    assert_eq!(stats.median, 20);
    assert_eq!(stats.max, 30);
}

#[test]
fn the_signal_tile_leads_with_the_signal_strength() {
    let tile = signal_tile(&stats(), &TrendBuffer::new()).expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "-97");
    assert_eq!(big.unit, Some("dBm".to_string()));
}

#[test]
fn the_signal_tile_falls_back_to_the_signal_level() {
    let mut phone = stats();
    phone.dbm = None;
    phone.signal = Some(3);
    let tile = signal_tile(&phone, &TrendBuffer::new()).expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "3/4");
    assert_eq!(big.unit, None);
}

#[test]
fn the_signal_tile_shows_the_network_and_the_level() {
    let tile = signal_tile(&stats(), &TrendBuffer::new()).expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the network");
    assert_eq!(
        facts.lines,
        vec!["5G (advanced)".to_string(), "Signal 4/4".to_string()]
    );
}

#[test]
fn the_signal_tile_draws_four_level_bars() {
    let tile = signal_tile(&stats(), &TrendBuffer::new()).expect("the tile has data");
    let trend = tile.trend.expect("the tile draws the level");
    let levels = trend.levels.expect("the tile draws the bars");
    assert_eq!(levels.level, 4);
    assert_eq!(levels.max, 4);
}

#[test]
fn the_phone_tile_leads_with_the_temperature() {
    let tile = phone_tile(Some(315), None, None, Some(74), &TrendBuffer::new(), None)
        .expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "31.5");
    assert_eq!(big.unit, Some("C".to_string()));
    assert_eq!(big.tone, Tone::Muted);
}

#[test]
fn the_phone_tile_uses_the_wait_tone_when_the_phone_is_warm() {
    let tile =
        phone_tile(Some(430), None, None, None, &TrendBuffer::new(), None).expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.tone, Tone::Wait);
}

#[test]
fn the_phone_tile_draws_the_heat_threshold_as_the_reference() {
    let tile =
        phone_tile(Some(430), None, None, None, &TrendBuffer::new(), None).expect("the tile has data");
    let trend = tile.trend.expect("the tile draws the temperature");
    assert_eq!(trend.reference, Some(420));
}

#[test]
fn the_phone_tile_shows_the_battery() {
    let tile = phone_tile(Some(315), None, None, Some(74), &TrendBuffer::new(), None)
        .expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the battery");
    assert!(
        facts.lines.contains(&"Battery 74%".to_string()),
        "{:?}",
        facts.lines
    );
}

#[test]
fn the_phone_tile_shows_what_the_phone_says_about_the_heat() {
    let tile =
        phone_tile(Some(430), Some(3), None, None, &TrendBuffer::new(), None).expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the thermal line");
    assert_eq!(
        facts.lines,
        vec!["Android says the phone is limiting itself".to_string()]
    );
}

#[test]
fn the_today_tile_leads_with_the_shared_data() {
    let tile = today_tile(&totals(), TileScope::Today, Measure::Laptop).expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "903.0 MB");
}

#[test]
fn the_today_tile_draws_the_down_and_up_bar() {
    let tile = today_tile(&totals(), TileScope::Today, Measure::Laptop).expect("the tile has data");
    let trend = tile.trend.expect("the tile draws the bar");
    match trend.kind {
        TrendKind::Split { down, up } => {
            assert_eq!(down, 695_000_000);
            assert_eq!(up, 208_000_000);
        }
        other => panic!("the today tile draws the two-part bar, got {other:?}"),
    }
}

#[test]
fn the_today_tile_shows_the_data_and_the_peak() {
    let tile = today_tile(&totals(), TileScope::Today, Measure::Laptop).expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the totals");
    assert_eq!(
        facts.lines,
        vec![
            "Down 695.0 MB, up 208.0 MB".to_string(),
            "Peak 49.9 MB/s down".to_string(),
        ]
    );
}

#[test]
fn the_today_tile_names_its_scope() {
    let tile =
        today_tile(&totals(), TileScope::Session, Measure::Laptop).expect("the tile has data");
    assert_eq!(tile.label, "SESSION");
    let tile = today_tile(&totals(), TileScope::Today, Measure::Laptop).expect("the tile has data");
    assert_eq!(tile.label, "TODAY");
}

#[test]
fn the_wifi_tile_leads_with_the_leg_to_the_phone() {
    let tile = wifi_tile(&WifiLeg::new(12, 0, 60), &TrendBuffer::new(), None).expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "12");
    assert_eq!(big.unit, Some("ms".to_string()));
    let facts = tile.facts.expect("the tile shows the loss");
    assert_eq!(facts.lines, vec!["Loss 0% of 60".to_string()]);
}

#[test]
fn the_drops_tile_leads_with_the_drops_of_the_last_day() {
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the tile has data");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "1");
    assert_eq!(tile.label, "DROPS 24 H");
}

#[test]
fn the_drops_tile_names_its_scope() {
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records.clone()),
        BASE + 20 * HOUR,
        0,
        TileScope::Today,
        None,
    )
    .expect("the tile has data");
    assert_eq!(tile.label, "DROPS TODAY");
    let session = ScopeSession {
        started_at_ms: BASE,
        up_bytes: 0,
        down_bytes: 0,
        peak_down_bps: 0,
    };
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the tile has data");
    assert_eq!(tile.label, "DROPS SESSION");
}

#[test]
fn the_drops_tile_counts_only_the_drops_of_its_scope() {
    // One drop in the session, one before it.
    let session = ScopeSession {
        started_at_ms: BASE + 10 * HOUR,
        up_bytes: 0,
        down_bytes: 0,
        peak_down_bps: 0,
    };
    let records = vec![
        drop(BASE + 11 * HOUR, Some(113_000)),
        drop(BASE + 2 * HOUR, Some(113_000)),
    ];
    let tile = drops_tile(
        &events(records.clone()),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the tile has data");
    assert_eq!(tile.big.expect("the number").value, "1");
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the tile has data");
    assert_eq!(tile.big.expect("the number").value, "2");
}

#[test]
fn an_absence_is_muted_in_the_strip() {
    let records = vec![
        Event::Drop(drop(BASE + 11 * HOUR, Some(113_000))),
        Event::Away {
            at_ms: BASE + 15 * HOUR,
            down_ms: Some(4_440_000),
        },
    ];
    let tile = drops_tile(&records, BASE + 20 * HOUR, 0, TileScope::Last24Hours, None)
        .expect("the tile has data");
    let cells = match tile.trend.expect("the tile draws the hour strip").kind {
        TrendKind::Hours { cells } => cells,
        other => panic!("the drops tile draws the hour strip, got {other:?}"),
    };
    let muted: Vec<u8> = cells
        .iter()
        .filter(|cell| cell.mark == CellMark::Away)
        .map(|cell| cell.hour)
        .collect();
    assert_eq!(muted, vec![15]);
    let red: Vec<u8> = cells
        .iter()
        .filter(|cell| cell.mark == CellMark::Drop)
        .map(|cell| cell.hour)
        .collect();
    assert_eq!(red, vec![11]);
    // The absence is not a drop: the count stays at one.
    assert_eq!(tile.big.expect("the number").value, "1");
}

#[test]
fn the_drops_tile_shows_the_absence_and_says_it_is_not_a_drop() {
    let records = vec![Event::Away {
        at_ms: BASE + 15 * HOUR,
        down_ms: Some(4_440_000),
    }];
    let tile = drops_tile(&records, BASE + 20 * HOUR, 0, TileScope::Last24Hours, None)
        .expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the last event");
    assert_eq!(facts.lines[0], "15:00, Phone away 1:14:00");
    assert_eq!(facts.lines[1], "not a drop");
    assert_eq!(tile.big.expect("the number").tone, Tone::Muted);
}

#[test]
fn the_drops_tile_shows_the_stop_on_the_phone() {
    let records = vec![Event::Stopped {
        at_ms: BASE + 15 * HOUR,
        down_ms: Some(12_000),
    }];
    let tile = drops_tile(&records, BASE + 20 * HOUR, 0, TileScope::Last24Hours, None)
        .expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the last event");
    assert_eq!(facts.lines[0], "15:00, Stopped on the phone");
    assert_eq!(facts.lines[1], "not a drop");
}

#[test]
fn the_drops_tile_mutes_the_hours_outside_its_scope() {
    // The strip of the tile is the last day, whatever the scope of the count
    // is: the hours outside the scope are outside, so the strip never shows a
    // red hour the number does not count.
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 30 * HOUR,
        0,
        TileScope::Today,
        None,
    )
    .expect("the tile has data");
    assert_eq!(
        tile.big.as_ref().expect("the number").value,
        "0",
        "no drop today"
    );
    let cells = cells_of(&tile);
    let red: Vec<u8> = cells
        .iter()
        .filter(|(_, mark, _)| *mark == CellMark::Drop)
        .map(|(hour, _, _)| *hour)
        .collect();
    assert!(
        red.is_empty(),
        "the strip shows no red hour for a drop the tile does not count: {cells:?}"
    );
    assert!(
        cells.iter().any(|(hour, _, inside)| *hour == 11 && !inside),
        "the hour of yesterday's drop is outside the window of today: {cells:?}"
    );
}

#[test]
fn the_drops_tile_marks_the_hours_with_drops() {
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the tile has data");
    let trend = tile.trend.expect("the tile draws the hour strip");
    match trend.kind {
        TrendKind::Hours { cells } => {
            assert_eq!(cells.len(), 24);
            let marked: Vec<u8> = cells
                .iter()
                .filter(|cell| cell.mark == CellMark::Drop)
                .map(|cell| cell.hour)
                .collect();
            assert_eq!(marked, vec![11]);
        }
        other => panic!("the drops tile draws the hour strip, got {other:?}"),
    }
}

#[test]
fn the_drops_tile_marks_the_local_hours() {
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    // UTC+3: the drop at 11:00 UTC happened at 14:00 local time.
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        180,
        TileScope::Last24Hours,
        None,
    )
    .expect("the tile has data");
    let trend = tile.trend.expect("the tile draws the hour strip");
    match trend.kind {
        TrendKind::Hours { cells } => {
            let marked: Vec<u8> = cells
                .iter()
                .filter(|cell| cell.mark == CellMark::Drop)
                .map(|cell| cell.hour)
                .collect();
            assert_eq!(marked, vec![14]);
        }
        other => panic!("the drops tile draws the hour strip, got {other:?}"),
    }
}

#[test]
fn the_drops_count_and_the_strip_cover_the_same_window() {
    // The window ends at the current hour: the last 24 hours, hour by hour.
    let now = BASE + 12 * HOUR;
    let records = vec![
        drop(now - HOUR, Some(113_000)),      // 11:00, inside
        drop(now - 20 * HOUR, Some(113_000)), // 16:00 of yesterday, inside the 24 h
        drop(now - 25 * HOUR, Some(113_000)), // 11:00 of yesterday, outside
    ];
    let tile =
        drops_tile(&events(records), now, 0, TileScope::Last24Hours, None).expect("the drops tile");
    let big = tile.big.expect("the tile leads with a number");
    assert_eq!(big.value, "2");
    assert_eq!(tile.label, "DROPS 24 H");
    let cells = match tile.trend.expect("the tile draws the strip").kind {
        TrendKind::Hours { cells } => cells,
        other => panic!("the drops tile has an hour strip, got {other:?}"),
    };
    let marked = cells
        .iter()
        .filter(|cell| cell.mark == CellMark::Drop)
        .count();
    assert_eq!(marked, 2);
}

#[test]
fn the_drops_tile_shows_the_last_drop_and_the_reason() {
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the tile has data");
    let facts = tile.facts.expect("the tile shows the last drop");
    assert_eq!(facts.lines[0], "11:00, back after 1:53");
    assert_eq!(facts.lines[1], "no clear cause");
}

#[test]
fn the_status_card_says_where_the_laptop_is_connected() {
    let card = status_card(&StatusInput {
        connected: true,
        name: Some("Icy Parrot"),
        path: Some("Direct over hotspot"),
        uptime_secs: Some(4324),
        notice: None,
    });
    assert_eq!(card.title, "Connected to Icy Parrot");
    assert_eq!(
        card.subline,
        Some("Direct over hotspot - up 1:12:04".to_string())
    );
    assert!(card.stop, "the card offers the Stop button");
}

#[test]
fn the_status_card_without_a_device_says_it_is_not_connected() {
    let card = status_card(&StatusInput {
        connected: false,
        name: None,
        path: None,
        uptime_secs: None,
        notice: None,
    });
    assert_eq!(card.title, "Not connected");
    assert_eq!(card.subline, None);
    assert!(!card.stop, "there is nothing to stop");
}

#[test]
fn the_status_card_shows_no_running_time_while_not_connected() {
    let card = status_card(&StatusInput {
        connected: false,
        name: None,
        path: None,
        uptime_secs: Some(0),
        notice: None,
    });
    assert_eq!(card.title, "Not connected");
    assert_eq!(card.subline, None);
}

#[test]
fn the_status_card_carries_the_good_time_notice() {
    let card = status_card(&StatusInput {
        connected: true,
        name: Some("Icy Parrot"),
        path: None,
        uptime_secs: None,
        notice: Some("Good time for big downloads"),
    });
    assert_eq!(card.notice, Some("Good time for big downloads".to_string()));
}

#[test]
fn the_hours_strip_marks_the_quick_and_the_busy_hours() {
    let summary = Summary {
        hours: vec![
            LocalHour {
                hour: 2,
                dns_ms: 20,
                days: 5,
            },
            LocalHour {
                hour: 14,
                dns_ms: 400,
                days: 5,
            },
        ],
        days: 5,
        baseline_ms: 100,
    };
    let strip = hours_strip(&summary);
    assert_eq!(strip.cells.len(), 24);
    assert_eq!(strip.cells[2].mark, CellMark::Quick);
    assert_eq!(strip.cells[14].mark, CellMark::Busy);
    assert_eq!(strip.cells[3].mark, CellMark::Unknown);
}

#[test]
fn the_hours_strip_carries_the_learning_line_when_there_is_no_verdict() {
    let summary = Summary {
        hours: Vec::new(),
        days: 1,
        baseline_ms: 0,
    };
    let strip = hours_strip(&summary);
    assert_eq!(strip.learning, summary.learning_line());
}

#[test]
fn the_hours_strip_carries_the_learning_line_as_its_caption() {
    // The caption of the strip says why the strip looks the way it does. The
    // learning line is that reason while the hours are learning.
    let summary = Summary {
        hours: Vec::new(),
        days: 1,
        baseline_ms: 0,
    };
    let strip = hours_strip(&summary);
    assert_eq!(
        strip.caption.as_deref(),
        Some("Learning the busy hours: 0 of 3 days so far")
    );
}

#[test]
fn a_strip_with_no_verdict_says_why_in_one_line() {
    // Every hour has its three days, but no hour is quick and none is busy:
    // the strip is all grey and says so.
    let summary = Summary {
        hours: vec![
            LocalHour {
                hour: 2,
                dns_ms: 100,
                days: 3,
            },
            LocalHour {
                hour: 14,
                dns_ms: 100,
                days: 3,
            },
        ],
        days: 3,
        baseline_ms: 100,
    };
    let strip = hours_strip(&summary);
    assert!(
        strip
            .cells
            .iter()
            .all(|cell| cell.mark == CellMark::Unknown),
        "no hour has a verdict"
    );
    assert_eq!(strip.caption.as_deref(), Some("No quick or busy hours yet"));
}

#[test]
fn a_strip_with_a_verdict_has_no_caption() {
    // The strip speaks for itself when it shows a verdict: no caption.
    let summary = Summary {
        hours: vec![LocalHour {
            hour: 2,
            dns_ms: 20,
            days: 5,
        }],
        days: 5,
        baseline_ms: 100,
    };
    let strip = hours_strip(&summary);
    assert_eq!(strip.cells[2].mark, CellMark::Quick);
    assert_eq!(strip.caption, None);
}

#[test]
fn the_hours_strip_is_titled_usual_hours() {
    let summary = Summary {
        hours: Vec::new(),
        days: 1,
        baseline_ms: 0,
    };
    let strip = hours_strip(&summary);
    assert_eq!(strip.title, "USUAL HOURS");
}

#[test]
fn a_tile_without_a_slot_is_empty() {
    let tile = dns_tile(Some(34), &TrendBuffer::new(), None).expect("the tile has data");
    assert!(!tile.is_empty());
    let empty = jafnify_core::tiles::Tile {
        id: "x".to_string(),
        label: "X".to_string(),
        badge: None,
        big: None,
        trend: None,
        facts: None,
        note: None,
    };
    assert!(empty.is_empty());
}

#[test]
fn the_phone_tile_shows_the_evidence_of_the_hot_hours() {
    let evidence = "When the phone was 42 C or more, DNS was usually 1.8x slower (3 hot hours)";
    let tile = phone_tile(
        Some(315),
        None,
        None,
        Some(74),
        &TrendBuffer::new(),
        Some(evidence),
    )
    .expect("the tile has data");
    assert_eq!(tile.note, Some(evidence.to_string()));
}

#[test]
fn a_tile_without_evidence_has_no_note() {
    let tile = phone_tile(Some(315), None, None, Some(74), &TrendBuffer::new(), None)
        .expect("the tile has data");
    assert_eq!(tile.note, None);
}

#[test]
fn the_trends_take_the_measurements_of_a_summary() {
    let mut trends = Trends::new();
    trends.push(0, &stats(), Some(12));
    assert_eq!(
        trends
            .dns()
            .stats()
            .expect("the dns trend has a sample")
            .median,
        34
    );
    assert_eq!(
        trends
            .signal()
            .stats()
            .expect("the signal trend has a sample")
            .median,
        -97
    );
    assert_eq!(
        trends
            .phone()
            .stats()
            .expect("the phone trend has a sample")
            .median,
        315
    );
    assert_eq!(
        trends
            .wifi()
            .stats()
            .expect("the wifi trend has a sample")
            .median,
        12
    );
}

#[test]
fn the_trends_skip_what_the_phone_does_not_report() {
    let mut trends = Trends::new();
    let mut phone = stats();
    phone.rtt_ms = None;
    phone.dbm = None;
    phone.temp_c_x10 = None;
    trends.push(0, &phone, None);
    assert!(trends.dns().is_empty());
    assert!(trends.signal().is_empty());
    assert!(trends.phone().is_empty());
    assert!(trends.wifi().is_empty());
    // The down rate is always on the summary line: a zero means nothing came
    // down, which is a measurement, not a missing value.
    assert_eq!(trends.speed().len(), 1);
}

#[test]
fn the_trends_forget_everything_when_the_link_goes_down() {
    let mut trends = Trends::new();
    trends.push(0, &stats(), Some(12));
    trends.clear();
    assert!(trends.dns().is_empty());
    assert!(trends.signal().is_empty());
    assert!(trends.phone().is_empty());
    assert!(trends.wifi().is_empty());
    assert!(trends.speed().is_empty());
}

#[test]
fn the_dns_tile_has_no_badge_when_the_overall_median_is_unknown() {
    let tile = dns_tile(Some(34), &TrendBuffer::new(), Some(0)).expect("the tile has data");
    assert!(
        tile.badge.is_none(),
        "a zero median says nothing about the hour"
    );
}

#[test]
fn the_facts_keep_at_most_two_lines() {
    let facts = Facts::new(vec![
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
    ]);
    assert_eq!(facts.lines.len(), 2);
}

#[test]
fn a_tile_without_a_filled_slot_is_not_shown() {
    let tile = Tile {
        id: "dns".to_string(),
        label: "DNS ANSWER".to_string(),
        badge: None,
        big: None,
        trend: None,
        facts: None,
        note: Some("only a note".to_string()),
    };
    assert!(tile.is_empty());
}

#[test]
fn the_verdict_of_the_hours_strip_is_the_rule_of_the_good_hours() {
    assert_eq!(
        Verdict::Quick,
        jafnify_core::good_hours::verdict_for(80, 100)
    );
    assert_eq!(
        Verdict::Busy,
        jafnify_core::good_hours::verdict_for(125, 100)
    );
}

/// The summary line of the phone: the network, the signal, the DNS time, the
/// battery, the temperature and what Android says about the heat.
fn phone_stats() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        rtt_ms: Some(34),
        dbm: Some(-71),
        battery: Some(74),
        temp_c_x10: Some(300),
        thr: Some(0),
        ..Default::default()
    }
}

fn phone_home_input<'a>(
    stats: Option<&'a PhoneStats>,
    trends: &'a Trends,
    today: Option<&'a UsageSummary>,
) -> jafnify_core::tiles::PhoneHomeInput<'a> {
    jafnify_core::tiles::PhoneHomeInput {
        state: PhoneShareState::Sharing,
        state_line: None,
        notice: None,
        uptime_secs: Some(72),
        today,
        stats,
        trends,
        charging: Some(true),
        battery_saver: Some(false),
        show_dns: true,
        show_battery: true,
        today_scope: TileScope::Today,
        speed_scope: TileScope::Today,
        average_scope: TileScope::Today,
        now_ms: BASE,
        session: None,
        range: None,
        network_permission: true,
    }
}

#[test]
fn the_phone_home_has_a_card_and_the_seven_tiles() {
    let mut stats = phone_stats();
    stats.down_bps = 49_900_000;
    stats.up_bps = 1_200_000;
    let mut trends = Trends::new();
    trends.push(BASE, &stats, None);
    let today = usage();
    let model =
        jafnify_core::tiles::phone_home(&phone_home_input(Some(&stats), &trends, Some(&today)));
    assert_eq!(model.status.title, "Sharing with the laptop");
    // The bytes and the peak of today live in the TODAY tile now.
    assert_eq!(model.status.subline.as_deref(), Some("Up 0:01:12"));
    assert_eq!(model.status.action.as_deref(), Some("Stop sharing"));
    assert!(model.status.stop);
    let ids: Vec<&str> = model.tiles.iter().map(|tile| tile.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["signal", "dns", "heat", "battery", "speed", "today", "avg"]
    );
    let labels: Vec<&str> = model.tiles.iter().map(|tile| tile.label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["SIGNAL", "DNS", "HEAT", "BATTERY", "SPEED", "TODAY", "AVERAGE"]
    );
}

#[test]
fn the_phone_card_says_the_truth_about_the_sharing() {
    let trends = Trends::new();
    // The tunnel is up.
    let mut input = phone_home_input(None, &trends, None);
    input.state = PhoneShareState::Sharing;
    let card = jafnify_core::tiles::phone_status_card(&input);
    assert_eq!(card.title, "Sharing with the laptop");
    assert_eq!(card.tone, Tone::Ok);
    assert!(card.connected);
    assert!(card.stop);
    assert_eq!(card.action.as_deref(), Some("Stop sharing"));

    // The phone is trying but the tunnel is not up: the card must not say
    // that it is sharing.
    let mut input = phone_home_input(None, &trends, None);
    input.state = PhoneShareState::Trying;
    input.state_line = Some("Retrying in 12 s");
    let card = jafnify_core::tiles::phone_status_card(&input);
    assert_eq!(card.title, "Trying to share");
    assert_eq!(card.tone, Tone::Wait);
    assert!(!card.connected);
    assert!(card.stop);
    assert_eq!(card.action.as_deref(), Some("Stop sharing"));
    assert_eq!(card.subline.as_deref(), Some("Retrying in 12 s"));

    // The phone does not share at all.
    let mut input = phone_home_input(None, &trends, None);
    input.state = PhoneShareState::Idle;
    let card = jafnify_core::tiles::phone_status_card(&input);
    assert_eq!(card.title, "Not sharing");
    assert_eq!(card.tone, Tone::Muted);
    assert!(!card.connected);
    assert!(!card.stop);
    assert_eq!(card.action.as_deref(), Some("Share"));
}

#[test]
fn the_phone_home_leaves_out_a_tile_without_data() {
    let trends = Trends::new();
    let mut input = phone_home_input(None, &trends, None);
    input.state = PhoneShareState::Idle;
    input.show_battery = false;
    let model = jafnify_core::tiles::phone_home(&input);
    assert!(model.tiles.is_empty());
    assert_eq!(model.status.title, "Not sharing");
    assert_eq!(model.status.action.as_deref(), Some("Share"));
    assert!(!model.status.stop);
}

#[test]
fn the_phone_home_leaves_out_the_dns_tile_without_the_dns_switch() {
    let stats = phone_stats();
    let trends = Trends::new();
    let mut input = phone_home_input(Some(&stats), &trends, None);
    input.show_dns = false;
    let model = jafnify_core::tiles::phone_home(&input);
    let ids: Vec<&str> = model.tiles.iter().map(|tile| tile.id.as_str()).collect();
    assert_eq!(ids, vec!["signal", "heat", "battery", "speed"]);
}

#[test]
fn the_phone_heat_tile_says_what_android_says() {
    let trends = Trends::new();
    let tile =
        jafnify_core::tiles::heat_tile(Some(300), Some(0), None, trends.phone())
            .expect("the heat tile");
    assert_eq!(tile.label, "HEAT");
    assert_eq!(
        tile.facts.expect("the tile says what Android says").lines,
        vec!["Android: no throttling".to_string()]
    );
    let big = tile.big.expect("the tile leads with the temperature");
    assert_eq!(big.value, "30");
    assert_eq!(big.unit, Some("C".to_string()));
    assert_eq!(big.tone, Tone::Muted);
    let trend = tile
        .trend
        .expect("the tile has the line of the temperature");
    assert_eq!(trend.reference, Some(420));
}

#[test]
fn the_battery_tile_shows_the_fill_and_the_facts() {
    let tile = jafnify_core::tiles::battery_tile(Some(74), Some(true), Some(true))
        .expect("the battery tile");
    assert_eq!(tile.label, "BATTERY");
    let big = tile.big.expect("the tile leads with the percent");
    assert_eq!(big.value, "74");
    assert_eq!(big.unit, Some("%".to_string()));
    assert_eq!(
        tile.facts.expect("the tile shows the facts").lines,
        vec!["Charging".to_string(), "Battery saver on".to_string()]
    );
    let trend = tile.trend.expect("the tile has the fill");
    assert!(matches!(
        trend.kind,
        TrendKind::Fill {
            value: 74,
            max: 100
        }
    ));
}

#[test]
fn the_home_text_is_the_contract_the_phone_reads() {
    // The phone parses this text. When a field moves, this test fails and the
    // parser in the app has to move with it.
    let stats = phone_stats();
    let trends = Trends::new();
    let today = usage();
    let model =
        jafnify_core::tiles::phone_home(&phone_home_input(Some(&stats), &trends, Some(&today)));
    let text = jafnify_core::tiles::phone_home_text(&model);
    let expected = concat!(
        "JFH1 1\n",
        "s | connected=1 | tone=ok | title=Sharing with the laptop",
        " | sub=Up 0:01:12",
        " | stop=1 | action=Stop sharing | notice=\n",
        "t | id=signal | label=SIGNAL | badge= | btone=muted | value=-71 | unit=dBm",
        " | tone=muted | kind=line | points= | ref= | level=4 | level_max=4",
        " | fact=5G (advanced) | fact=Signal 4/4 | note=\n",
        "t | id=dns | label=DNS | badge= | btone=muted | value=34 | unit=ms",
        " | tone=muted | kind=none | points= | ref= | level= | level_max= | note=\n",
        "t | id=heat | label=HEAT | badge= | btone=muted | value=30 | unit=C",
        " | tone=muted | kind=line | points= | ref=420 | level= | level_max=",
        " | fact=Android: no throttling | note=\n",
        "t | id=battery | label=BATTERY | badge= | btone=muted | value=74 | unit=%",
        " | tone=muted | kind=fill | fill=74 | fill_max=100 | ref= | level= | level_max=",
        " | fact=Charging | fact=Battery saver off | note=\n",
        "t | id=speed | label=SPEED | badge= | btone=muted | value=0 | unit=B/s",
        " | tone=muted | kind=none | points= | ref= | level= | level_max=",
        " | fact=Up 0 B/s | note=Peak 49.9 MB/s today\n",
        "t | id=today | label=TODAY | badge= | btone=muted | value=903.0 MB | unit=",
        " | tone=muted | kind=split | down=695000000 | up=208000000",
        " | ref= | level= | level_max=",
        " | fact=Down 695.0 MB, up 208.0 MB | fact=Peak 49.9 MB/s down | note=Counted with encryption\n",
        "t | id=avg | label=AVERAGE | badge= | btone=muted | value=193.1 | unit=KB/s",
        " | tone=muted | kind=split | down=695000000 | up=208000000",
        " | ref= | level= | level_max=",
        " | fact=Peak 49.9 MB/s down, 120.0 KB/s up today | fact=Moved 903.0 MB today | note=Avg 57.8 KB/s up today",
    );
    assert_eq!(text, expected);
}

#[test]
fn the_words_of_the_home_never_carry_a_bar() {
    // The text of the home is split on " | ": a word with a bar in it would
    // break the parse of the phone.
    let stats = phone_stats();
    let trends = Trends::new();
    let today = usage();
    let model =
        jafnify_core::tiles::phone_home(&phone_home_input(Some(&stats), &trends, Some(&today)));
    let mut words: Vec<&str> = vec![model.status.title.as_str()];
    if let Some(subline) = model.status.subline.as_deref() {
        words.push(subline);
    }
    if let Some(action) = model.status.action.as_deref() {
        words.push(action);
    }
    for tile in &model.tiles {
        words.push(tile.label.as_str());
        if let Some(badge) = &tile.badge {
            words.push(badge.text.as_str());
        }
        if let Some(big) = &tile.big {
            words.push(big.value.as_str());
        }
        if let Some(facts) = &tile.facts {
            for line in &facts.lines {
                words.push(line.as_str());
            }
        }
        if let Some(note) = tile.note.as_deref() {
            words.push(note);
        }
    }
    for word in words {
        assert!(!word.contains(" | "), "the word {word:?} carries a bar");
        assert!(
            !word.contains('\n'),
            "the word {word:?} carries a line break"
        );
    }
}

#[test]
fn the_phone_home_text_carries_the_version_and_the_parts() {
    let stats = phone_stats();
    let trends = Trends::new();
    let today = usage();
    let model =
        jafnify_core::tiles::phone_home(&phone_home_input(Some(&stats), &trends, Some(&today)));
    let text = jafnify_core::tiles::phone_home_text(&model);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("JFH1 1"));
    let status = lines.next().expect("the card line");
    assert!(status.starts_with("s | "));
    assert!(status.contains("title=Sharing with the laptop"));
    assert!(status.contains("action=Stop sharing"));
    let tiles: Vec<&str> = lines.collect();
    assert_eq!(tiles.len(), 7);
    assert!(tiles[0].starts_with("t | id=signal | label=SIGNAL"));
    assert!(tiles[0].contains("value=-71"));
    assert!(tiles[0].contains("unit=dBm"));
    assert!(tiles[0].contains("fact=5G (advanced)"));
    assert!(tiles[0].contains("fact=Signal 4/4"));
    assert!(tiles[2].contains("label=HEAT"));
    assert!(tiles[2].contains("fact=Android: no throttling"));
    assert!(tiles[3].contains("label=BATTERY"));
    assert!(tiles[3].contains("fill=74"));
    assert!(tiles[3].contains("fill_max=100"));
}

#[test]
fn the_speed_tile_shows_the_rate_and_the_peak_of_the_ring() {
    let mut stats = phone_stats();
    stats.down_bps = 49_900_000;
    stats.up_bps = 1_200_000;
    let mut trends = Trends::new();
    trends.push(BASE, &stats, None);
    let mut faster = stats.clone();
    faster.down_bps = 61_300_000;
    trends.push(BASE + 1_000, &faster, None);
    let tile =
        jafnify_core::tiles::speed_tile(&stats, trends.speed(), TileScope::Today, 61_300_000)
            .expect("the speed tile");
    assert_eq!(tile.id, "speed");
    assert_eq!(tile.label, "SPEED");
    let big = tile.big.expect("the number");
    assert_eq!(big.value, "49.9");
    assert_eq!(big.unit.as_deref(), Some("MB/s"));
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts, vec!["49.9-61.3 MB/s in 5 min", "Up 1.2 MB/s"]);
    assert_eq!(
        tile.note.as_deref(),
        Some("Peak 61.3 MB/s today"),
        "the peak of the scope is a note under the facts"
    );
    assert!(
        tile.trend.is_some(),
        "the speed carries the line of the ring"
    );
}

#[test]
fn the_speed_tile_names_the_scope_of_its_peak() {
    let mut stats = phone_stats();
    stats.down_bps = 49_900_000;
    let trends = Trends::new();
    let tile =
        jafnify_core::tiles::speed_tile(&stats, trends.speed(), TileScope::Session, 61_300_000)
            .expect("the speed tile");
    let facts = tile.facts.expect("the facts").lines;
    assert!(facts.len() <= 2, "the facts stay two lines: {facts:?}");
    assert_eq!(facts.last().map(String::as_str), Some("Up 0 B/s"));
    assert_eq!(
        tile.note.as_deref(),
        Some("Peak 61.3 MB/s in session"),
        "the note names the scope of the peak"
    );
}

#[test]
fn the_speed_tile_has_no_peak_without_samples() {
    let mut stats = phone_stats();
    stats.down_bps = 512;
    let trends = Trends::new();
    let tile = jafnify_core::tiles::speed_tile(&stats, trends.speed(), TileScope::Today, 0)
        .expect("the speed tile");
    assert_eq!(tile.big.expect("the number").value, "512");
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts, vec!["Up 0 B/s"]);
}

#[test]
fn the_speed_of_a_stopped_sharing_is_forgotten() {
    let mut stats = phone_stats();
    stats.down_bps = 49_900_000;
    let mut trends = Trends::new();
    trends.push(BASE, &stats, None);
    assert_eq!(trends.speed().len(), 1);
    trends.clear();
    assert!(trends.speed().is_empty());
}

#[test]
fn the_rate_is_split_into_a_number_and_a_unit() {
    assert_eq!(
        jafnify_core::usage::format_rate_parts(49_900_000),
        ("49.9".to_string(), "MB/s".to_string())
    );
    assert_eq!(
        jafnify_core::usage::format_rate_parts(512),
        ("512".to_string(), "B/s".to_string())
    );
}

#[test]
fn the_speed_tile_is_not_shown_without_a_tunnel() {
    let stats = phone_stats();
    let trends = Trends::new();
    let mut input = phone_home_input(Some(&stats), &trends, None);
    input.state = PhoneShareState::Trying;
    let model = jafnify_core::tiles::phone_home(&input);
    let ids: Vec<&str> = model.tiles.iter().map(|tile| tile.id.as_str()).collect();
    assert!(!ids.contains(&"speed"), "no rate without a tunnel: {ids:?}");
    assert_eq!(model.status.subline, None);
}

#[test]
fn the_today_tile_shows_the_plan_when_the_phone_knows_one() {
    let today = usage();
    let tile = jafnify_core::tiles::phone_today_tile(
        &totals_of(&today),
        1_200_000_000,
        5_000_000_000,
        TileScope::Today,
    )
    .expect("the today tile");
    assert_eq!(tile.label, "TODAY");
    assert_eq!(tile.big.expect("the number").value, "903.0 MB");
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts[0], "Down 695.0 MB, up 208.0 MB");
    assert_eq!(facts[1], "Plan: 1.2 GB of 5.0 GB this month");
}

#[test]
fn the_today_tile_falls_back_to_the_peak_without_a_plan() {
    let today = usage();
    let tile = jafnify_core::tiles::phone_today_tile(&totals_of(&today), 0, 0, TileScope::Today)
        .expect("the today tile");
    let facts = tile.facts.expect("the facts").lines;
    assert_eq!(facts[1], "Peak 49.9 MB/s down");
}

#[test]
fn the_today_tile_is_hidden_when_nothing_was_shared() {
    let mut today = usage();
    today.down_bytes = 0;
    today.up_bytes = 0;
    assert!(
        jafnify_core::tiles::phone_today_tile(&totals_of(&today), 0, 0, TileScope::Today).is_none()
    );
}

#[test]
fn the_today_tile_of_the_session_shows_the_numbers_of_the_sharing() {
    let session = ScopeSession {
        started_at_ms: BASE,
        up_bytes: 1_000_000,
        down_bytes: 2_000_000,
        peak_down_bps: 3_000_000,
    };
    let tile =
        jafnify_core::tiles::phone_today_tile(&totals_of(&usage()), 0, 0, TileScope::Session);
    // The scope is the label; the numbers come from the caller.
    assert!(tile.is_some());
    let scoped = jafnify_core::tiles::phone_today_tile(
        &ScopeTotals {
            up_bytes: session.up_bytes,
            down_bytes: session.down_bytes,
            peak_down_bps: session.peak_down_bps,
        },
        0,
        0,
        TileScope::Session,
    )
    .expect("the tile of the session");
    assert_eq!(scoped.label, "SESSION");
    assert_eq!(scoped.big.expect("the number").value, "3.0 MB");
}

#[test]
fn the_sharing_card_never_carries_the_state_line() {
    // A transition line under "Sharing with the laptop" would contradict the
    // title: while the tunnel is up the card says nothing but the uptime.
    let trends = Trends::new();
    let mut input = phone_home_input(None, &trends, None);
    input.uptime_secs = None;
    input.state_line = Some("Retrying in 12 s");
    let model = jafnify_core::tiles::phone_home(&input);
    assert_eq!(model.status.subline, None);
    assert_eq!(model.status.title, "Sharing with the laptop");
}

#[test]
fn the_text_carries_the_seven_tiles_and_the_split_of_today() {
    let mut stats = phone_stats();
    stats.down_bps = 49_900_000;
    stats.up_bps = 1_200_000;
    let mut trends = Trends::new();
    trends.push(BASE, &stats, None);
    let today = usage();
    let model =
        jafnify_core::tiles::phone_home(&phone_home_input(Some(&stats), &trends, Some(&today)));
    let text = jafnify_core::tiles::phone_home_text(&model);
    assert!(text.starts_with("JFH1 1\n"), "the version stays 1: {text}");
    assert_eq!(
        text.lines().filter(|line| line.starts_with("t | ")).count(),
        7
    );
    assert!(text.contains("label=SPEED"));
    // The line of the speed travels in the text too, not only in the tile.
    assert!(
        text.contains(
            "label=SPEED | badge= | btone=muted | value=49.9 | unit=MB/s | tone=muted | kind=line | points=49900000"
        ),
        "{text}"
    );
    assert!(text.contains("fact=Up 1.2 MB/s"));
    assert!(text.contains("note=Peak 49.9 MB/s today"));
    assert!(text.contains("kind=split | down=695000000 | up=208000000"));
    assert!(text.contains("sub=Up 0:01:12"));
}

// ---- the number, the strip and the fact line share one window ----

/// A sharing session that started at the given moment.
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
        .expect("the tile shows facts")
        .lines
        .clone()
}

#[test]
fn the_hours_outside_the_scope_are_outside() {
    // The session starts at 10:00; the drop at 11:00 is inside it, the hour
    // before it is not.
    let session = session_at(BASE + 10 * HOUR);
    let records = vec![drop(BASE + 11 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    let cells = cells_of(&tile);
    assert!(
        cells.contains(&(11, CellMark::Drop, true)),
        "the hour of the drop is inside the window: {cells:?}"
    );
    assert!(
        cells.iter().any(|(hour, _, inside)| *hour == 9 && !inside),
        "the hour before the session is outside: {cells:?}"
    );
}

#[test]
fn the_drops_tile_says_no_drops_when_the_scope_has_no_event() {
    // The only drop is before the session: the tile counts nothing.
    let session = session_at(BASE + 10 * HOUR);
    let records = vec![drop(BASE + 2 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    assert_eq!(tile.big.as_ref().expect("the number").value, "0");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops this session");
    assert_eq!(facts[1], "Last: 02:00, back after 1:53");
}

#[test]
fn the_drops_tile_says_no_drops_today_when_the_scope_is_today() {
    // The drop is on 2026-01-01, the tile is built on 2026-01-02.
    let records = vec![drop(BASE + 2 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 30 * HOUR,
        0,
        TileScope::Today,
        None,
    )
    .expect("the drops tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops today");
}

#[test]
fn the_drops_tile_says_no_drops_in_24_h_when_the_scope_is_the_last_day() {
    let records = vec![drop(BASE + 2 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 40 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the drops tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops in 24 h");
}

#[test]
fn the_drops_tile_names_the_newest_older_event() {
    // Two events before the scope: the newest one is the one that is named.
    let session = session_at(BASE + 10 * HOUR);
    let records = vec![
        drop(BASE + 2 * HOUR, Some(113_000)),
        drop(BASE + 4 * HOUR, Some(113_000)),
    ];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops this session");
    assert_eq!(facts[1], "Last: 04:00, back after 1:53");
}

#[test]
fn a_drop_outside_the_scope_is_not_counted() {
    // The drop is one millisecond before the session starts.
    let session = session_at(BASE + 10 * HOUR);
    let records = vec![drop(BASE + 10 * HOUR - 1, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    assert_eq!(tile.big.as_ref().expect("the number").value, "0");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops this session");
}

#[test]
fn a_drop_right_after_the_session_starts_is_counted() {
    let session = session_at(BASE + 10 * HOUR);
    let records = vec![drop(BASE + 10 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Session,
        Some(&session),
    )
    .expect("the drops tile");
    assert_eq!(tile.big.as_ref().expect("the number").value, "1");
    assert_eq!(facts_of(&tile)[0], "10:00, back after 1:53");
}

#[test]
fn the_fact_time_of_today_has_no_day() {
    let records = vec![drop(BASE + 17 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        0,
        TileScope::Today,
        None,
    )
    .expect("the drops tile");
    assert_eq!(facts_of(&tile)[0], "17:00, back after 1:53");
}

#[test]
fn the_fact_time_of_yesterday_says_yesterday() {
    // The event is on 2026-01-01, the tile is built on 2026-01-02.
    let records = vec![drop(BASE + 17 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 30 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the drops tile");
    assert_eq!(facts_of(&tile)[0], "yesterday 17:00, back after 1:53");
}

#[test]
fn an_older_fact_time_says_the_month_and_the_day() {
    // The event is on 2026-01-01, the tile is built on 2026-01-03.
    let records = vec![drop(BASE + 17 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 50 * HOUR,
        0,
        TileScope::Last24Hours,
        None,
    )
    .expect("the drops tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops in 24 h");
    assert_eq!(facts[1], "Last: Jan 1 17:00, back after 1:53");
}

#[test]
fn a_fact_time_at_midnight_belongs_to_the_day_before() {
    // The event is at 23:00 of 2026-01-01; the tile is built at 00:01 of the
    // next day, so the event is yesterday.
    let records = vec![drop(BASE + 23 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 24 * HOUR + 60_000,
        0,
        TileScope::Today,
        None,
    )
    .expect("the drops tile");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "No drops today");
    assert_eq!(facts[1], "Last: yesterday 23:00, back after 1:53");
}

#[test]
fn a_fact_time_uses_the_local_offset() {
    // UTC-5: the drop at 17:00 UTC happened at 12:00 local time.
    let records = vec![drop(BASE + 17 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 20 * HOUR,
        -300,
        TileScope::Today,
        None,
    )
    .expect("the drops tile");
    assert_eq!(facts_of(&tile)[0], "12:00, back after 1:53");
}

#[test]
fn a_fact_time_yesterday_with_a_local_offset() {
    // UTC-5: the drop at 20:00 UTC was at 15:00 of the day before.
    let records = vec![drop(BASE + 20 * HOUR, Some(113_000))];
    let tile = drops_tile(
        &events(records),
        BASE + 30 * HOUR,
        -300,
        TileScope::Last24Hours,
        None,
    )
    .expect("the drops tile");
    assert_eq!(facts_of(&tile)[0], "yesterday 15:00, back after 1:53");
}

#[test]
fn the_signal_tile_says_the_window_of_its_line() {
    let mut trend = TrendBuffer::new();
    trend.push(0, -97);
    trend.push(1_000, -80);
    let tile = signal_tile(&stats(), &trend).expect("the tile has data");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "-97 to -80 dBm in 5 min");
}

#[test]
fn the_wifi_leg_tile_says_the_window_of_its_line() {
    let mut trend = TrendBuffer::new();
    trend.push(0, 8);
    trend.push(1_000, 40);
    let tile = wifi_tile(&WifiLeg::new(12, 0, 60), &trend, None).expect("the tile has data");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "8-40 ms in 5 min");
}

#[test]
fn the_phone_tile_says_the_window_of_its_line() {
    let mut trend = TrendBuffer::new();
    trend.push(0, 300);
    trend.push(1_000, 330);
    let tile = phone_tile(Some(315), None, None, Some(74), &trend, None).expect("the tile has data");
    let facts = facts_of(&tile);
    assert_eq!(facts[0], "30.0-33.0 C in 5 min");
}

#[test]
fn the_peak_of_the_phone_never_reads_below_its_line() {
    // SPEED said "0 B/s to 64.0 KB/s in 5 min" over
    // "Peak 63.9 KB/s in session"; the line and the peak come from two
    // samplers. The peak of the scope is at least the top of its line, on the
    // speed tile and on the session tile alike.
    let mut stats = phone_stats();
    stats.down_bps = 64_000;
    let mut trends = Trends::new();
    trends.push(BASE, &stats, None);
    let session = ScopeSession {
        started_at_ms: BASE - 60_000,
        up_bytes: 244_600,
        down_bytes: 317_700,
        peak_down_bps: 63_900,
    };
    let today = usage();
    let mut input = phone_home_input(Some(&stats), &trends, Some(&today));
    input.session = Some(&session);
    input.speed_scope = TileScope::Session;
    input.today_scope = TileScope::Session;
    let model = jafnify_core::tiles::phone_home(&input);
    let speed = model.tiles.iter().find(|t| t.id == "speed").unwrap();
    assert_eq!(speed.note.as_deref(), Some("Peak 64.0 KB/s in session"));
    let today = model.tiles.iter().find(|t| t.id == "today").unwrap();
    assert_eq!(
        today.facts.as_ref().unwrap().lines[1],
        "Peak 64.0 KB/s down"
    );

    // A sample from before the session is not a peak of the session.
    let mut early = Trends::new();
    early.push(BASE - 120_000, &stats, None);
    let mut input = phone_home_input(Some(&stats), &early, None);
    input.session = Some(&session);
    input.speed_scope = TileScope::Session;
    let model = jafnify_core::tiles::phone_home(&input);
    let speed = model.tiles.iter().find(|t| t.id == "speed").unwrap();
    assert_eq!(speed.note.as_deref(), Some("Peak 63.9 KB/s in session"));
}

#[test]
fn the_phone_shows_how_far_the_laptop_is_while_it_reports() {
    use jafnify_core::wifi_range::RangeReport;
    let stats = phone_stats();
    let trends = Trends::new();
    let mut levels = jafnify_core::tiles::TrendBuffer::new();
    levels.push(BASE - 2_000, -70);
    levels.push(BASE, -74);
    let report = RangeReport {
        level_dbm: -74,
        rate_100k: Some(1300),
        missed_beacons: false,
    };
    let mut input = phone_home_input(Some(&stats), &trends, None);
    input.range = Some((report, &levels));
    let model = jafnify_core::tiles::phone_home(&input);
    let range = model.tiles.iter().find(|t| t.id == "range").expect("RANGE");
    assert_eq!(range.big.as_ref().unwrap().value, "-74");
    let facts = &range.facts.as_ref().unwrap().lines;
    assert_eq!(facts[0], "-74 to -70 dBm in 5 min");
    assert_eq!(facts[1], "Likely far from the phone");
    assert_eq!(range.note.as_deref(), Some("Link 130.0 Mbit/s"));
    let text = jafnify_core::tiles::phone_home_text(&model);
    assert!(text.contains("id=range | label=RANGE"), "{text}");

    // Not sharing: no tunnel, no tile.
    input.state = PhoneShareState::Idle;
    let model = jafnify_core::tiles::phone_home(&input);
    assert!(model.tiles.iter().all(|t| t.id != "range"));
}

#[test]
fn the_phone_signal_tile_explains_a_missing_network_permission() {
    let stats = PhoneStats {
        signal: None,
        dbm: None,
        ..phone_stats()
    };
    let trends = Trends::new();
    let today = usage();
    let mut input = phone_home_input(Some(&stats), &trends, Some(&today));
    input.network_permission = false;
    let model = jafnify_core::tiles::phone_home(&input);
    let signal = model
        .tiles
        .iter()
        .find(|tile| tile.id == "signal")
        .expect("the signal slot explains the permission");
    assert_eq!(signal.big.as_ref().unwrap().value, "--");
    assert_eq!(
        signal.big.as_ref().unwrap().tone,
        jafnify_core::tiles::Tone::Muted
    );
    assert_eq!(
        signal.facts.as_ref().unwrap().lines,
        vec![jafnify_core::tiles::SIGNAL_PERMISSION_PHONE.to_string()]
    );
}

#[test]
fn the_phone_signal_tile_keeps_no_data_when_the_permission_is_granted() {
    let stats = PhoneStats {
        signal: None,
        dbm: None,
        ..phone_stats()
    };
    let trends = Trends::new();
    let today = usage();
    let input = phone_home_input(Some(&stats), &trends, Some(&today));
    let model = jafnify_core::tiles::phone_home(&input);
    assert!(model.tiles.iter().all(|tile| tile.id != "signal"));
    let arranged = jafnify_core::home_layout::arrange(
        &jafnify_core::home_layout::Layout::default_for(
            jafnify_core::home_layout::Platform::Android,
        ),
        jafnify_core::home_layout::Platform::Android,
        model.tiles,
    );
    let signal = arranged
        .iter()
        .find(|tile| tile.id == "signal")
        .expect("the signal slot");
    assert_eq!(
        signal.facts.as_ref().unwrap().lines,
        vec!["No data yet".to_string()]
    );
}

#[test]
fn the_permission_facts_are_the_required_words() {
    assert_eq!(
        jafnify_core::tiles::SIGNAL_PERMISSION_PHONE,
        "Turn on Network details in Settings"
    );
    assert_eq!(
        jafnify_core::tiles::SIGNAL_PERMISSION_LAPTOP,
        "Turn on Network details on the phone"
    );
}
