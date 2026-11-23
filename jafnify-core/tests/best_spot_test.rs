//! The best-spot finder: the window, the ranking and the fixed wording.
//!
//! Pure module: no clock, no file, no socket. The caller hands over the
//! samples and the marks and gets back the lines.

use jafnify_core::best_spot::{
    best, hold_line, live_line, parse_marks, parse_samples, session_over, spot_at, spot_line,
    verdict, Mark, Sample, Spot, MIN_SAMPLES, SESSION_SECS, WINDOW_SECS,
};
use jafnify_core::phone_stats::NetworkKind;

fn sample(at_ms: i64, net: NetworkKind, level: u8, dbm: Option<i16>) -> Sample {
    Sample {
        at_ms,
        net,
        level,
        dbm,
        rtt_ms: None,
        wifi_ms: None,
    }
}

fn spot(number: usize, net: NetworkKind, dbm: Option<i16>, level: u8) -> Spot {
    Spot {
        number,
        net,
        dbm,
        level,
        rtt_ms: None,
        samples: MIN_SAMPLES,
    }
}

#[test]
fn the_constants_are_the_agreed_ones() {
    assert_eq!(SESSION_SECS, 120);
    assert_eq!(WINDOW_SECS, 15);
    assert_eq!(MIN_SAMPLES, 5);
}

#[test]
fn a_spot_counts_only_the_last_fifteen_seconds() {
    let mark = Mark {
        at_ms: 60_000,
        number: 1,
    };
    let mut samples = Vec::new();
    for second in 0..6 {
        samples.push(sample(
            50_000 + second * 1_000,
            NetworkKind::Mobile5g,
            3,
            Some(-100),
        ));
    }
    // One second too old, and one after the mark: neither is in the window.
    samples.push(sample(44_999, NetworkKind::Mobile2g, 1, Some(-130)));
    samples.push(sample(60_001, NetworkKind::Mobile2g, 1, Some(-130)));

    let spot = spot_at(&samples, mark).expect("the window has enough samples");
    assert_eq!(spot.samples, 6);
    assert_eq!(spot.net, NetworkKind::Mobile5g);
    assert_eq!(spot.number, 1);
}

#[test]
fn fewer_than_five_samples_make_no_spot() {
    let mark = Mark {
        at_ms: 10_000,
        number: 1,
    };
    let samples: Vec<Sample> = (0..4)
        .map(|second| sample(7_000 + second * 1_000, NetworkKind::Mobile5g, 3, Some(-100)))
        .collect();
    assert!(
        spot_at(&samples, mark).is_none(),
        "four samples are not enough"
    );
    let samples: Vec<Sample> = (0..5)
        .map(|second| sample(6_000 + second * 1_000, NetworkKind::Mobile5g, 3, Some(-100)))
        .collect();
    assert!(spot_at(&samples, mark).is_some(), "five samples are enough");
}

#[test]
fn a_spot_takes_the_medians_and_the_most_seen_network() {
    let mark = Mark {
        at_ms: 20_000,
        number: 3,
    };
    let samples = vec![
        sample(10_000, NetworkKind::Mobile4g, 1, Some(-120)),
        sample(11_000, NetworkKind::Mobile5g, 2, Some(-100)),
        sample(12_000, NetworkKind::Mobile5g, 4, Some(-90)),
        sample(13_000, NetworkKind::Mobile4g, 3, Some(-110)),
        sample(14_000, NetworkKind::Mobile5g, 3, Some(-95)),
    ];
    let spot = spot_at(&samples, mark).expect("a spot");
    assert_eq!(spot.net, NetworkKind::Mobile5g, "5G was seen most often");
    assert_eq!(spot.dbm, Some(-100), "the median dBm");
    assert_eq!(spot.level, 3, "the median level");
    assert_eq!(spot.samples, 5);
}

#[test]
fn a_tie_in_the_network_count_goes_to_the_later_kind() {
    let mark = Mark {
        at_ms: 20_000,
        number: 1,
    };
    let samples = vec![
        sample(10_000, NetworkKind::Mobile5g, 3, Some(-100)),
        sample(11_000, NetworkKind::Mobile4g, 3, Some(-100)),
        sample(12_000, NetworkKind::Mobile5g, 3, Some(-100)),
        sample(13_000, NetworkKind::Mobile4g, 3, Some(-100)),
        sample(14_000, NetworkKind::Wifi, 3, Some(-100)),
    ];
    let spot = spot_at(&samples, mark).expect("a spot");
    assert_eq!(
        spot.net,
        NetworkKind::Mobile4g,
        "the later kind wins the tie"
    );
}

#[test]
fn the_best_spot_prefers_five_g_over_lte_over_the_rest() {
    let five_g = spot(1, NetworkKind::Mobile5g, Some(-120), 1);
    let lte = spot(2, NetworkKind::Mobile4g, Some(-60), 4);
    let other = spot(3, NetworkKind::Wifi, Some(-40), 4);
    assert_eq!(best(&[other, lte, five_g]).unwrap().number, 1);
    assert_eq!(best(&[other, lte]).unwrap().number, 2);
    assert_eq!(best(&[five_g, other]).unwrap().number, 1);
}

