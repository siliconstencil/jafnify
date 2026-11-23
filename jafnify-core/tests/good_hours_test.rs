//! The good hours rules: the hourly record, the local hour summary, the
//! quick and busy verdicts and the lines the laptop shows.

use jafnify_core::good_hours::{
    hot_hour_evidence, verdict_for, HourRecord, Sample, Sampler, Summary, Verdict,
};
use jafnify_core::phone_stats::NetworkKind;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// 2026-01-01T00:00:00Z.
const BASE: i64 = 1_767_225_600_000;
/// The "now" of the tests: day 20, hour 12.
const NOW: i64 = BASE + 20 * DAY + 12 * HOUR;
/// The last three days before NOW.
const DAYS: [i64; 3] = [18, 19, 20];

fn sample(at_ms: i64, dns: Option<u32>) -> Sample {
    Sample {
        at_ms,
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        dbm: Some(-80),
        dns_ms: dns,
        temp_c_x10: None,
        thr: None,
    }
}

fn record(day: i64, hour: u8, dns: Option<u32>) -> HourRecord {
    HourRecord {
        hour_start_ms: BASE + day * DAY + hour as i64 * HOUR,
        samples: 1,
        dns_ms: dns,
        five_g_pct: 100,
        signal: Some(4),
        temp_c_x10: None,
        thr: None,
        partial: false,
    }
}

/// An hour record with a temperature of its own.
fn heat_record(at_ms: i64, temp_c_x10: i16, dns: u32) -> HourRecord {
    HourRecord {
        hour_start_ms: at_ms.div_euclid(HOUR) * HOUR,
        samples: 1,
        dns_ms: Some(dns),
        five_g_pct: 100,
        signal: Some(4),
        temp_c_x10: Some(temp_c_x10),
        thr: None,
        partial: false,
    }
}

/// A sample of the phone while it is on Wi-Fi: the time belongs to the
/// Wi-Fi, not to the mobile network.
fn wifi_sample(at_ms: i64, dns: Option<u32>) -> Sample {
    Sample {
        at_ms,
        net: NetworkKind::Wifi,
        signal: Some(4),
        dbm: Some(-40),
        dns_ms: dns,
        temp_c_x10: None,
        thr: None,
    }
}

/// Every hour of every day with the same DNS time, except the hours that get
/// their own value.
fn hours(days: &[i64], special: &[(u8, u32)]) -> Vec<HourRecord> {
    let mut out = Vec::new();
    for day in days {
        for hour in 0..24u8 {
            let dns = special
                .iter()
                .find(|(h, _)| *h == hour)
                .map(|(_, value)| *value)
                .unwrap_or(100);
            out.push(record(*day, hour, Some(dns)));
        }
    }
    out
}

#[test]
fn the_median_of_an_odd_number_of_dns_samples_is_the_middle_one() {
    let mut sampler = Sampler::new();
    for dns in [30u32, 10, 20] {
        sampler.push(sample(BASE + 1_000, Some(dns)));
    }
    assert_eq!(sampler.current().unwrap().dns_ms, Some(20));
}

#[test]
fn the_median_of_an_even_number_of_dns_samples_is_the_mean_of_the_middle_two() {
    let mut sampler = Sampler::new();
    for dns in [10u32, 20, 30, 40] {
        sampler.push(sample(BASE + 1_000, Some(dns)));
    }
    assert_eq!(sampler.current().unwrap().dns_ms, Some(25));
}

#[test]
fn the_sampler_finishes_an_hour_when_the_next_sample_belongs_to_the_next_hour() {
    let mut sampler = Sampler::new();
    assert!(sampler.push(sample(BASE + 10_000, Some(30))).is_none());
    let finished = sampler
        .push(sample(BASE + HOUR + 10_000, Some(40)))
        .expect("the first hour is finished");
    assert_eq!(finished.hour_start_ms, BASE);
    assert_eq!(finished.dns_ms, Some(30));
}

#[test]
fn the_current_hour_is_kept_in_memory_until_the_hour_ends() {
    let mut sampler = Sampler::new();
    sampler.push(sample(BASE + 10_000, Some(30)));
    let current = sampler.current().expect("the hour is in memory");
    assert_eq!(current.hour_start_ms, BASE);
    assert_eq!(current.samples, 1);
    assert_eq!(sampler.take().expect("the hour ends").hour_start_ms, BASE);
    assert!(sampler.current().is_none());
}

