//! The pure usage model: sampler rules, hourly buckets, summaries, and the
//! one-line bucket format. No clock, no disk, no socket.

use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::usage::{
    hour_start_ms, push_bucket, UsageBucket, UsageParseError, UsageSample, UsageSampler,
    UsageSummary, UsageWindow, DAY_MS, HOUR_MS, MAX_BUCKETS,
};

fn sample(at_ms: i64, up: u64, down: u64, connected: bool, network: NetworkKind) -> UsageSample {
    UsageSample {
        at_ms,
        up_bytes: up,
        down_bytes: down,
        connected,
        network,
    }
}

fn bucket_at(
    hour: i64,
    up: u64,
    down: u64,
    connected_ms: u64,
    network: NetworkKind,
) -> UsageBucket {
    UsageBucket {
        hour_start_ms: hour * HOUR_MS,
        up_bytes: up,
        down_bytes: down,
        min_up_bps: 0,
        min_down_bps: 0,
        peak_up_bps: 0,
        peak_down_bps: 0,
        connected_ms,
        samples: 1,
        network,
    }
}

#[test]
fn the_three_windows_are_one_seven_and_thirty_days() {
    assert_eq!(UsageWindow::ALL.map(|window| window.days()), [1, 7, 30]);
}

#[test]
fn the_hour_start_is_floored_to_the_utc_hour() {
    assert_eq!(hour_start_ms(0), 0);
    assert_eq!(hour_start_ms(HOUR_MS - 1), 0);
    assert_eq!(hour_start_ms(HOUR_MS), HOUR_MS);
    assert_eq!(hour_start_ms(-1), -HOUR_MS);
}

#[test]
fn the_first_sample_is_only_a_baseline() {
    let mut sampler = UsageSampler::new();
    assert!(sampler
        .step(sample(0, 1_000, 2_000, true, NetworkKind::Wifi))
        .is_none());
    let bucket = sampler.current().unwrap();
    assert_eq!(bucket.hour_start_ms, 0);
    assert_eq!(bucket.up_bytes, 0);
    assert_eq!(bucket.down_bytes, 0);
    assert_eq!(bucket.min_up_bps, 0);
    assert_eq!(bucket.min_down_bps, 0);
    assert_eq!(bucket.peak_up_bps, 0);
    assert_eq!(bucket.peak_down_bps, 0);
    assert_eq!(bucket.connected_ms, 0);
    assert_eq!(bucket.samples, 0);
    assert_eq!(bucket.network, NetworkKind::Wifi);
}

#[test]
fn a_connected_interval_counts_bytes_rates_and_time() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 1_000, 2_000, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (1_000, 2_000));
    assert_eq!((bucket.min_up_bps, bucket.min_down_bps), (1_000, 2_000));
    assert_eq!((bucket.peak_up_bps, bucket.peak_down_bps), (1_000, 2_000));
    assert_eq!(bucket.connected_ms, 1_000);
    assert_eq!(bucket.samples, 1);
}

#[test]
fn idle_connected_time_counts_but_lowest_stays_no_sample() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 0, 0, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (0, 0));
    assert_eq!((bucket.min_up_bps, bucket.min_down_bps), (0, 0));
    assert_eq!((bucket.peak_up_bps, bucket.peak_down_bps), (0, 0));
    assert_eq!(bucket.connected_ms, 1_000);
    assert_eq!(bucket.samples, 1);
}

#[test]
fn one_byte_is_a_positive_sample() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 1, 0, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!(bucket.up_bytes, 1);
    assert_eq!(bucket.min_up_bps, 1);
    assert_eq!(bucket.peak_up_bps, 1);
    assert_eq!(bucket.min_down_bps, 0);
}

#[test]
fn only_connected_intervals_count() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 1_000, 2_000, false, NetworkKind::Wifi));
    sampler.step(sample(2_000, 2_000, 4_000, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (1_000, 2_000));
    assert_eq!(bucket.connected_ms, 1_000);
    assert_eq!(bucket.samples, 1);
}

