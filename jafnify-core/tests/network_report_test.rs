//! The network report core: the generation words, the switch counting, the
//! hourly buckets and the wording the phone and the laptop share.

use jafnify_core::network_report::{
    push_bucket, sane_mah, window_lines, Generation, GenerationTracker, NetworkBucket,
    NetworkSummary, Stability,
};

fn bucket() -> NetworkBucket {
    NetworkBucket {
        hour_start_ms: 3_600_000,
        switches: 7,
        generation_ms: vec![(Generation::Nr, 2_520_000), (Generation::Lte, 1_080_000)],
        up_bytes: 300_000,
        down_bytes: 2_100_000,
        drops: 2,
        drain_mah: Some(240),
        drain_pct: None,
        charging: false,
        connected_ms: 3_600_000,
        samples: 60,
    }
}

#[test]
fn the_window_keeps_only_the_last_24_hours() {
    let day = 24 * 3_600_000;
    let mut buckets = Vec::new();
    for index in 0..30 {
        let mut bucket = bucket();
        bucket.hour_start_ms = index * day;
        bucket.switches = 1;
        buckets.push(bucket);
    }
    let now = 29 * day + 3_600_000;
    let lines = window_lines(&buckets, now);
    assert!(
        lines.iter().any(|line| line == "Network switches: 1"),
        "{lines:?}"
    );
}

#[test]
fn every_generation_has_a_label_and_a_wire_word() {
    for (gen, label, wire) in [
        (Generation::Lte, "4G", "4g"),
        (Generation::LteCa, "4G (aggregated)", "4g-ca"),
        (Generation::LteAdvancedPro, "4G+", "4g+"),
        (Generation::Nr, "5G", "5g"),
        (Generation::NrNsa, "5G (non-standalone)", "5g-nsa"),
        (Generation::NrNsaMmwave, "5G (mmWave)", "5g-mmwave"),
        (Generation::NrAdvanced, "5G (advanced)", "5g-adv"),
    ] {
        assert_eq!(gen.label(), label);
        assert_eq!(gen.wire(), wire);
        assert_eq!(Generation::from_wire(wire), Some(gen));
    }
    assert_eq!(Generation::from_wire("6g"), None);
}

#[test]
fn a_bucket_round_trips_through_its_line() {
    let line = bucket().line();
    assert!(line.starts_with("JFN1 1 "), "{line}");
    let parsed = NetworkBucket::parse_line(&line).expect("parses");
    assert_eq!(parsed.hour_start_ms, 3_600_000);
    assert_eq!(parsed.switches, 7);
    assert_eq!(
        parsed.generation_ms,
        vec![(Generation::Nr, 2_520_000), (Generation::Lte, 1_080_000)]
    );
    assert_eq!(parsed.up_bytes, 300_000);
    assert_eq!(parsed.down_bytes, 2_100_000);
    assert_eq!(parsed.drops, 2);
    assert_eq!(parsed.drain_mah, Some(240));
    assert_eq!(parsed.drain_pct, None);
    assert!(!parsed.charging);
    assert_eq!(parsed.connected_ms, 3_600_000);
    assert_eq!(parsed.samples, 60);
}

#[test]
fn a_bad_line_is_refused() {
    assert!(NetworkBucket::parse_line("JFU1 1 hour=0").is_err());
    assert!(NetworkBucket::parse_line("JFN1 2 hour=0").is_err());
    assert!(NetworkBucket::parse_line(
        "JFN1 1 hour=x switches=0 up=0 down=0 drops=0 charging=0 connected_ms=0 samples=0"
    )
    .is_err());
}

#[test]
fn the_same_hour_merges_and_the_cap_keeps_720_hours() {
    let mut buckets = Vec::new();
    push_bucket(&mut buckets, bucket());
    let mut second = bucket();
    second.switches = 1;
    second.up_bytes = 10;
    push_bucket(&mut buckets, second);
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].switches, 8);
    assert_eq!(buckets[0].up_bytes, 300_010);

    let mut many = Vec::new();
    for hour in 0..721 {
        let mut b = bucket();
        b.hour_start_ms = hour * 3_600_000;
        push_bucket(&mut many, b);
    }
    assert_eq!(many.len(), 720);
    assert_eq!(many[0].hour_start_ms, 3_600_000);
    assert_eq!(many[719].hour_start_ms, 720 * 3_600_000);
}

