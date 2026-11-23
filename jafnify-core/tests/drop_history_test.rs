use jafnify_core::drop_history::{
    events, is_away, pair, reason_word, summary, DropParseError, DropRecord, Entry, EventKind,
    PhoneContext, AWAY_AFTER, AWAY_LABEL, NOT_A_DROP, NOT_BACK, NO_PHONE_DATA, STOPPED_LABEL,
};
use jafnify_core::phone_exit::ExitReason;
use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::wifi_leg::WifiLeg;

fn phone() -> PhoneContext {
    PhoneContext {
        net: Some(NetworkKind::Mobile5gAdv),
        signal: Some(1),
        temp_c_x10: Some(395),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    }
}

/// A phone that reports nothing about the heat.
fn cool_phone() -> PhoneContext {
    PhoneContext {
        net: Some(NetworkKind::Mobile5g),
        signal: Some(3),
        temp_c_x10: Some(300),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    }
}

fn record() -> DropRecord {
    DropRecord {
        at_ms: 1_712_326_800_000,
        reason: "peer-left".to_string(),
        phone: Some(phone()),
        wifi: Some(WifiLeg::new(42, 3, 30)),
        down_ms: Some(12_000),
        exit: None,
        link: Default::default(),
    }
}

#[test]
fn a_drop_line_round_trips() {
    let line = record().drop_line();
    let parsed = match pair(&[line]).into_iter().next() {
        Some(entry) => entry,
        None => panic!("the line must parse"),
    };
    assert_eq!(parsed.at_ms, 1_712_326_800_000);
    assert_eq!(parsed.reason, "peer-left");
    assert_eq!(parsed.phone, Some(phone()));
    assert_eq!(parsed.wifi, Some(WifiLeg::new(42, 3, 30)));
    assert_eq!(parsed.down_ms, None);
}

#[test]
fn a_back_line_round_trips() {
    let line = DropRecord::back_line(1_712_345_690_000, 12_000);
    match jafnify_core::drop_history::parse_line(&line) {
        Ok(Entry::Back { at_ms, down_ms }) => {
            assert_eq!(at_ms, 1_712_345_690_000);
            assert_eq!(down_ms, 12_000);
        }
        other => panic!("a back line must parse as a back: {other:?}"),
    }
}

#[test]
fn a_line_without_the_prefix_is_rejected() {
    assert_eq!(
        jafnify_core::drop_history::parse_line("JFZ1 1 drop at=1").unwrap_err(),
        DropParseError::BadPrefix
    );
}

#[test]
fn a_line_with_an_unknown_version_is_rejected() {
    assert_eq!(
        jafnify_core::drop_history::parse_line("JFD1 9 drop at=1").unwrap_err(),
        DropParseError::BadVersion
    );
}

#[test]
fn a_malformed_line_is_rejected() {
    assert_eq!(
        jafnify_core::drop_history::parse_line("JFD1 1 drop at=abc").unwrap_err(),
        DropParseError::Malformed
    );
}

#[test]
fn the_core_pairs_a_drop_with_its_back() {
    let entries = pair(&[
        record().drop_line(),
        DropRecord::back_line(1_712_345_690_000, 12_000),
    ]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].down_ms, Some(12_000));
}

#[test]
fn a_drop_without_a_back_stays_open() {
    let entries = pair(&[record().drop_line()]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].down_ms, None);
}

#[test]
fn the_summary_counts_the_last_day_only() {
    let now = 1_712_345_678_901;
    let mut old = record();
    old.at_ms = now - 2 * 86_400_000;
    let mut fresh = record();
    fresh.at_ms = now - 60_000;
    fresh.down_ms = None;
    assert_eq!(
        summary(&[old, fresh], now),
        "Drops in the last 24 h: 1 (longest 0 s)"
    );
}

#[test]
fn the_summary_names_the_longest_down_time() {
    let now = 1_712_345_678_901;
    let mut long = record();
    long.down_ms = Some(95_000);
    assert_eq!(
        summary(&[long], now),
        "Drops in the last 24 h: 1 (longest 95 s)"
    );
}

#[test]
fn a_weak_signal_is_a_likely_cause() {
    assert_eq!(record().hint(), "likely weak signal");
}

#[test]
fn a_slow_mobile_network_is_a_likely_cause() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        net: Some(NetworkKind::Mobile3g),
        signal: Some(3),
        temp_c_x10: Some(300),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    });
    assert_eq!(drop.hint(), "likely slow mobile network");
}