#[test]
fn a_counter_reset_starts_a_new_baseline_without_a_huge_rate() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 1_000, 2_000, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 10, 20, true, NetworkKind::Wifi));
    sampler.step(sample(2_000, 1_010, 2_020, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (1_000, 2_000));
    assert_eq!((bucket.min_up_bps, bucket.min_down_bps), (1_000, 2_000));
    assert_eq!((bucket.peak_up_bps, bucket.peak_down_bps), (1_000, 2_000));
    assert_eq!(bucket.connected_ms, 2_000);
    assert_eq!(bucket.samples, 2);
}

#[test]
fn a_backwards_clock_starts_a_new_baseline() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(1_000, 1_000, 2_000, true, NetworkKind::Wifi));
    sampler.step(sample(500, 2_000, 4_000, true, NetworkKind::Wifi));
    sampler.step(sample(1_500, 3_000, 6_000, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (1_000, 2_000));
    assert_eq!(bucket.connected_ms, 1_000);
    assert_eq!(bucket.samples, 1);
}

#[test]
fn a_same_timestamp_keeps_the_delta_for_the_next_step() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(0, 1_000, 2_000, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 1_000, 2_000, true, NetworkKind::Wifi));

    let bucket = sampler.current().unwrap();
    assert_eq!((bucket.up_bytes, bucket.down_bytes), (1_000, 2_000));
    assert_eq!(bucket.connected_ms, 1_000);
    assert_eq!(bucket.samples, 1);
}

#[test]
fn an_hour_roll_returns_the_previous_bucket() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1_000, 1_000, 2_000, true, NetworkKind::Wifi));

    let done = sampler.step(sample(HOUR_MS, 2_000, 4_000, true, NetworkKind::Wifi));
    let done = done.unwrap();
    assert_eq!(done.hour_start_ms, 0);
    assert_eq!((done.up_bytes, done.down_bytes), (1_000, 2_000));
    assert_eq!(done.connected_ms, 1_000);

    let current = sampler.current().unwrap();
    assert_eq!(current.hour_start_ms, HOUR_MS);
    assert_eq!((current.up_bytes, current.down_bytes), (1_000, 2_000));
    assert_eq!(current.connected_ms, HOUR_MS as u64 - 1_000);
}

#[test]
fn the_current_bucket_can_be_read_without_taking_it() {
    let mut sampler = UsageSampler::new();
    assert!(sampler.current().is_none());
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    assert!(sampler.current().is_some());
}

#[test]
fn line_round_trips() {
    let bucket = UsageBucket {
        hour_start_ms: 1_700_000_000_000,
        up_bytes: 123,
        down_bytes: 456,
        min_up_bps: 7,
        min_down_bps: 8,
        peak_up_bps: 9,
        peak_down_bps: 10,
        connected_ms: 11,
        samples: 12,
        network: NetworkKind::Mobile5g,
    };
    let line = bucket.line();
    assert!(line.starts_with("JFU1 1 "));
    assert_eq!(UsageBucket::parse_line(&line).unwrap(), bucket);
}

#[test]
fn an_old_line_without_min_fields_loads_zero() {
    let line = "JFU1 1 hour=1700000000000 up=123 down=456 peak_up=9 peak_down=10 connected_ms=11 samples=12 net=5g";
    let bucket = UsageBucket::parse_line(line).unwrap();
    assert_eq!(bucket.min_up_bps, 0);
    assert_eq!(bucket.min_down_bps, 0);
    assert_eq!(bucket.up_bytes, 123);
    assert_eq!(bucket.network, NetworkKind::Mobile5g);
}

#[test]
fn unknown_fields_are_ignored_and_never_round_trip() {
    let line = "JFU1 1 hour=0 up=1 down=2 min_up=3 min_down=4 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi name=Alice key=SGVsbG8= addr=1.2.3.4";
    let bucket = UsageBucket::parse_line(line).unwrap();
    assert_eq!(bucket.min_up_bps, 3);
    assert_eq!(bucket.min_down_bps, 4);
    let out = bucket.line();
    assert!(!out.contains("Alice"));
    assert!(!out.contains("key="));
    assert!(!out.contains("SGVsbG8"));
    assert!(!out.contains("1.2.3.4"));
}