#[test]
fn a_switch_is_a_change_between_two_generations_and_a_repeat_is_not() {
    let mut tracker = GenerationTracker::new();
    assert_eq!(
        tracker.observe(Some(Generation::Nr)),
        None,
        "the first sighting is not a switch"
    );
    assert_eq!(
        tracker.observe(Some(Generation::Nr)),
        None,
        "a repeat is not a switch"
    );
    assert_eq!(
        tracker.observe(Some(Generation::Lte)),
        Some(Generation::Lte)
    );
    assert_eq!(
        tracker.observe(None),
        None,
        "moving to Wi-Fi is not a generation switch"
    );
    assert_eq!(
        tracker.observe(Some(Generation::Lte)),
        None,
        "the same generation again is not a switch"
    );
    assert_eq!(tracker.observe(Some(Generation::Nr)), Some(Generation::Nr));
}

#[test]
fn the_stability_thresholds_are_fixed() {
    assert_eq!(Stability::of(0), Stability::Steady);
    assert_eq!(Stability::of(1), Stability::Steady);
    assert_eq!(Stability::of(2), Stability::Changing);
    assert_eq!(Stability::of(3), Stability::Changing);
    assert_eq!(Stability::of(4), Stability::SwitchingOften);
    assert_eq!(Stability::of(40), Stability::SwitchingOften);
    assert_eq!(Stability::Steady.label(), "steady");
    assert_eq!(Stability::Changing.label(), "changing");
    assert_eq!(Stability::SwitchingOften.label(), "switching often");
}

#[test]
fn the_drain_uses_mah_when_it_is_sane_and_percent_otherwise() {
    assert_eq!(sane_mah(0), None);
    assert_eq!(sane_mah(-1), None);
    assert_eq!(sane_mah(i64::MIN), None);
    assert_eq!(sane_mah(240), Some(240));

    let mut b = bucket();
    b.drain_mah = None;
    b.drain_pct = Some(3);
    let lines = NetworkSummary::build(&b).lines();
    assert!(
        lines.iter().any(|l| l == "Battery drain this hour: 3%."),
        "{lines:?}"
    );
}

#[test]
fn a_charging_hour_has_no_drain_figure() {
    let mut b = bucket();
    b.charging = true;
    let lines = NetworkSummary::build(&b).lines();
    assert!(
        lines
            .iter()
            .any(|l| l == "Charging this hour - no drain figure."),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("Battery drain")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("More switching")),
        "{lines:?}"
    );
}

#[test]
fn the_report_lines_say_what_happened() {
    let lines = NetworkSummary::build(&bucket()).lines();
    assert!(
        lines
            .iter()
            .any(|l| l == "Time on 5G: 42 min. On 4G: 18 min."),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "Network switches this hour: 7"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "Speed this hour: 583 B/s down, 83 B/s up (what Jafnify shared)"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l == "Drops this hour: 2"), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l == "Network stability this hour: switching often."),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "Battery drain this hour: 240 mAh."),
        "{lines:?}"
    );
}

#[test]
fn the_correlation_line_never_claims_a_cause() {
    let lines = NetworkSummary::build(&bucket()).lines();
    let correlation = lines
        .iter()
        .find(|l| l.contains("switches, "))
        .expect("a correlation line");
    assert_eq!(
        *correlation,
        "More switching and a higher battery drain this hour: 7 switches, 240 mAh."
    );
    for line in &lines {
        assert!(!line.contains("caused"), "{line}");
        assert!(!line.contains("because"), "{line}");
    }
}

#[test]
fn an_empty_hour_says_not_enough_data_yet() {
    let mut b = bucket();
    b.samples = 0;
    b.switches = 0;
    b.generation_ms.clear();
    b.up_bytes = 0;
    b.down_bytes = 0;
    b.drops = 0;
    b.drain_mah = None;
    b.connected_ms = 0;
    let lines = NetworkSummary::build(&b).lines();
    assert_eq!(lines, vec!["Not enough data yet."]);
}

#[test]
fn two_parts_of_one_hour_add_their_drain() {
    let mut buckets = Vec::new();
    let mut first = bucket();
    first.drain_mah = Some(100);
    first.drain_pct = None;
    let mut second = bucket();
    second.drain_mah = Some(60);
    second.drain_pct = None;
    push_bucket(&mut buckets, first);
    push_bucket(&mut buckets, second);
    assert_eq!(buckets[0].drain_mah, Some(160));
}

#[test]
fn an_hour_without_a_reported_generation_claims_nothing_about_switching() {
    let line = "JFN1 1 hour=0 switches=0 gen=none up=0 down=0 drops=0 batt_mah=50 charging=0 connected_ms=0 samples=60";
    let bucket = NetworkBucket::parse_line(line).unwrap();
    let lines = NetworkSummary::build(&bucket).lines();
    assert!(
        lines.contains(&"Network type not reported this hour.".to_string()),
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("stability") || l.contains("switch")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"Battery drain this hour: 50 mAh.".to_string()),
        "{lines:?}"
    );
}