#[test]
fn a_missing_mobile_network_is_a_likely_cause() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        net: Some(NetworkKind::None),
        signal: Some(3),
        temp_c_x10: Some(300),
        battery: Some(80),
        rtt_ms: None,
        thr: None,
    });
    assert_eq!(drop.hint(), "likely no mobile network");
}

#[test]
fn a_hot_phone_is_a_likely_cause() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        net: Some(NetworkKind::Mobile5g),
        signal: Some(3),
        temp_c_x10: Some(420),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    });
    assert_eq!(drop.hint(), "likely hot phone");
}

#[test]
fn a_slow_wifi_leg_is_a_likely_cause() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        net: Some(NetworkKind::Mobile5g),
        signal: Some(3),
        temp_c_x10: Some(300),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    });
    drop.wifi = Some(WifiLeg::new(80, 0, 30));
    assert_eq!(drop.hint(), "likely slow Wi-Fi");
}

#[test]
fn a_good_context_has_no_clear_cause() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        net: Some(NetworkKind::Mobile5g),
        signal: Some(3),
        temp_c_x10: Some(300),
        battery: Some(80),
        rtt_ms: Some(45),
        thr: None,
    });
    drop.wifi = Some(WifiLeg::new(20, 0, 30));
    assert_eq!(drop.hint(), "no clear cause");
}

#[test]
fn a_drop_without_a_network_blames_the_hotspot_of_the_phone() {
    let mut drop = record();
    drop.reason = "no-network".to_string();
    // The Wi-Fi leg of that moment was slow, but the laptop lost its way to
    // the phone: that is the hotspot of the phone going away.
    drop.wifi = Some(WifiLeg::new(120, 4, 30));
    assert_eq!(drop.hint(), "likely the phone's hotspot went away");
}

#[test]
fn a_hot_phone_is_only_blamed_when_android_says_so() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        thr: Some(2),
        temp_c_x10: Some(430),
        ..cool_phone()
    });
    drop.wifi = Some(WifiLeg::new(20, 0, 30));
    assert_eq!(drop.hint(), "no clear cause");
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        thr: Some(3),
        temp_c_x10: Some(430),
        ..cool_phone()
    });
    drop.wifi = Some(WifiLeg::new(20, 0, 30));
    assert_eq!(drop.hint(), "likely hot phone");
}

#[test]
fn a_phone_without_the_thermal_status_keeps_the_temperature_rule() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        temp_c_x10: Some(430),
        ..cool_phone()
    });
    drop.wifi = Some(WifiLeg::new(20, 0, 30));
    assert_eq!(drop.hint(), "likely hot phone");
}

#[test]
fn the_thermal_status_travels_in_the_drop_line() {
    let mut drop = record();
    drop.phone = Some(PhoneContext {
        thr: Some(3),
        ..cool_phone()
    });
    let line = drop.drop_line();
    assert!(line.contains(" thr=3"), "{line}");
    let parsed = match jafnify_core::drop_history::parse_line(&line) {
        Ok(Entry::Drop(record)) => record,
        other => panic!("the line must parse as a drop: {other:?}"),
    };
    let phone = parsed.phone.expect("the phone context travels");
    assert_eq!(phone.thr, Some(3));
}

#[test]
fn an_old_drop_line_without_the_thermal_status_still_loads() {
    let line = "JFD1 1 drop at=1700000000000 reason=no-data net=5g sig=3 temp=300 batt=80 rtt=40";
    let parsed = match jafnify_core::drop_history::parse_line(line) {
        Ok(Entry::Drop(record)) => record,
        other => panic!("the old line must parse as a drop: {other:?}"),
    };
    let phone = parsed.phone.expect("the phone context travels");
    assert_eq!(phone.thr, None);
}

#[test]
fn the_detail_line_reads_like_the_example() {
    assert_eq!(
        record().detail(0),
        "14:20  down 12 s  5G (advanced), signal 1/4, 39.5 C - likely weak signal"
    );
}

#[test]
fn the_detail_line_says_phone_data_none_without_a_summary() {
    let mut drop = record();
    drop.phone = None;
    drop.wifi = None;
    assert_eq!(
        drop.detail(0),
        "14:20  down 12 s  phone data: none - no clear cause"
    );
    assert!(drop.detail(0).contains(NO_PHONE_DATA));
}

#[test]
fn the_detail_line_says_not_back_before_restart() {
    let mut drop = record();
    drop.down_ms = None;
    assert!(
        drop.detail(0)
            .contains("14:20  down, not back before restart"),
        "{}",
        drop.detail(0)
    );
    assert!(drop.detail(0).contains(NOT_BACK));
}

