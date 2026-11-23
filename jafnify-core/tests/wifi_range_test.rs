//! The range of the laptop from the phone: the level the laptop hears from
//! the hotspot, its words, its tile and what it says about a drop.

use jafnify_core::drop_history::{
    parse_line, DropRecord, Entry, HINT_HOTSPOT_GONE, HINT_OUT_OF_RANGE, HINT_RESET,
    REASON_NO_NETWORK, REASON_RESET,
};
use jafnify_core::tiles::{Tone, TrendBuffer};
use jafnify_core::wifi_range::{
    parse_proc_wireless, range_tile, range_word, weakest_before, LinkEvidence, LOOKBACK_MS,
    WORD_CLOSE, WORD_EDGE, WORD_FAIR, WORD_FAR,
};

const PROC: &str =
    "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
  wlo1: 0000   70.  -38.  -256        0      0      0     38    909        0
 wlan1: 0000   40.  200.  -256        0      0      0      0      0        0
 wlan2: 0000    0.    0.  -256        0      0      0      0      0        0
";

#[test]
fn the_level_is_read_for_the_uplink_only() {
    assert_eq!(parse_proc_wireless(PROC, "wlo1"), Some(-38));
    // An unsigned byte is a negative level.
    assert_eq!(parse_proc_wireless(PROC, "wlan1"), Some(-56));
    // No level is no reading, not 0 dBm.
    assert_eq!(parse_proc_wireless(PROC, "wlan2"), None);
    // A cable or a lab veth is not listed.
    assert_eq!(parse_proc_wireless(PROC, "veth0"), None);
    assert_eq!(parse_proc_wireless("", "wlo1"), None);
    assert_eq!(parse_proc_wireless("  wlo1: 0000 70.", "wlo1"), None);
}

#[test]
fn the_words_follow_the_level() {
    assert_eq!(range_word(-38), WORD_CLOSE);
    assert_eq!(range_word(-60), WORD_CLOSE);
    assert_eq!(range_word(-61), WORD_FAIR);
    assert_eq!(range_word(-70), WORD_FAIR);
    assert_eq!(range_word(-71), WORD_FAR);
    assert_eq!(range_word(-77), WORD_FAR);
    assert_eq!(range_word(-78), WORD_EDGE);
    assert!(
        [WORD_CLOSE, WORD_FAIR, WORD_FAR, WORD_EDGE]
            .iter()
            .all(|w| w.starts_with("Likely")),
        "a level is a hint of the distance, never a measure"
    );
}

#[test]
fn the_weakest_level_before_a_drop_is_its_evidence() {
    let at = 1_000_000;
    let mut trend = TrendBuffer::new();
    trend.push(at - LOOKBACK_MS - 1_000, -85); // too old to count
    trend.push(at - 20_000, -66);
    trend.push(at - 5_000, -79);
    trend.push(at + 3_000, -82); // after the link went, before it was seen
    assert_eq!(weakest_before(&trend, at, at + 5_000), Some(-82));
    assert_eq!(weakest_before(&trend, at, at), Some(-79));
    assert_eq!(weakest_before(&TrendBuffer::new(), at, at), None);
}

#[test]
fn the_tile_says_its_window_and_the_likely_distance() {
    let mut trend = TrendBuffer::new();
    for (i, dbm) in [-74, -70, -66, -65].iter().enumerate() {
        trend.push(1_000_000 + i as i64 * 1_000, *dbm);
    }
    let tile = range_tile(&report(-65, Some(5850), false), &trend);
    assert_eq!(tile.label, "RANGE");
    let big = tile.big.as_ref().unwrap();
    assert_eq!(big.value, "-65");
    assert_eq!(big.unit.as_deref(), Some("dBm"));
    assert_eq!(big.tone, Tone::Muted);
    let facts = &tile.facts.as_ref().unwrap().lines;
    assert_eq!(facts[0], "-74 to -65 dBm in 5 min");
    assert_eq!(facts[1], WORD_FAIR);
    assert!(tile.trend.is_some(), "the line is drawn");
    assert_eq!(tile.note.as_deref(), Some("Link 585.0 Mbit/s"));

    // Far is the waiting tone, never the fault tone; a flat line is one value.
    let mut flat = TrendBuffer::new();
    flat.push(1_000_000, -76);
    let tile = range_tile(&report(-76, None, false), &flat);
    assert_eq!(tile.big.as_ref().unwrap().tone, Tone::Wait);
    assert_eq!(tile.facts.as_ref().unwrap().lines[0], "-76 dBm in 5 min");
}

