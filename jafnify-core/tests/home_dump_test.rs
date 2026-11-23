//! The text of the home model of the laptop, for two cases: a session that
//! holds only the drops of the day before, and the same session with a drop
//! inside it.
//!
//! The dump is written under the test scratch directory. It is the words the
//! window draws, line by line, so it can be read without a screenshot.

use std::path::PathBuf;

use jafnify_core::drop_history::{DropRecord, Event, REASON_PEER_LEFT};
use jafnify_core::good_hours::{HourRecord, Summary};
use jafnify_core::phone_stats::{NetworkKind, PhoneStats};
use jafnify_core::scope::{ScopeSession, ScopeTotals, TileScope};
use jafnify_core::tiles::{self, HomeModel, StatusInput, TrendBuffer, TrendKind};
use jafnify_core::wifi_leg::WifiLeg;

/// The local time of the case, in UTC+3.
const NOW: i64 = 1_795_068_960_000;
/// The offset of the local time, in minutes.
const OFFSET: i32 = 180;
/// An hour, in milliseconds.
const HOUR: i64 = 3_600_000;

#[test]
fn the_text_of_the_session_without_a_drop_is_written() {
    let home = home_with_events(&yesterday_events());
    let file = write_dump("laptop-home-0916-session", &home);
    assert!(file.exists(), "the text of the session is written");
    let text = std::fs::read_to_string(&file).expect("the text of the session");
    assert!(
        text.contains("No drops this session"),
        "the session without a drop says it: {text}"
    );
    assert!(
        text.contains("Last: yesterday 17:21, Phone away 1:46:17"),
        "the session names the newest event it does not count: {text}"
    );
}

#[test]
fn the_text_of_a_drop_in_the_session_is_written() {
    let mut events = yesterday_events();
    events.push(Event::Drop(drop_record(NOW - 2 * 60_000, 12_000)));
    let home = home_with_events(&events);
    let file = write_dump("laptop-home-drop-in-scope", &home);
    assert!(file.exists(), "the text of the drop is written");
    let text = std::fs::read_to_string(&file).expect("the text of the drop");
    assert!(
        text.contains("DROPS SESSION"),
        "the tile of the drops is in the home: {text}"
    );
    assert!(
        text.contains("8:outside 9:drop"),
        "the hour of the drop inside the window is the one drop of the strip: {text}"
    );
}

/// The drops of the day before and the away that came after them.
fn yesterday_events() -> Vec<Event> {
    vec![
        Event::Drop(drop_record(1_795_006_920_000, 113_000)),
        Event::Drop(drop_record(1_795_007_040_000, 61_000)),
        Event::Away {
            at_ms: 1_795_011_660_000,
            down_ms: Some(6_377_000),
        },
    ]
}

/// One drop of the history.
fn drop_record(at_ms: i64, down_ms: u64) -> DropRecord {
    DropRecord {
        at_ms,
        reason: REASON_PEER_LEFT.to_string(),
        phone: None,
        wifi: None,
        down_ms: Some(down_ms),
        exit: None,
        link: Default::default(),
    }
}

/// The home screen of the laptop with the given drops.
fn home_with_events(events: &[Event]) -> HomeModel {
    let session = ScopeSession {
        started_at_ms: NOW - 6 * 60_000,
        up_bytes: 12_345_678,
        down_bytes: 98_765_432,
        peak_down_bps: 8_400_000,
    };
    let status = tiles::status_card(&StatusInput {
        connected: true,
        name: Some("Pixel 7"),
        path: Some("Wi-Fi > Phone"),
        uptime_secs: Some(6 * 60),
        notice: Some("Good time for big downloads"),
    });
    let stats = PhoneStats {
        net: NetworkKind::Mobile5g,
        signal: Some(3),
        rtt_ms: Some(28),
        up_bps: 900_000,
        down_bps: 8_400_000,
        battery: Some(62),
        temp_c_x10: Some(310),
        used_bytes: 4_000_000_000,
        allow_bytes: 8_000_000_000,
        reset_day: 1,
        month: 0,
        switches: Some(2),
        dbm: Some(-81),
        thr: Some(1),
        ..Default::default()
    };
    let mut tiles = Vec::new();
    if let Some(tile) = tiles::dns_tile(stats.rtt_ms, &trend(&[28, 41, 61]), Some(45)) {
        tiles.push(tile);
    }
    if let Some(tile) = tiles::signal_tile(&stats, &trend(&[-97, -88, -81])) {
        tiles.push(tile);
    }
    if let Some(tile) = tiles::phone_tile(
        stats.temp_c_x10,
        stats.thr,
        stats.hr30,
        stats.battery,
        &trend(&[300, 305, 310]),
        None,
    ) {
        tiles.push(tile);
    }
    if let Some(tile) = tiles::wifi_tile(&WifiLeg::new(12, 0, 120), &trend(&[9, 12, 15]), None) {
        tiles.push(tile);
    }
    if let Some(tile) = tiles::today_tile(
        &ScopeTotals {
            up_bytes: session.up_bytes,
            down_bytes: session.down_bytes,
            peak_down_bps: session.peak_down_bps,
        },
        TileScope::Today,
        tiles::Measure::Laptop,
    ) {
        tiles.push(tile);
    }
    if let Some(tile) = tiles::drops_tile(events, NOW, OFFSET, TileScope::Session, Some(&session)) {
        tiles.push(tile);
    }
    let hours = tiles::hours_strip(&summary());
    tiles::home_model(status, tiles, Some(hours))
}