#[test]
fn the_detail_line_uses_the_local_clock() {
    // One hour ahead of UTC: the local clock reads 15:20.
    assert!(record().detail(60).starts_with("15:20"));
}

#[test]
fn the_reason_word_never_leaks_the_free_text() {
    assert_eq!(reason_word("peer disconnected"), "peer-left");
    assert_eq!(reason_word("no data from peer for 35 s"), "no-data");
    assert_eq!(reason_word("no network"), "no-network");
    assert_eq!(reason_word("session reset"), "reset");
    assert_eq!(reason_word("something else entirely"), "unknown");
}

#[test]
fn a_session_reset_with_a_data_stall_is_a_reset() {
    // A live line: the reset is the cause, the data
    // stall the symptom. It must not read as a plain no-data drop, or the
    // hint could blame the Wi-Fi.
    assert_eq!(
        reason_word("peer session reset; no data for 8 s"),
        "reset"
    );
}

#[test]
fn a_network_that_cannot_be_reached_reads_as_no_network() {
    assert_eq!(
        reason_word("known-peer socket error: Network is unreachable (os error 101)"),
        "no-network"
    );
    assert_eq!(reason_word("Network is unreachable"), "no-network");
}

/// The moment the phone went away, in epoch milliseconds.
const AWAY_AT: i64 = 1_712_326_800_000;

/// The lines of an absence: the drop that opened it, the away that
/// reclassified it and the back that closed it. The absence lasts 1:14:00.
fn away_lines() -> Vec<String> {
    vec![
        "JFD1 1 drop at=1712326800000 reason=no-network".to_string(),
        DropRecord::away_line(AWAY_AT),
        DropRecord::back_line(AWAY_AT + 4_440_000, 4_440_000),
    ]
}

/// The lines of a stop on the phone: the stopped line and the back.
fn stopped_lines() -> Vec<String> {
    vec![
        DropRecord::stopped_line(AWAY_AT),
        DropRecord::back_line(AWAY_AT + 12_000, 12_000),
    ]
}

#[test]
fn an_away_line_round_trips() {
    let line = DropRecord::away_line(AWAY_AT);
    assert_eq!(line, "JFD1 1 away at=1712326800000");
    match jafnify_core::drop_history::parse_line(&line) {
        Ok(Entry::Away { at_ms }) => assert_eq!(at_ms, AWAY_AT),
        other => panic!("the line must parse as an away: {other:?}"),
    }
}

#[test]
fn a_stopped_line_round_trips() {
    let line = DropRecord::stopped_line(AWAY_AT);
    assert_eq!(line, "JFD1 1 stopped at=1712326800000");
    match jafnify_core::drop_history::parse_line(&line) {
        Ok(Entry::Stopped { at_ms }) => assert_eq!(at_ms, AWAY_AT),
        other => panic!("the line must parse as a stopped: {other:?}"),
    }
}

#[test]
fn an_absence_below_the_edge_stays_a_drop() {
    let shorter = AWAY_AFTER.as_millis() as u64 - 1;
    assert!(!is_away(shorter), "an absence of {shorter} ms is too short");
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-network".to_string(),
        DropRecord::back_line(AWAY_AT + shorter as i64, shorter),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), EventKind::Drop);
    assert_eq!(pair(&lines).len(), 1, "a short loss is a real drop");
}

#[test]
fn an_absence_at_the_edge_is_an_away() {
    let edge = AWAY_AFTER.as_millis() as u64;
    assert!(is_away(edge), "an absence of {edge} ms is an absence");
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-network".to_string(),
        DropRecord::away_line(AWAY_AT),
        DropRecord::back_line(AWAY_AT + edge as i64, edge),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), EventKind::Away);
}

#[test]
fn an_absence_above_the_edge_is_an_away() {
    let longer = AWAY_AFTER.as_millis() as u64 + 1;
    assert!(is_away(longer), "an absence of {longer} ms is an absence");
}

#[test]
fn a_short_loss_keeps_the_hotspot_hint() {
    let mut drop = record();
    drop.reason = reason_word("no network").to_string();
    assert_eq!(drop.hint(), "likely the phone's hotspot went away");
}

#[test]
fn the_away_replaces_the_drop_it_reclassifies() {
    let lines = away_lines();
    let events = events(&lines);
    assert_eq!(events.len(), 1, "one outage is one record");
    assert_eq!(events[0].kind(), EventKind::Away);
    assert_eq!(
        pair(&lines).len(),
        0,
        "a reclassified outage is not a drop any more"
    );
}

#[test]
fn the_away_keeps_the_start_of_the_outage() {
    let events = events(&away_lines());
    assert_eq!(events[0].at_ms(), AWAY_AT);
}