fn record(reason: &str, range_dbm: Option<i32>) -> DropRecord {
    let link = LinkEvidence {
        weakest_dbm: range_dbm,
        ..LinkEvidence::default()
    };
    DropRecord {
        at_ms: 1_795_133_054_711,
        reason: reason.to_string(),
        phone: None,
        wifi: None,
        down_ms: None,
        exit: None,
        link,
    }
}

#[test]
fn a_drop_at_the_edge_of_range_says_so() {
    assert_eq!(
        record(REASON_NO_NETWORK, Some(-81)).hint(),
        HINT_OUT_OF_RANGE
    );
    assert_eq!(record("no-data", Some(-78)).hint(), HINT_OUT_OF_RANGE);
    // Strong right up to the loss: the hotspot went, the distance did not.
    assert_eq!(
        record(REASON_NO_NETWORK, Some(-45)).hint(),
        HINT_HOTSPOT_GONE
    );
    // No reading (old drops, a cable): the old hint.
    assert_eq!(record(REASON_NO_NETWORK, None).hint(), HINT_HOTSPOT_GONE);
    // A reset comes from the phone app whatever the level.
    assert_eq!(record(REASON_RESET, Some(-85)).hint(), HINT_RESET);
}

#[test]
fn the_level_travels_in_the_line_and_old_lines_still_read() {
    let line = record(REASON_NO_NETWORK, Some(-81)).drop_line();
    assert!(line.ends_with(" range_dbm=-81"), "{line}");
    match parse_line(&line) {
        Ok(Entry::Drop(back)) => assert_eq!(back.link.weakest_dbm, Some(-81)),
        other => panic!("{other:?}"),
    }
    let old = "JFD1 1 drop at=1795133054711 reason=no-network net=5g-adv sig=3";
    match parse_line(old) {
        Ok(Entry::Drop(back)) => assert_eq!(back.link, LinkEvidence::default()),
        other => panic!("{other:?}"),
    }
}

use jafnify_core::wifi_range::{link_log_line, StationRing, StationSample};

fn reading(avg: i32, rate: u32, retries: u32, bloss: u32) -> StationSample {
    StationSample {
        signal_dbm: Some(avg),
        signal_avg_dbm: Some(avg),
        tx_rate_100k: Some(rate),
        tx_retries: Some(retries),
        tx_failed: Some(1),
        beacon_loss: Some(bloss),
    }
}

#[test]
fn the_ring_gives_the_evidence_of_the_seconds_before_a_drop() {
    let at = 10_000_000;
    let mut ring = StationRing::new();
    ring.push(at - 60_000, reading(-90, 10, 0, 50)); // before the lookback
    ring.push(at - 20_000, reading(-64, 8667, 100, 3));
    ring.push(at - 5_000, reading(-73, 1300, 180, 4));
    ring.push(at + 2_000, reading(-75, 650, 260, 9));
    let link = ring.evidence(at, at + 4_000);
    assert_eq!(link.weakest_dbm, Some(-75));
    assert_eq!(link.lowest_rate_100k, Some(650));
    assert_eq!(link.retries, Some(160));
    assert_eq!(link.failed, Some(0));
    assert_eq!(link.beacon_loss, Some(6));
    // Beacons missed while already far: likely the distance.
    assert!(link.out_of_range());

    // A counter that went down is a new association: no growth claimed.
    let mut ring = StationRing::new();
    ring.push(at - 10_000, reading(-60, 8667, 500, 9));
    ring.push(at - 5_000, reading(-60, 8667, 20, 0));
    let link = ring.evidence(at, at);
    assert_eq!(link.retries, None);
    assert_eq!(link.beacon_loss, None);
    // Strong to the end: the hotspot went, not the laptop.
    assert!(!link.out_of_range());

    // The ring forgets what is older than its window.
    let mut ring = StationRing::new();
    ring.push(0, reading(-60, 1, 1, 1));
    ring.push(
        jafnify_core::wifi_range::RING_WINDOW_MS + 1,
        reading(-60, 1, 1, 1),
    );
    assert_eq!(ring.len(), 1);
}