#[test]
fn the_record_counts_the_samples_the_five_g_share_and_the_signal_median() {
    let mut sampler = Sampler::new();
    sampler.push(Sample {
        at_ms: BASE,
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        dbm: Some(-80),
        dns_ms: Some(30),
        temp_c_x10: None,
        thr: None,
    });
    sampler.push(Sample {
        at_ms: BASE + 1_000,
        net: NetworkKind::Mobile4g,
        signal: Some(2),
        dbm: None,
        dns_ms: Some(40),
        temp_c_x10: None,
        thr: None,
    });
    let current = sampler.current().unwrap();
    assert_eq!(current.samples, 2);
    assert_eq!(current.five_g_pct, 50);
    assert_eq!(current.signal, Some(3));
}

#[test]
fn only_the_samples_with_a_dns_answer_time_count_for_the_speed() {
    let mut sampler = Sampler::new();
    sampler.push(sample(BASE, Some(30)));
    sampler.push(sample(BASE + 1_000, None));
    let current = sampler.current().unwrap();
    assert_eq!(current.samples, 2);
    assert_eq!(current.dns_ms, Some(30));
}

#[test]
fn a_line_round_trips_through_the_parser() {
    let record = HourRecord {
        hour_start_ms: BASE,
        samples: 7,
        dns_ms: Some(28),
        five_g_pct: 80,
        signal: Some(3),
        temp_c_x10: None,
        thr: None,
        partial: false,
    };
    let line = record.line();
    assert_eq!(line, format!("JFG1 1 hour={} n=7 rtt=28 g5=80 sig=3", BASE));
    assert_eq!(HourRecord::parse_line(&line).unwrap(), record);
}

#[test]
fn a_record_without_a_dns_time_does_not_write_the_field() {
    let record = HourRecord {
        hour_start_ms: BASE,
        samples: 2,
        dns_ms: None,
        five_g_pct: 0,
        signal: None,
        temp_c_x10: None,
        thr: None,
        partial: false,
    };
    assert_eq!(record.line(), format!("JFG1 1 hour={} n=2 g5=0", BASE));
    assert_eq!(HourRecord::parse_line(&record.line()).unwrap(), record);
}

#[test]
fn the_parser_ignores_an_unknown_key() {
    let line = format!("JFG1 1 hour={} n=3 rtt=28 g5=80 sig=3 future=7", BASE);
    let parsed = HourRecord::parse_line(&line).unwrap();
    assert_eq!(parsed.dns_ms, Some(28));
    assert_eq!(parsed.samples, 3);
    assert_eq!(parsed.five_g_pct, 80);
    assert_eq!(parsed.signal, Some(3));
}

#[test]
fn the_parser_rejects_a_bad_prefix_and_a_bad_version() {
    assert!(HourRecord::parse_line(&format!("XFG1 1 hour={} n=3", BASE)).is_err());
    assert!(HourRecord::parse_line(&format!("JFG1 2 hour={} n=3", BASE)).is_err());
    assert!(HourRecord::parse_line("JFG1 1 n=3").is_err());
}

#[test]
fn the_summary_covers_the_last_fourteen_local_days() {
    let mut records = hours(&DAYS, &[]);
    records.push(record(6, 12, Some(100)));
    let summary = Summary::build(&records, NOW, 0);
    assert_eq!(summary.days, 3);
    let hour = summary.hours.iter().find(|h| h.hour == 12).unwrap();
    assert_eq!(hour.days, 3);
}

#[test]
fn a_local_hour_needs_three_distinct_days_before_it_gets_a_verdict() {
    let summary = Summary::build(&hours(&[19, 20], &[]), NOW, 0);
    assert!(summary.range_lines().is_empty());
    assert!(summary.now_line(NOW, 0).is_none());
    let hour = summary.hours.iter().find(|h| h.hour == 12).unwrap();
    assert_eq!(hour.days, 2);
}

#[test]
fn a_quick_hour_is_at_most_eighty_percent_of_the_overall_median() {
    assert_eq!(verdict_for(80, 100), Verdict::Quick);
    assert_eq!(verdict_for(81, 100), Verdict::Neither);
}

#[test]
fn a_busy_hour_is_at_least_a_hundred_and_twenty_five_percent_of_the_overall_median() {
    assert_eq!(verdict_for(125, 100), Verdict::Busy);
    assert_eq!(verdict_for(124, 100), Verdict::Neither);
}