#[test]
fn the_away_is_not_counted_as_a_drop() {
    let now = AWAY_AT + 4_440_000;
    assert_eq!(
        summary(&pair(&away_lines()), now),
        "Drops in the last 24 h: 0 (longest 0 s)"
    );
}

#[test]
fn the_away_reads_phone_away() {
    let events = events(&away_lines());
    assert_eq!(events[0].detail(0), "14:20  Phone away 1:14:00  not a drop");
    assert!(events[0].detail(0).contains(AWAY_LABEL));
    assert!(events[0].detail(0).contains(NOT_A_DROP));
}

#[test]
fn a_bye_is_a_stopped_event_and_not_a_drop() {
    let lines = stopped_lines();
    let events = events(&lines);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), EventKind::Stopped);
    assert_eq!(pair(&lines).len(), 0, "a stop on the phone is not a drop");
    assert_eq!(
        summary(&pair(&lines), AWAY_AT + 12_000),
        "Drops in the last 24 h: 0 (longest 0 s)"
    );
}

#[test]
fn the_stopped_reads_stopped_on_the_phone() {
    let events = events(&stopped_lines());
    assert_eq!(
        events[0].detail(0),
        "14:20  Stopped on the phone  not a drop"
    );
    assert!(events[0].detail(0).contains(STOPPED_LABEL));
}

#[test]
fn a_loss_without_a_bye_stays_a_drop() {
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-data".to_string(),
        DropRecord::back_line(AWAY_AT + 10_516, 10_516),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), EventKind::Drop);
    assert_eq!(pair(&lines).len(), 1, "a loss without a signal is a drop");
}

#[test]
fn the_drop_line_takes_an_unknown_key() {
    let line = "JFD1 1 drop at=1712326800000 reason=no-data future=42";
    match jafnify_core::drop_history::parse_line(line) {
        Ok(Entry::Drop(record)) => assert_eq!(record.reason, "no-data"),
        other => panic!("the line must parse as a drop: {other:?}"),
    }
}

#[test]
fn the_away_line_takes_an_unknown_key() {
    let line = "JFD1 1 away at=1712326800000 future=42";
    match jafnify_core::drop_history::parse_line(line) {
        Ok(Entry::Away { at_ms }) => assert_eq!(at_ms, AWAY_AT),
        other => panic!("the line must parse as an away: {other:?}"),
    }
}

#[test]
fn the_stopped_line_takes_an_unknown_key() {
    let line = "JFD1 1 stopped at=1712326800000 future=42";
    match jafnify_core::drop_history::parse_line(line) {
        Ok(Entry::Stopped { at_ms }) => assert_eq!(at_ms, AWAY_AT),
        other => panic!("the line must parse as a stopped: {other:?}"),
    }
}

#[test]
fn an_away_without_a_drop_stands_alone() {
    let lines = vec![
        DropRecord::away_line(AWAY_AT),
        DropRecord::back_line(AWAY_AT + 4_440_000, 4_440_000),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), EventKind::Away);
    assert_eq!(events[0].down_ms(), Some(4_440_000));
}

#[test]
fn an_away_that_never_came_back_stays_open() {
    let lines = vec![DropRecord::away_line(AWAY_AT)];
    let events = events(&lines);
    assert_eq!(events[0].down_ms(), None);
    assert!(events[0].detail(0).contains("not back yet"));
}

#[test]
fn a_stopped_line_does_not_swallow_an_older_drop() {
    // A daemon that restarts in the middle of an outage leaves an open drop
    // in the file. The phone says goodbye later: the older fault stays a
    // fault, it is not relabelled as a stop on the phone.
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-data".to_string(),
        DropRecord::stopped_line(AWAY_AT + 60_000),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Drop);
    assert_eq!(events[1].kind(), EventKind::Stopped);
    assert_eq!(pair(&lines).len(), 1, "the older fault is still a drop");
}

#[test]
fn a_second_away_line_is_the_same_away() {
    let lines = vec![
        DropRecord::away_line(AWAY_AT),
        DropRecord::away_line(AWAY_AT),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Away);
}