#[test]
fn bad_prefix_version_and_values_are_rejected() {
    let valid = "hour=0 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi";
    assert_eq!(UsageBucket::parse_line(""), Err(UsageParseError::BadPrefix));
    assert_eq!(
        UsageBucket::parse_line(&format!("JFU2 1 {valid}")),
        Err(UsageParseError::BadPrefix)
    );
    assert_eq!(
        UsageBucket::parse_line(&format!("JFU1 2 {valid}")),
        Err(UsageParseError::BadVersion)
    );
    assert_eq!(
        UsageBucket::parse_line(
            "JFU1 1 hour=x up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi"
        ),
        Err(UsageParseError::Malformed)
    );
    assert_eq!(
        UsageBucket::parse_line(
            "JFU1 1 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi"
        ),
        Err(UsageParseError::Malformed)
    );
    assert_eq!(
        UsageBucket::parse_line("JFU1 1 hour=0 hour=1 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi"),
        Err(UsageParseError::Malformed)
    );
    assert_eq!(
        UsageBucket::parse_line("JFU1 1 hour=0 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi nope"),
        Err(UsageParseError::Malformed)
    );
    assert_eq!(
        UsageBucket::parse_line(
            "JFU1 1 hour=0 up=-1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi"
        ),
        Err(UsageParseError::Malformed)
    );
    assert_eq!(
        UsageBucket::parse_line("JFU1 1 hour=0 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=99999999999 net=wifi"),
        Err(UsageParseError::Malformed)
    );
}

#[test]
fn an_unknown_network_word_reads_as_unknown() {
    let line = "JFU1 1 hour=0 up=1 down=2 peak_up=5 peak_down=6 connected_ms=7 samples=8 net=foo";
    assert_eq!(
        UsageBucket::parse_line(line).unwrap().network,
        NetworkKind::Unknown
    );
}

#[test]
fn the_cap_drops_the_oldest_bucket() {
    let mut buckets = Vec::new();
    for hour in 0..=MAX_BUCKETS as i64 {
        push_bucket(
            &mut buckets,
            bucket_at(hour, 1, 1, 1_000, NetworkKind::Wifi),
        );
    }
    assert_eq!(buckets.len(), MAX_BUCKETS);
    assert_eq!(buckets[0].hour_start_ms, HOUR_MS);
    assert_eq!(
        buckets[MAX_BUCKETS - 1].hour_start_ms,
        MAX_BUCKETS as i64 * HOUR_MS
    );
}