#[test]
fn the_hours_between_the_two_edges_are_neither_quick_nor_busy() {
    let summary = Summary::build(&hours(&DAYS, &[(5, 50), (6, 85), (7, 125)]), NOW, 0);
    assert_eq!(summary.baseline_ms, 100);
    assert_eq!(
        summary.range_lines(),
        vec![
            "Usually quick: 05:00 (DNS about 50 ms)".to_string(),
            "Usually busy: 07:00 (DNS about 125 ms)".to_string(),
        ]
    );
}

#[test]
fn neighbouring_hours_with_the_same_verdict_merge_into_one_range() {
    let summary = Summary::build(
        &hours(
            &DAYS,
            &[(2, 50), (3, 50), (4, 50), (19, 150), (20, 150), (21, 150)],
        ),
        NOW,
        0,
    );
    let lines = summary.range_lines();
    assert_eq!(lines[0], "Usually quick: 02:00-04:00 (DNS about 50 ms)");
    assert_eq!(lines[1], "Usually busy: 19:00-21:00 (DNS about 150 ms)");
}

#[test]
fn a_range_wraps_past_midnight() {
    let summary = Summary::build(&hours(&DAYS, &[(23, 50), (0, 50), (1, 50)]), NOW, 0);
    assert_eq!(
        summary.range_lines()[0],
        "Usually quick: 23:00-01:00 (DNS about 50 ms)"
    );
}

#[test]
fn at_most_two_good_and_two_busy_ranges_are_shown_and_the_longest_comes_first() {
    let summary = Summary::build(
        &hours(
            &DAYS,
            &[
                (1, 50),
                (2, 50),
                (5, 50),
                (9, 50),
                (10, 50),
                (11, 50),
                (13, 150),
                (17, 150),
                (18, 150),
                (21, 150),
                (22, 150),
                (23, 150),
            ],
        ),
        NOW,
        0,
    );
    let lines = summary.range_lines();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0], "Usually quick: 09:00-11:00 (DNS about 50 ms)");
    assert_eq!(lines[1], "Usually quick: 01:00-02:00 (DNS about 50 ms)");
    assert_eq!(lines[2], "Usually busy: 21:00-23:00 (DNS about 150 ms)");
    assert_eq!(lines[3], "Usually busy: 17:00-18:00 (DNS about 150 ms)");
}

#[test]
fn a_day_with_a_time_change_still_counts_as_one_day() {
    // The laptop moves to summer time: the local clock jumps one hour
    // forward, so the last hour of a UTC day belongs to the next local day.
    // The days are still days.
    let records = hours(&[17, 18, 19], &[(23, 50)]);
    let winter = Summary::build(&records, NOW, 0);
    assert_eq!(winter.days, 3);
    assert_eq!(winter.hours.iter().find(|h| h.hour == 23).unwrap().days, 3);
    let summer = Summary::build(&records, NOW, 60);
    assert_eq!(summer.hours.iter().find(|h| h.hour == 0).unwrap().days, 3);
    // the window holds one local day more than the number of UTC days
    assert_eq!(summer.days, 4);
}

#[test]
fn the_learning_line_counts_the_days_so_far() {
    let summary = Summary::build(&hours(&[19, 20], &[]), NOW, 0);
    assert_eq!(
        summary.learning_line().unwrap(),
        "Learning the busy hours: 2 of 3 days so far"
    );
    let filled = Summary::build(&hours(&DAYS, &[]), NOW, 0);
    assert!(filled.learning_line().is_none());
}

#[test]
fn the_now_line_is_shown_for_a_quick_or_a_busy_hour_and_left_out_otherwise() {
    let quick = Summary::build(&hours(&DAYS, &[(12, 50)]), NOW, 0);
    assert_eq!(
        quick.now_line(NOW, 0).unwrap(),
        "Now is usually a quick hour"
    );
    let busy = Summary::build(&hours(&DAYS, &[(12, 150)]), NOW, 0);
    assert_eq!(busy.now_line(NOW, 0).unwrap(), "Now is usually a busy hour");
    let neither = Summary::build(&hours(&DAYS, &[]), NOW, 0);
    assert!(neither.now_line(NOW, 0).is_none());
}

#[test]
fn the_empty_summary_shows_only_the_learning_line() {
    let summary = Summary::build(&[], NOW, 0);
    assert_eq!(
        summary.learning_line().unwrap(),
        "Learning the busy hours: 0 of 3 days so far"
    );
    assert!(summary.range_lines().is_empty());
    assert!(summary.now_line(NOW, 0).is_none());
    assert_eq!(summary.lines(NOW, 0).len(), 1);
}