#[test]
fn the_same_family_is_decided_by_the_dbm_and_then_by_the_level() {
    let weak = spot(1, NetworkKind::Mobile5g, Some(-120), 4);
    let strong = spot(2, NetworkKind::Mobile5g, Some(-90), 1);
    assert_eq!(best(&[weak, strong]).unwrap().number, 2, "the dBm decides");

    let same_dbm_low = spot(1, NetworkKind::Mobile5g, Some(-100), 1);
    let same_dbm_high = spot(2, NetworkKind::Mobile5g, Some(-100), 3);
    assert_eq!(
        best(&[same_dbm_low, same_dbm_high]).unwrap().number,
        2,
        "the level decides"
    );
}

#[test]
fn a_missing_dbm_falls_back_to_the_level() {
    let with_dbm = spot(1, NetworkKind::Mobile5g, Some(-120), 1);
    let without_dbm = spot(2, NetworkKind::Mobile5g, None, 3);
    assert_eq!(
        best(&[with_dbm, without_dbm]).unwrap().number,
        2,
        "only one spot has a dBm: the level decides"
    );
}

#[test]
fn a_tie_keeps_the_earlier_spot() {
    let first = spot(1, NetworkKind::Mobile5g, Some(-100), 3);
    let second = spot(2, NetworkKind::Mobile5g, Some(-100), 3);
    assert_eq!(best(&[first, second]).unwrap().number, 1);
    assert_eq!(best(&[second, first]).unwrap().number, 2);
}

#[test]
fn no_spots_have_no_best() {
    assert!(best(&[]).is_none());
}

#[test]
fn the_live_line_carries_the_fixed_wording() {
    let sample = Sample {
        at_ms: 1_000,
        net: NetworkKind::Mobile5gAdv,
        level: 3,
        dbm: Some(-97),
        rtt_ms: Some(40),
        wifi_ms: None,
    };
    assert_eq!(
        live_line(Some(&sample)),
        "Now: 5G (advanced), -97 dBm, signal 3/4, DNS 40 ms"
    );

    let with_wifi = Sample {
        wifi_ms: Some(30),
        ..sample
    };
    assert_eq!(
        live_line(Some(&with_wifi)),
        "Now: 5G (advanced), -97 dBm, signal 3/4, DNS 40 ms, Wi-Fi 30 ms"
    );

    let bare = Sample {
        net: NetworkKind::Mobile4g,
        level: 2,
        dbm: None,
        rtt_ms: None,
        wifi_ms: None,
        ..sample
    };
    assert_eq!(live_line(Some(&bare)), "Now: 4G, signal 2/4");

    assert_eq!(live_line(None), hold_line());
}

#[test]
fn the_spot_line_carries_the_fixed_wording() {
    let marked = spot(2, NetworkKind::Mobile5gAdv, Some(-95), 3);
    assert_eq!(
        spot_line(&marked),
        "Spot 2: 5G (advanced), -95 dBm, signal 3/4"
    );

    let without_dbm = spot(3, NetworkKind::Mobile4g, None, 2);
    assert_eq!(spot_line(&without_dbm), "Spot 3: 4G, signal 2/4");
}

#[test]
fn the_verdict_never_calls_a_spot_the_best_one() {
    assert_eq!(verdict(&[]), "Mark another spot to compare");

    let first = spot(1, NetworkKind::Mobile5g, Some(-100), 3);
    assert_eq!(verdict(&[first]), "Mark another spot to compare");

    let second = spot(2, NetworkKind::Mobile5g, Some(-90), 3);
    assert_eq!(verdict(&[first, second]), "Best so far: Spot 2");
    assert_eq!(verdict(&[second, first]), "Best so far: Spot 2");
}

#[test]
fn the_hold_line_is_fixed() {
    assert_eq!(hold_line(), "Hold the phone still a few seconds longer");
}

#[test]
fn the_session_ends_after_two_minutes() {
    assert!(!session_over(0, 0));
    assert!(!session_over(0, 119_999), "one second before the end");
    assert!(
        session_over(0, 120_000),
        "the session is over at two minutes"
    );
    assert!(session_over(0, 200_000));

    let started = 1_000_000;
    assert!(!session_over(started, started + 30_000), "an early stop");
    assert!(!session_over(started, started), "the start is not the end");
}

#[test]
fn the_samples_parse_from_the_wire_text() {
    let text = "1000,5g-adv,3,-97,40,30\n2000,4g,2,,,\n";
    let samples = parse_samples(text);
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].at_ms, 1_000);
    assert_eq!(samples[0].net, NetworkKind::Mobile5gAdv);
    assert_eq!(samples[0].level, 3);
    assert_eq!(samples[0].dbm, Some(-97));
    assert_eq!(samples[0].rtt_ms, Some(40));
    assert_eq!(samples[0].wifi_ms, Some(30));
    assert_eq!(samples[1].net, NetworkKind::Mobile4g);
    assert_eq!(samples[1].level, 2);
    assert_eq!(samples[1].dbm, None);
    assert_eq!(samples[1].rtt_ms, None);
    assert_eq!(samples[1].wifi_ms, None);
}

#[test]
fn a_sample_line_that_does_not_parse_is_skipped() {
    let text = "not a sample\n1000,5g,3,-97,40,30\n2000,5g\n3000,5g,3,-97,40,30,extra\n";
    let samples = parse_samples(text);
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].at_ms, 1_000);
}

#[test]
fn the_marks_parse_from_the_wire_text() {
    let text = "5000,1\n6000,2\nbad line\n7000,x\n";
    let marks = parse_marks(text);
    assert_eq!(marks.len(), 2);
    assert_eq!(
        marks[0],
        Mark {
            at_ms: 5_000,
            number: 1
        }
    );
    assert_eq!(
        marks[1],
        Mark {
            at_ms: 6_000,
            number: 2
        }
    );
}