#[test]
fn a_month_feed_has_a_known_total() {
    const MIN_MS: i64 = 60_000;
    const MINUTES: i64 = 30 * 24 * 60;

    let mut sampler = UsageSampler::new();
    let mut buckets = Vec::new();
    for minute in 0..MINUTES {
        let at = minute * MIN_MS;
        let up = minute as u64 * 60_000;
        let down = minute as u64 * 120_000;
        if let Some(done) = sampler.step(sample(at, up, down, true, NetworkKind::Wifi)) {
            push_bucket(&mut buckets, done);
        }
    }
    if let Some(current) = sampler.current() {
        push_bucket(&mut buckets, *current);
    }

    assert_eq!(buckets.len(), 720);
    assert_eq!(buckets[0].up_bytes, 59 * 60_000);
    assert_eq!(buckets[1].up_bytes, 60 * 60_000);

    let total_up: u64 = buckets.iter().map(|b| b.up_bytes).sum();
    let total_down: u64 = buckets.iter().map(|b| b.down_bytes).sum();
    assert_eq!(total_up, (MINUTES as u64 - 1) * 60_000);
    assert_eq!(total_down, (MINUTES as u64 - 1) * 120_000);

    let now = 29 * DAY_MS + 12 * HOUR_MS;
    let month = UsageSummary::build(&buckets, UsageWindow::ThirtyDays, now, 0);
    assert!(month.total_line().starts_with("Shared this month: "));
    assert_eq!(
        month.busiest_line(),
        "Busiest: 01:00-02:00, 02:00-03:00, 03:00-04:00"
    );
    assert_eq!(month.up_bytes, total_up);
    assert_eq!(month.down_bytes, total_down);
    assert_eq!(month.connected_ms, (MINUTES as u64 - 1) * MIN_MS as u64);
    assert_eq!(month.average_up_bps, 1_000);
    assert_eq!(month.average_down_bps, 2_000);
    assert_eq!(month.peak_up_bps, 1_000);
    assert_eq!(month.peak_down_bps, 2_000);
    assert_eq!(month.lowest_up_bps, 1_000);
    assert_eq!(month.lowest_down_bps, 2_000);
    assert_eq!(
        month.busiest.iter().map(|b| b.hour).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(month.network, Some(NetworkKind::Wifi));

    let week = UsageSummary::build(&buckets, UsageWindow::SevenDays, now, 0);
    assert_eq!(week.up_bytes, 7 * 24 * 60 * 60_000);
    assert_eq!(week.down_bytes, 7 * 24 * 60 * 120_000);
    assert_eq!(
        week.busiest.iter().map(|b| b.hour).collect::<Vec<_>>(),
        [0, 1, 2]
    );

    let today = UsageSummary::build(&buckets, UsageWindow::Today, now, 0);
    assert_eq!(today.up_bytes, 24 * 60 * 60_000);
    assert_eq!(today.down_bytes, 24 * 60 * 120_000);
}

#[test]
fn busy_hours_break_ties_by_earlier_hour() {
    let now = 20 * HOUR_MS + 30 * 60_000;
    let buckets = [
        bucket_at(5, 100, 0, 1_000, NetworkKind::Wifi),
        bucket_at(8, 200, 0, 1_000, NetworkKind::Wifi),
        bucket_at(20, 200, 0, 1_000, NetworkKind::Wifi),
        bucket_at(12, 300, 0, 1_000, NetworkKind::Wifi),
    ];
    let summary = UsageSummary::build(&buckets, UsageWindow::Today, now, 0);
    assert_eq!(
        summary.busiest.iter().map(|b| b.hour).collect::<Vec<_>>(),
        [12, 8, 20]
    );
}

#[test]
fn the_window_uses_the_passed_utc_offset() {
    let buckets = [
        bucket_at(23, 100, 0, 1_000, NetworkKind::Wifi),
        UsageBucket {
            hour_start_ms: DAY_MS,
            up_bytes: 200,
            down_bytes: 0,
            connected_ms: 1_000,
            network: NetworkKind::Wifi,
            ..Default::default()
        },
    ];
    let now = DAY_MS + 30 * 60_000;
    let utc = UsageSummary::build(&buckets, UsageWindow::Today, now, 0);
    assert_eq!(utc.up_bytes, 200);
    let plus_one = UsageSummary::build(&buckets, UsageWindow::Today, now, 60);
    assert_eq!(plus_one.up_bytes, 300);
    let minus_one = UsageSummary::build(&buckets, UsageWindow::Today, now, -60);
    assert_eq!(minus_one.up_bytes, 300);
}

#[test]
fn dominant_network_needs_a_clear_majority() {
    let buckets = [
        bucket_at(1, 1, 0, 60_000, NetworkKind::Wifi),
        bucket_at(2, 1, 0, 60_000, NetworkKind::Mobile5g),
    ];
    let split = UsageSummary::build(&buckets, UsageWindow::Today, 3 * HOUR_MS, 0);
    assert_eq!(split.network, None);
    assert_eq!(split.network_line(), "Network: mixed");

    let buckets = [
        bucket_at(1, 1, 0, 60_000, NetworkKind::Wifi),
        bucket_at(2, 1, 0, 20_000, NetworkKind::Mobile5g),
    ];
    let clear = UsageSummary::build(&buckets, UsageWindow::Today, 3 * HOUR_MS, 0);
    assert_eq!(clear.network, Some(NetworkKind::Wifi));
    assert_eq!(clear.network_line(), "Network: Wi-Fi");
}

#[test]
fn the_summary_wording_is_fixed() {
    let buckets = [UsageBucket {
        hour_start_ms: 20 * HOUR_MS,
        up_bytes: 1_000,
        down_bytes: 1_000,
        min_up_bps: 500,
        min_down_bps: 600,
        peak_up_bps: 2_000,
        peak_down_bps: 3_000,
        connected_ms: 1_000,
        samples: 1,
        network: NetworkKind::Wifi,
    }];
    let summary = UsageSummary::build(&buckets, UsageWindow::Today, 21 * HOUR_MS, 0);
    assert_eq!(summary.total_line(), "Shared today: 2.0 KB");
    assert_eq!(
        summary.average_line(),
        "Average: 1.0 KB/s down, 1.0 KB/s up"
    );
    assert_eq!(summary.peak_line(), "Peak: 3.0 KB/s down, 2.0 KB/s up");
    assert_eq!(summary.lowest_line(), "Lowest: 600 B/s down, 500 B/s up");
    assert_eq!(summary.busiest_line(), "Busiest: 20:00-21:00");
    assert_eq!(summary.network_line(), "Network: Wi-Fi");

    let down_only = UsageSummary::build(
        &[UsageBucket {
            hour_start_ms: 0,
            min_down_bps: 600,
            connected_ms: 1_000,
            network: NetworkKind::Wifi,
            ..Default::default()
        }],
        UsageWindow::Today,
        0,
        0,
    );
    assert_eq!(down_only.lowest_line(), "Lowest: 600 B/s down");

    let no_sample = UsageSummary::build(&[], UsageWindow::Today, 0, 0);
    assert!(no_sample.lines().contains(&"Lowest: no sample".to_string()));
    assert!(no_sample
        .lines()
        .contains(&"Busiest: no sample".to_string()));
    assert!(no_sample
        .lines()
        .contains(&"Network: no sample".to_string()));

    let week = UsageSummary::build(
        &[UsageBucket {
            hour_start_ms: 0,
            up_bytes: 1_200_000_000,
            down_bytes: 2_000_000_000,
            connected_ms: 1_000,
            network: NetworkKind::Wifi,
            ..Default::default()
        }],
        UsageWindow::SevenDays,
        0,
        0,
    );
    assert_eq!(week.total_line(), "Shared this week: 3.2 GB");
}

#[test]
fn no_overflow_at_u64_extremes() {
    let mut sampler = UsageSampler::new();
    sampler.step(sample(0, 0, 0, true, NetworkKind::Wifi));
    sampler.step(sample(1, u64::MAX, u64::MAX, true, NetworkKind::Wifi));
    let bucket = sampler.current().unwrap();
    assert_eq!(bucket.up_bytes, u64::MAX);
    assert_eq!(bucket.down_bytes, u64::MAX);
    assert_eq!(bucket.min_up_bps, u64::MAX);
    assert_eq!(bucket.peak_up_bps, u64::MAX);

    sampler.step(sample(
        2,
        u64::MAX - 10,
        u64::MAX - 10,
        true,
        NetworkKind::Wifi,
    ));
    sampler.step(sample(3, u64::MAX, u64::MAX, true, NetworkKind::Wifi));
    let bucket = sampler.current().unwrap();
    assert_eq!(bucket.up_bytes, u64::MAX);
    assert_eq!(bucket.down_bytes, u64::MAX);
    assert_eq!(bucket.peak_up_bps, u64::MAX);
    assert_eq!(bucket.min_up_bps, 10_000);

    let buckets = [
        bucket_at(0, u64::MAX, u64::MAX, u64::MAX, NetworkKind::Wifi),
        bucket_at(1, u64::MAX, u64::MAX, u64::MAX, NetworkKind::Wifi),
    ];
    let summary = UsageSummary::build(&buckets, UsageWindow::Today, 2 * HOUR_MS, 0);
    assert_eq!(summary.up_bytes, u64::MAX);
    assert_eq!(summary.down_bytes, u64::MAX);
    assert_eq!(summary.connected_ms, u64::MAX);
}

#[test]
fn seeded_feed_totals_match_the_sum_of_positive_deltas() {
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state
    };

    let mut sampler = UsageSampler::new();
    let mut buckets = Vec::new();
    let mut at = 0_i64;
    let mut up = 0_u64;
    let mut down = 0_u64;
    let mut prev_up = 0_u64;
    let mut prev_down = 0_u64;
    let mut have_prev = false;
    let mut expected_up = 0_u64;
    let mut expected_down = 0_u64;

    for _ in 0..5_000 {
        let r = next();
        at += 1 + (r % 3_000) as i64;
        if r % 17 == 0 {
            up = 1 + next() % 1_000;
            down = 1 + next() % 1_000;
        } else {
            up = up.saturating_add(next() % 5_000);
            down = down.saturating_add(next() % 5_000);
        }
        let connected = r % 5 != 0;
        let network = match r % 3 {
            0 => NetworkKind::Wifi,
            1 => NetworkKind::Mobile5g,
            _ => NetworkKind::Unknown,
        };
        if have_prev && connected && up >= prev_up && down >= prev_down {
            expected_up = expected_up.saturating_add(up - prev_up);
            expected_down = expected_down.saturating_add(down - prev_down);
        }
        prev_up = up;
        prev_down = down;
        have_prev = true;
        if let Some(done) = sampler.step(sample(at, up, down, connected, network)) {
            buckets.push(done);
        }
    }
    if let Some(current) = sampler.current() {
        buckets.push(*current);
    }

    let total_up = buckets
        .iter()
        .fold(0_u64, |total, b| total.saturating_add(b.up_bytes));
    let total_down = buckets
        .iter()
        .fold(0_u64, |total, b| total.saturating_add(b.down_bytes));
    assert_eq!(total_up, expected_up);
    assert_eq!(total_down, expected_down);
    for bucket in &buckets {
        assert!(bucket.min_up_bps == 0 || bucket.min_up_bps <= bucket.peak_up_bps);
        assert!(bucket.min_down_bps == 0 || bucket.min_down_bps <= bucket.peak_down_bps);
    }
}