#[test]
fn the_section_holds_the_learning_line_the_ranges_and_the_now_line() {
    let summary = Summary::build(&hours(&DAYS, &[(12, 50), (19, 150)]), NOW, 0);
    let lines = summary.lines(NOW, 0);
    assert_eq!(lines[0], "Usually quick: 12:00 (DNS about 50 ms)");
    assert_eq!(lines[1], "Usually busy: 19:00 (DNS about 150 ms)");
    assert_eq!(lines[2], "Now is usually a quick hour");
}

#[test]
fn a_wifi_sample_does_not_count_for_the_speed() {
    let mut sampler = Sampler::new();
    sampler.push(wifi_sample(BASE + 1_000, Some(10)));
    assert_eq!(
        sampler.current().unwrap().dns_ms,
        None,
        "the Wi-Fi time says nothing about the mobile network"
    );
    sampler.push(sample(BASE + 2_000, Some(40)));
    assert_eq!(sampler.current().unwrap().dns_ms, Some(40));
}

#[test]
fn a_half_hour_is_not_in_the_summary() {
    let mut half = record(20, 9, Some(10));
    half.partial = true;
    let records = vec![record(18, 9, Some(40)), record(19, 9, Some(60)), half];
    let summary = Summary::build(&records, NOW, 0);
    assert_eq!(summary.hours.len(), 1, "{:?}", summary.hours);
    assert_eq!(summary.hours[0].dns_ms, 50, "the half hour is not counted");
    assert_eq!(summary.hours[0].days, 2, "a half hour is not a day");
}

#[test]
fn a_half_hour_never_gives_a_day() {
    let records: Vec<HourRecord> = (18..=20)
        .map(|day| {
            let mut half = record(day, 9, Some(40));
            half.partial = true;
            half
        })
        .collect();
    let summary = Summary::build(&records, NOW, 0);
    assert!(summary.hours.is_empty(), "{:?}", summary.hours);
    assert!(summary.range_lines().is_empty());
    assert_eq!(
        summary.learning_line().unwrap(),
        "Learning the busy hours: 0 of 3 days so far"
    );
}

#[test]
fn the_learning_line_stays_while_no_hour_has_its_three_days() {
    // Three days, three different hours: no hour has three days yet.
    let records = vec![
        record(18, 3, Some(40)),
        record(19, 7, Some(40)),
        record(20, 21, Some(40)),
    ];
    let summary = Summary::build(&records, NOW, 0);
    assert_eq!(
        summary.learning_line().unwrap(),
        "Learning the busy hours: 1 of 3 days so far"
    );
    assert!(summary.range_lines().is_empty());
}

#[test]
fn the_learning_line_counts_the_closest_hour() {
    // Two hours of the same days, one of them with three days already.
    let mut records = hours(&[18, 19], &[(9, 40)]);
    records.push(record(20, 9, Some(40)));
    records.push(record(20, 21, Some(40)));
    let summary = Summary::build(&records, NOW, 0);
    assert_eq!(
        summary.learning_line().unwrap(),
        "Learning the busy hours: 2 of 3 days so far"
    );
}

#[test]
fn a_half_hour_line_carries_the_part_key_and_round_trips() {
    let mut half = record(18, 9, Some(40));
    half.partial = true;
    let line = half.line();
    assert!(line.contains("part=1"), "{line}");
    assert_eq!(HourRecord::parse_line(&line).unwrap(), half);
}

#[test]
fn a_finished_hour_line_has_no_part_key() {
    let line = record(18, 9, Some(40)).line();
    assert!(!line.contains("part"), "{line}");
    assert!(!HourRecord::parse_line(&line).unwrap().partial);
}

#[test]
fn the_sampler_marks_the_hour_in_the_making_as_a_half_hour() {
    let mut sampler = Sampler::new();
    sampler.push(sample(BASE + 10_000, Some(30)));
    assert!(sampler.current().unwrap().partial, "the hour is not over");
    let finished = sampler
        .push(sample(BASE + HOUR + 10_000, Some(30)))
        .unwrap();
    assert!(!finished.partial, "the hour is over");
}

#[test]
fn the_hour_record_carries_the_heat_of_the_hour() {
    let mut record = heat_record(BASE + 18 * DAY + 3 * HOUR, 430, 120);
    record.thr = Some(3);
    let line = record.line();
    assert!(line.contains("tmp=430"), "{line}");
    assert!(line.contains("thr=3"), "{line}");
    let back = HourRecord::parse_line(&line).expect("the line parses");
    assert_eq!(back.temp_c_x10, Some(430));
    assert_eq!(back.thr, Some(3));
}