/// A trend of the given values, sampled over the last five minutes.
fn trend(values: &[i64]) -> TrendBuffer {
    let mut buffer = TrendBuffer::new();
    for (index, value) in values.iter().enumerate() {
        buffer.push(NOW - (values.len() - index) as i64 * 60_000, *value);
    }
    buffer
}

/// The good hours of the last three days: the strip still learns.
fn summary() -> Summary {
    let records = hour_records();
    Summary::build(&records, NOW, OFFSET)
}

/// The hours of the last two days: the strip is still learning the busy
/// hours, so it has no verdict yet.
fn hour_records() -> Vec<HourRecord> {
    let first = jafnify_core::usage::hour_start_ms(NOW) - 2 * 24 * HOUR;
    let mut records = Vec::new();
    let mut at = first;
    while at <= NOW {
        let hour = jafnify_core::usage::local_hour(at, i64::from(OFFSET) * 60_000);
        let busy = (9..18).contains(&hour);
        records.push(HourRecord {
            hour_start_ms: at,
            samples: 60,
            dns_ms: Some(if busy { 220 } else { 30 }),
            five_g_pct: 60,
            signal: Some(3),
            temp_c_x10: Some(310),
            thr: Some(1),
            partial: at + HOUR > NOW,
        });
        at += HOUR;
    }
    records
}

/// Write the text of the home model under the test scratch directory.
fn write_dump(name: &str, home: &HomeModel) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tmp")
        .join("h38")
        .join("shots");
    std::fs::create_dir_all(&dir).expect("the folder of the dumps");
    let file = dir.join(format!("{name}.txt"));
    std::fs::write(&file, dump(home)).expect("the text of the home");
    file
}

/// The text of the home model: the words the window draws, line by line.
fn dump(home: &HomeModel) -> String {
    let mut out = String::new();
    out.push_str("Jafnify home of the laptop\n");
    out.push_str("the local time of the case: 2026-11-19 09:16 (UTC+3)\n\n");
    out.push_str("status\n");
    out.push_str(&format!("  title: {}\n", home.status.title));
    if let Some(subline) = &home.status.subline {
        out.push_str(&format!("  subline: {subline}\n"));
    }
    if let Some(notice) = &home.status.notice {
        out.push_str(&format!("  notice: {notice}\n"));
    }
    for tile in &home.tiles {
        out.push_str(&format!("\ntile {}\n", tile.id));
        out.push_str(&format!("  label: {}\n", tile.label));
        if let Some(badge) = &tile.badge {
            out.push_str(&format!("  badge: {}\n", badge.text));
        }
        if let Some(big) = &tile.big {
            match &big.unit {
                Some(unit) => out.push_str(&format!("  big: {} {unit}\n", big.value)),
                None => out.push_str(&format!("  big: {}\n", big.value)),
            }
        }
        if let Some(facts) = &tile.facts {
            out.push_str("  facts:\n");
            for line in &facts.lines {
                out.push_str(&format!("    - {line}\n"));
            }
        }
        if let Some(note) = &tile.note {
            out.push_str(&format!("  note: {note}\n"));
        }
        if let Some(trend) = &tile.trend {
            if let TrendKind::Hours { cells } = &trend.kind {
                out.push_str("  strip (the last day, oldest first; a cell outside the window of the tile is \"outside\"):\n    ");
                for cell in cells {
                    out.push_str(&format!(
                        "{}:{} ",
                        cell.hour,
                        jafnify_core::tiles::cell_word(cell)
                    ));
                }
                out.push('\n');
            }
        }
    }
    if let Some(hours) = &home.hours {
        out.push_str("\nhours strip\n");
        out.push_str(&format!("  title: {}\n", hours.title));
        if let Some(caption) = &hours.caption {
            out.push_str(&format!("  caption: {caption}\n"));
        }
        if let Some(learning) = &hours.learning {
            out.push_str(&format!("  learning: {learning}\n"));
        }
        out.push_str("  cells:\n    ");
        for cell in &hours.cells {
            out.push_str(&format!(
                "{}:{} ",
                cell.hour,
                jafnify_core::tiles::cell_word(cell)
            ));
        }
        out.push('\n');
    }
    out.push_str("\nthe text the phone parses of the same model:\n");
    out.push_str(&tiles::phone_home_text(home));
    out
}