#[test]
fn the_drop_line_holds_no_endpoint_and_no_peer_id() {
    let mut drop = record();
    drop.reason = reason_word("peer 10.233.0.9:9797 left").to_string();
    let line = drop.drop_line();
    assert!(!line.contains("10.233.0.9"));
    assert!(!line.contains("9797"));
    assert!(line.starts_with("JFD1 1 drop at="));
    // The format has no field for an address, a key or a peer id.
    let keys: Vec<&str> = line
        .split_whitespace()
        .filter_map(|part| part.split_once('=').map(|(key, _)| key))
        .collect();
    assert!(
        keys.iter().all(|key| {
            matches!(
                *key,
                "at" | "reason"
                    | "net"
                    | "sig"
                    | "temp"
                    | "batt"
                    | "rtt"
                    | "thr"
                    | "wifi_ms"
                    | "wifi_loss"
                    | "wifi_n"
            )
        }),
        "{keys:?}"
    );
}

#[test]
fn an_update_after_the_back_line_reclassifies_its_own_outage() {
    // The phone can only say why it ended after the reconnect: the neutral
    // line arrives after the back line and still finds its own outage.
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=reset".to_string(),
        DropRecord::back_line(1712326803000, 3_000),
        DropRecord::updated_line(1712326800000),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Updated);
    assert_eq!(events[0].down_ms(), Some(3_000), "the back line survives");
    assert!(!events[0].is_drop());
    assert!(pair(&lines).is_empty(), "an update is not counted as a drop");
    assert!(events[0]
        .detail(0)
        .ends_with("Phone app updated  not a drop"));
}

#[test]
fn a_stop_can_also_arrive_after_the_back_line() {
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-data".to_string(),
        DropRecord::back_line(1712326810000, 10_000),
        DropRecord::stopped_line(1712326800000),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Stopped);
    assert_eq!(events[0].down_ms(), Some(10_000));
    assert!(pair(&lines).is_empty());
}

#[test]
fn an_update_never_reclassifies_an_older_fault() {
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=no-data".to_string(),
        DropRecord::back_line(1712326810000, 10_000),
        DropRecord::updated_line(1712326900000),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Drop);
    assert_eq!(events[1].kind(), EventKind::Updated);
    assert_eq!(pair(&lines).len(), 1);
}

#[test]
fn a_crash_keeps_the_drop_and_says_why() {
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=reset".to_string(),
        DropRecord::back_line(1712326803000, 3_000),
        DropRecord::exit_line(1712326800000, ExitReason::Crash),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].is_drop(), "a crash is a real fault");
    let records = pair(&lines);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hint(), "likely the phone app stopped");
    assert_eq!(records[0].exit, Some(ExitReason::Crash));
}

#[test]
fn a_system_kill_keeps_the_drop_and_says_android_closed_it() {
    for word in [ExitReason::LowMemory, ExitReason::Killed] {
        let lines = vec![
            "JFD1 1 drop at=1712326800000 reason=no-data".to_string(),
            DropRecord::exit_line(1712326800000, word),
        ];
        let records = pair(&lines);
        assert_eq!(records.len(), 1, "a system kill is still a drop");
        assert_eq!(records[0].hint(), "likely Android closed the phone app");
    }
}

#[test]
fn an_exit_annotation_never_changes_a_neutral_event() {
    let lines = vec![
        "JFD1 1 drop at=1712326800000 reason=reset".to_string(),
        DropRecord::updated_line(1712326800000),
        DropRecord::exit_line(1712326800000, ExitReason::Crash),
    ];
    let events = events(&lines);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind(), EventKind::Updated);
}

#[test]
fn an_unknown_exit_word_is_tolerated_and_hides_nothing() {
    let line = "JFD1 1 exit at=1712326800000 word=teleported";
    let parsed = jafnify_core::drop_history::parse_line(line);
    assert!(parsed.is_ok(), "{parsed:?}");
    let mut lines = vec!["JFD1 1 drop at=1712326800000 reason=no-data".to_string()];
    lines.push(line.to_string());
    let records = pair(&lines);
    assert_eq!(records.len(), 1, "an unknown word never hides a drop");
    assert_eq!(records[0].hint(), "no clear cause");
}

#[test]
fn a_session_reset_drop_is_never_blamed_on_the_wifi() {
    let mut drop = record();
    drop.reason = "reset".to_string();
    assert_eq!(drop.hint(), "likely the phone app restarted");
    // Even a slow Wi-Fi leg does not turn a reset into a Wi-Fi problem.
    drop.wifi = Some(WifiLeg::new(500, 40, 30));
    assert_eq!(drop.hint(), "likely the phone app restarted");
}

#[test]
fn an_empty_exit_line_is_malformed() {
    assert_eq!(
        jafnify_core::drop_history::parse_line("JFD1 1 exit at=1712326800000"),
        Err(DropParseError::Malformed)
    );
    assert_eq!(
        jafnify_core::drop_history::parse_line("JFD1 1 exit word=crash"),
        Err(DropParseError::Malformed)
    );
}