#[test]
fn an_old_hour_record_without_the_heat_still_loads() {
    let line = "JFG1 1 hour=1767225600000 n=1 rtt=30 g5=100 sig=4";
    let record = HourRecord::parse_line(line).expect("the old line parses");
    assert_eq!(record.temp_c_x10, None);
    assert_eq!(record.thr, None);
}

#[test]
fn the_sampler_keeps_the_median_temperature_and_the_highest_thermal_status() {
    let mut sampler = Sampler::new();
    sampler.push(Sample {
        at_ms: BASE,
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        dbm: None,
        dns_ms: Some(30),
        temp_c_x10: Some(430),
        thr: Some(2),
    });
    sampler.push(Sample {
        at_ms: BASE + 1_000,
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        dbm: None,
        dns_ms: Some(40),
        temp_c_x10: Some(410),
        thr: Some(3),
    });
    let current = sampler.current().expect("the hour is in memory");
    assert_eq!(current.temp_c_x10, Some(420));
    assert_eq!(current.thr, Some(3));
}

/// Three hot hours on two days and three other hours.
fn heat_records() -> Vec<HourRecord> {
    vec![
        heat_record(BASE + 18 * DAY + 3 * HOUR, 430, 200),
        heat_record(BASE + 18 * DAY + 4 * HOUR, 435, 210),
        heat_record(BASE + 19 * DAY + 3 * HOUR, 425, 190),
        heat_record(BASE + 18 * DAY + 5 * HOUR, 300, 100),
        heat_record(BASE + 19 * DAY + 5 * HOUR, 310, 110),
        heat_record(BASE + 20 * DAY + 5 * HOUR, 320, 120),
    ]
}

#[test]
fn the_hot_hours_are_compared_with_the_other_hours() {
    let line = hot_hour_evidence(&heat_records(), NOW, 0).expect("the line has enough data");
    assert!(
        line.starts_with("When the phone was 42 C or more"),
        "{line}"
    );
    assert!(line.contains("usually 1.8x slower"), "{line}");
    assert!(line.contains("(3 hot hours)"), "{line}");
}

#[test]
fn the_hot_hours_need_three_hours_on_two_days() {
    let mut records = heat_records();
    records.retain(|record| {
        record.hour_start_ms != BASE + 19 * DAY + 3 * HOUR
            && record.hour_start_ms != BASE + 18 * DAY + 4 * HOUR
    });
    assert!(hot_hour_evidence(&records, NOW, 0).is_none());
}

#[test]
fn the_hot_hours_need_two_days() {
    let records = vec![
        heat_record(BASE + 18 * DAY + 3 * HOUR, 430, 200),
        heat_record(BASE + 18 * DAY + 4 * HOUR, 435, 210),
        heat_record(BASE + 18 * DAY + 6 * HOUR, 425, 190),
        heat_record(BASE + 18 * DAY + 5 * HOUR, 300, 100),
        heat_record(BASE + 19 * DAY + 5 * HOUR, 310, 110),
        heat_record(BASE + 20 * DAY + 5 * HOUR, 320, 120),
    ];
    assert!(hot_hour_evidence(&records, NOW, 0).is_none());
}

#[test]
fn the_hot_hours_need_three_other_hours() {
    let mut records = heat_records();
    records.retain(|record| record.temp_c_x10 != Some(320));
    assert!(hot_hour_evidence(&records, NOW, 0).is_none());
}

#[test]
fn the_hours_without_a_temperature_do_not_count_as_other_hours() {
    let mut records = heat_records();
    records.retain(|record| record.temp_c_x10 != Some(320));
    records.push(record(20, 6, Some(130)));
    assert!(hot_hour_evidence(&records, NOW, 0).is_none());
}

#[test]
fn the_evidence_says_the_speed_is_the_same_when_it_is() {
    let mut records = heat_records();
    for record in &mut records {
        record.dns_ms = Some(100);
    }
    let line = hot_hour_evidence(&records, NOW, 0).expect("the line has enough data");
    assert!(line.contains("about the same"), "{line}");
}

#[test]
fn the_evidence_says_faster_when_the_hot_hours_are_faster() {
    let mut records = heat_records();
    for record in &mut records {
        record.dns_ms = Some(
            if record.temp_c_x10 == Some(300)
                || record.temp_c_x10 == Some(310)
                || record.temp_c_x10 == Some(320)
            {
                200
            } else {
                100
            },
        );
    }
    let line = hot_hour_evidence(&records, NOW, 0).expect("the line has enough data");
    assert!(line.contains("usually 2.0x faster"), "{line}");
}