/// The tiles that changed, as the phone and the laptop draw them - the
/// window of the phone's heat and speed lines, and the session tile of both
/// screens saying what it counts, in the same direction.
#[test]
fn the_text_of_the_h39_tiles_is_written() {
    use jafnify_core::tiles::Trends;
    let stats = PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        rtt_ms: Some(34),
        battery: Some(74),
        temp_c_x10: Some(315),
        up_bps: 300_000,
        down_bps: 1_200_000,
        ..Default::default()
    };
    let mut trends = Trends::new();
    for second in 0..300 {
        trends.push(NOW - 300_000 + second * 1_000, &stats, Some(40));
    }
    // The numbers of the case, the phone's turned to the laptop's
    // direction: the laptop sent 821.4 KB, the phone counted it encrypted.
    let laptop = ScopeTotals {
        up_bytes: 821_400,
        down_bytes: 487_700,
        peak_down_bps: 1_100_000,
    };
    let phone = ScopeTotals {
        up_bytes: 1_500_000,
        down_bytes: 604_700,
        peak_down_bps: 678_800,
    };
    let mut home = home_with_events(&[]);
    home.tiles = [
        tiles::heat_tile(Some(315), Some(2), None, trends.phone()),
        tiles::speed_tile(&stats, trends.speed(), TileScope::Session, 678_800),
        tiles::phone_today_tile(&phone, 0, 0, TileScope::Session),
        tiles::today_tile(&laptop, TileScope::Session, tiles::Measure::Laptop),
    ]
    .into_iter()
    .flatten()
    .collect();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tmp")
        .join("h39")
        .join("shots");
    std::fs::create_dir_all(&dir).expect("the folder of the dumps");
    let text = dump(&home).replacen(
        "Jafnify home of the laptop",
        "tiles: phone heat, speed, session; laptop session",
        1,
    );
    std::fs::write(dir.join("h39-tiles.txt"), &text).expect("the text of the tiles");
    assert!(text.contains("in 5 min"), "{text}");
    assert!(text.contains("Counted with encryption"), "{text}");
    assert!(text.contains("Counted in the tunnel"), "{text}");
}

/// The text of a hot phone (thermal headroom 105 percent) and a Wi-Fi
/// leg that shows the phone's 3 percent WireGuard loss. Run explicitly with
/// `cargo test --test home_dump_test -- --ignored`; it only writes the dumps.
#[test]
#[ignore]
fn h43_hot_phone_and_loss_text_dumps() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tmp")
        .join("h43")
        .join("shots");
    std::fs::create_dir_all(&dir).expect("the folder of the dumps");

    let stats = PhoneStats {
        net: NetworkKind::Mobile5g,
        signal: Some(3),
        rtt_ms: Some(28),
        battery: Some(62),
        temp_c_x10: Some(450),
        dbm: Some(-81),
        thr: Some(2),
        hr30: Some(105),
        ..Default::default()
    };

    let mut phone_trend = TrendBuffer::new();
    for index in 0..5 {
        phone_trend.push(NOW - (5 - index) as i64 * 60_000, 450);
    }
    let mut wifi_trend = TrendBuffer::new();
    for index in 0..5 {
        wifi_trend.push(NOW - (5 - index) as i64 * 60_000, 40);
    }

    let mut laptop_tiles = Vec::new();
    if let Some(tile) = tiles::phone_tile(
        stats.temp_c_x10,
        stats.thr,
        stats.hr30,
        stats.battery,
        &phone_trend,
        None,
    ) {
        laptop_tiles.push(tile);
    }
    if let Some(tile) = tiles::wifi_tile(&WifiLeg::new(40, 3, 30), &wifi_trend, Some(3)) {
        laptop_tiles.push(tile);
    }
    let status = tiles::status_card(&StatusInput {
        connected: true,
        name: Some("Pixel 7"),
        path: Some("Wi-Fi > Phone"),
        uptime_secs: Some(6 * 60),
        notice: None,
    });
    let laptop = tiles::home_model(status, laptop_tiles, None);
    std::fs::write(
        dir.join("laptop-tiles-hot-hr30-105-loss-3.txt"),
        dump(&laptop),
    )
    .expect("the text of the laptop tiles");

    let mut trends = tiles::Trends::new();
    for index in 0..5 {
        trends.push(NOW - (5 - index) as i64 * 60_000, &stats, Some(40));
    }
    let input = tiles::PhoneHomeInput {
        state: tiles::PhoneShareState::Sharing,
        state_line: None,
        notice: None,
        uptime_secs: Some(6 * 60),
        today: None,
        stats: Some(&stats),
        network_permission: true,
        trends: &trends,
        charging: Some(false),
        battery_saver: Some(false),
        show_dns: true,
        show_battery: true,
        today_scope: TileScope::Today,
        speed_scope: TileScope::Session,
        average_scope: TileScope::Today,
        now_ms: NOW,
        session: None,
        range: None,
    };
    let model = tiles::phone_home(&input);
    std::fs::write(dir.join("phone-home-hot-hr30-105.txt"), tiles::phone_home_text(&model))
        .expect("the text of the phone home");
}