#[test]
fn a_second_bucket_for_the_same_hour_is_merged_not_appended() {
    use jafnify_core::usage::{push_bucket, UsageBucket, HOUR_MS};
    let hour = 10 * HOUR_MS;
    let first = UsageBucket {
        hour_start_ms: hour,
        up_bytes: 100,
        down_bytes: 1_000,
        min_up_bps: 5,
        min_down_bps: 0,
        peak_up_bps: 50,
        peak_down_bps: 500,
        connected_ms: 60_000,
        samples: 60,
        ..Default::default()
    };
    let second = UsageBucket {
        hour_start_ms: hour,
        up_bytes: 10,
        down_bytes: 2_000,
        min_up_bps: 9,
        min_down_bps: 7,
        peak_up_bps: 20,
        peak_down_bps: 900,
        connected_ms: 30_000,
        samples: 30,
        ..Default::default()
    };
    let mut buckets = Vec::new();
    push_bucket(&mut buckets, first);
    push_bucket(&mut buckets, second);
    assert_eq!(buckets.len(), 1);
    let b = buckets[0];
    assert_eq!((b.up_bytes, b.down_bytes), (110, 3_000));
    assert_eq!((b.min_up_bps, b.min_down_bps), (5, 7));
    assert_eq!((b.peak_up_bps, b.peak_down_bps), (50, 900));
    assert_eq!((b.connected_ms, b.samples), (90_000, 90));
    push_bucket(
        &mut buckets,
        UsageBucket {
            hour_start_ms: hour + HOUR_MS,
            ..Default::default()
        },
    );
    assert_eq!(buckets.len(), 2);
}