#[test]
fn the_whole_evidence_travels_in_the_drop_line() {
    let mut drop = record(REASON_NO_NETWORK, Some(-79));
    drop.link = LinkEvidence {
        weakest_dbm: Some(-79),
        lowest_rate_100k: Some(65),
        retries: Some(320),
        failed: Some(2),
        beacon_loss: Some(7),
        phone_loss_pct: Some(12),
        phone_rtt_ms: Some(41),
    };
    let line = drop.drop_line();
    assert!(
        line.ends_with(
            " range_dbm=-79 rate_min=65 retry_d=320 fail_d=2 bloss_d=7 ploss=12 prtt=41"
        ),
        "{line}"
    );
    match parse_line(&line) {
        Ok(Entry::Drop(back)) => assert_eq!(back.link, drop.link),
        other => panic!("{other:?}"),
    }
    assert!(parse_line("JFD1 1 drop at=1 reason=no-network bloss_d=x").is_err());
}

#[test]
fn the_link_log_holds_numbers_only() {
    let line = link_log_line(1_795_150_081_266, &reading(-34, 8667, 3970, 0));
    assert_eq!(
        line,
        "JFL1 1 at=1795150081266 sig=-34 avg=-34 rate=8667 retry=3970 fail=1 bloss=0"
    );
    let line = link_log_line(5, &StationSample::level_only(-50));
    assert_eq!(line, "JFL1 1 at=5 sig=-50");
}

use jafnify_core::wifi_range::RangeReport;

fn report(level_dbm: i32, rate_100k: Option<u32>, missed_beacons: bool) -> RangeReport {
    RangeReport {
        level_dbm,
        rate_100k,
        missed_beacons,
    }
}

#[test]
fn missed_beacons_are_the_waiting_tone_even_when_the_level_looks_fine() {
    let mut trend = TrendBuffer::new();
    trend.push(1_000_000, -62);
    let tile = range_tile(&report(-62, Some(4333), true), &trend);
    assert_eq!(tile.big.as_ref().unwrap().tone, Tone::Wait);
    assert_eq!(tile.note.as_deref(), Some("433.3 Mbit/s, beacons missed"));
    let tile = range_tile(&report(-62, None, false), &trend);
    assert_eq!(tile.note, None);
}

#[test]
fn the_report_is_made_of_the_newest_reading_and_travels_on_the_wire() {
    let sample = reading(-71, 1300, 10, 2);
    let mut ring = StationRing::new();
    ring.push(0, reading(-60, 8667, 0, 0));
    ring.push(5_000, sample);
    let recent = ring.evidence(5_000, 5_000);
    let made = RangeReport::of(&sample, &recent).unwrap();
    assert_eq!(made, report(-71, Some(1300), true));
    let words = made.wire_words();
    assert_eq!(words, " lvl=-71 rate=1300 mb=1");
    let back = RangeReport::from_wire_words(words.split_whitespace()).unwrap();
    assert_eq!(back, made);
    // No level, a broken value or a level that makes no sense: no report.
    assert!(RangeReport::from_wire_words("rate=5".split_whitespace()).is_none());
    assert!(RangeReport::from_wire_words("lvl=x".split_whitespace()).is_none());
    assert!(RangeReport::from_wire_words("lvl=12".split_whitespace()).is_none());
    // Unknown words of a newer laptop are ignored.
    assert_eq!(
        RangeReport::from_wire_words("lvl=-50 new=1".split_whitespace()),
        Some(report(-50, None, false))
    );
    assert!(RangeReport::of(&StationSample::default(), &recent).is_none());
}
