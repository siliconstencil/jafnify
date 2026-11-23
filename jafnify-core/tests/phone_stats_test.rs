use jafnify_core::phone_stats::{
    display_lines, main_lines, parse_request, request_history, request_summary, FrameError,
    NetworkKind, PhoneStats, PhoneStatsRequest, HEADING, SUMMARY_MAX, UNAVAILABLE_LINE,
};

fn full() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Mobile5g,
        signal: Some(3),
        rtt_ms: Some(45),
        up_bps: 123_456,
        down_bps: 654_321,
        battery: Some(80),
        temp_c_x10: Some(425),
        used_bytes: 123_456_789,
        allow_bytes: 10_737_418_240,
        reset_day: 12,
        month: 24323,
        switches: None,
        dbm: None,
        thr: None,
        hr30: None,
        wg_loss_pct: Some(3),
        wg_rtt_ms: Some(41),
        caps_link: false,
        caps_net: false,
        sconn_ms: None,
    }
}

#[test]
fn the_two_requests_parse_and_accept_a_trailing_newline() {
    assert_eq!(
        parse_request(request_summary()).unwrap(),
        PhoneStatsRequest::Summary
    );
    assert_eq!(
        parse_request(request_history()).unwrap(),
        PhoneStatsRequest::History
    );
    assert_eq!(
        parse_request(b"JFS1 1 summary\n").unwrap(),
        PhoneStatsRequest::Summary
    );
    assert_eq!(
        parse_request(b"JFS1 1 history\r\n").unwrap(),
        PhoneStatsRequest::History
    );
}

#[test]
fn bad_requests_are_rejected_by_kind() {
    assert_eq!(parse_request(b"JFS2 1 summary"), Err(FrameError::BadMagic));
    assert_eq!(
        parse_request(b"JFS1 2 summary"),
        Err(FrameError::BadVersion)
    );
    assert_eq!(parse_request(b"JFS1 1 stats"), Err(FrameError::Malformed));
    assert_eq!(parse_request(b""), Err(FrameError::Malformed));
    assert_eq!(
        parse_request(b"JFS1 1 summary extra"),
        Err(FrameError::Malformed)
    );
    assert_eq!(parse_request(&[b'J'; 65]), Err(FrameError::TooLarge));
}

#[test]
fn a_full_summary_round_trips() {
    let stats = full();
    let text = stats.to_response();
    assert!(text.len() <= SUMMARY_MAX, "{}", text.len());
    assert_eq!(PhoneStats::parse_response(text.as_bytes()).unwrap(), stats);
}

#[test]
fn missing_fields_are_unknown_and_unknown_keys_are_ignored() {
    let stats = PhoneStats::parse_response(b"JFS1 1 summary v=1").unwrap();
    assert_eq!(stats, PhoneStats::default());
    assert_eq!(stats.net, NetworkKind::Unknown);

    let stats = PhoneStats::parse_response(
        b"JFS1 1 summary v=1 net=wifi name=Alice key=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA addr=203.0.113.5 sig=3",
    )
    .unwrap();
    assert_eq!(stats.net, NetworkKind::Wifi);
    assert_eq!(stats.signal, Some(3));
    let debug = format!("{stats:?}");
    let wire = stats.to_response();
    let shown = display_lines(Some(&stats), Some(0), false, true).join("\n");
    for needle in ["Alice", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "203.0.113.5"] {
        assert!(!debug.contains(needle), "{debug}");
        assert!(!wire.contains(needle), "{wire}");
        assert!(!shown.contains(needle), "{shown}");
    }
}

#[test]
fn a_known_field_cannot_smuggle_text_or_a_key_shape() {
    let stats =
        PhoneStats::parse_response(b"JFS1 1 summary net=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(stats.net, NetworkKind::Unknown);
    assert!(!stats.to_response().contains("AAAA"));

    let stats = PhoneStats::parse_response(b"JFS1 1 summary net=203.0.113.5").unwrap();
    assert_eq!(stats.net, NetworkKind::Unknown);
    assert!(!stats.to_response().contains("203.0.113.5"));
}

#[test]
fn malformed_values_and_duplicates_are_rejected() {
    for text in [
        "JFS1 1 summary net=wifi net=5g",
        "JFS1 1 summary v=1 v=1",
        "JFS1 1 summary v=2",
        "JFS1 1 summary sig=5",
        "JFS1 1 summary sig=abc",
        "JFS1 1 summary rtt=abc",
        "JFS1 1 summary batt=101",
        "JFS1 1 summary temp=abc",
        "JFS1 1 summary reset=32",
        "JFS1 1 summary up=-1",
        "JFS1 1 summary broken",
    ] {
        assert!(
            PhoneStats::parse_response(text.as_bytes()).is_err(),
            "{text}"
        );
    }
    assert_eq!(
        PhoneStats::parse_response(b"JFS2 1 summary"),
        Err(FrameError::BadMagic)
    );
    assert_eq!(
        PhoneStats::parse_response(b"JFS1 2 summary"),
        Err(FrameError::BadVersion)
    );
    assert_eq!(
        PhoneStats::parse_response(b"JFS1 1 history n=0"),
        Err(FrameError::Unsupported)
    );
    assert_eq!(
        PhoneStats::parse_response(&[b'J'; SUMMARY_MAX + 1]),
        Err(FrameError::TooLarge)
    );
}

#[test]
fn sentinels_parse_and_format() {
    let stats = PhoneStats::parse_response(
        b"JFS1 1 summary v=1 net=unknown sig=255 rtt=0 batt=255 temp=-32768",
    )
    .unwrap();
    assert_eq!(stats.signal, None);
    assert_eq!(stats.rtt_ms, None);
    assert_eq!(stats.battery, None);
    assert_eq!(stats.temp_c_x10, None);
    assert!(stats.to_response().contains("sig=255"));
    assert!(stats.to_response().contains("temp=-32768"));
}

#[test]
fn the_display_lines_are_fixed_and_read_only() {
    let stats = full();
    assert_eq!(
        display_lines(Some(&stats), Some(0), false, true),
        [
            "Network: 5G",
            "Signal: 3 of 4",
            "DNS answer time: 45 ms",
            "Phone battery: 80%.",
            "Phone is hot: 42.5 C.",
            "Speed: 654.3 KB/s down, 123.5 KB/s up",
            "Mobile data: 0.1 GB of 10.7 GB this month.",
        ]
    );
    assert_eq!(HEADING, "Phone measurements");
}

#[test]
fn dns_answer_time_is_opt_in_and_says_when_it_has_no_sample() {
    let stats = full();
    let on = display_lines(Some(&stats), Some(0), false, true).join("\n");
    assert!(on.contains("DNS answer time: 45 ms"), "{on}");
    assert!(!on.to_lowercase().contains("ping"), "{on}");
    assert!(!on.to_lowercase().contains("internet speed"), "{on}");

    let mut unknown = full();
    unknown.rtt_ms = None;
    let unknown = display_lines(Some(&unknown), Some(0), false, true).join("\n");
    assert!(
        unknown.contains("DNS answer time: not enough data"),
        "{unknown}"
    );

    let off = display_lines(Some(&stats), Some(0), false, false).join("\n");
    assert!(!off.contains("DNS answer time"), "{off}");
}

#[test]
fn stale_and_unavailable_lines() {
    assert_eq!(
        display_lines(None, None, false, false),
        Vec::<String>::new()
    );
    assert_eq!(
        display_lines(None, None, true, false),
        [UNAVAILABLE_LINE.to_string()]
    );
    let shown = display_lines(Some(&full()), Some(16), false, false);
    assert_eq!(shown.last().unwrap(), "Phone measurements are stale.");
    assert!(!display_lines(Some(&full()), Some(15), false, false)
        .iter()
        .any(|line| line.contains("stale")));
}

#[test]
fn network_kind_words_are_stable() {
    for (word, kind, shown) in [
        ("none", NetworkKind::None, "No network"),
        ("wifi", NetworkKind::Wifi, "Wi-Fi"),
        ("eth", NetworkKind::Ethernet, "Ethernet"),
        ("2g", NetworkKind::Mobile2g, "2G"),
        ("3g", NetworkKind::Mobile3g, "3G"),
        ("4g", NetworkKind::Mobile4g, "4G"),
        ("5g", NetworkKind::Mobile5g, "5G"),
        ("mobile", NetworkKind::Mobile, "Mobile network"),
        ("other", NetworkKind::Other, "Other"),
        ("unknown", NetworkKind::Unknown, "Unknown"),
    ] {
        assert_eq!(NetworkKind::parse(word), kind, "{word}");
        assert_eq!(kind.as_str(), word);
        assert_eq!(kind.display(), shown);
    }
    assert_eq!(NetworkKind::parse("cellular"), NetworkKind::Mobile);
    assert_eq!(NetworkKind::parse("5g cellular"), NetworkKind::Unknown);
    assert_eq!(NetworkKind::default(), NetworkKind::Unknown);
}

#[test]
fn the_switch_count_is_optional_and_round_trips() {
    let line = "JFS1 1 summary v=1 net=5g-nsa sig=3 rtt=45 up=10 down=20 batt=80 temp=315 used=0 allow=0 reset=1 month=24323 switch=7";
    let stats = PhoneStats::parse_response(line.as_bytes()).unwrap();
    assert_eq!(stats.switches, Some(7));
    assert_eq!(stats.to_response(), line);

    let old = "JFS1 1 summary v=1 net=4g sig=255 rtt=0 up=0 down=0 batt=255 temp=-32768 used=0 allow=0 reset=0 month=0";
    let stats = PhoneStats::parse_response(old.as_bytes()).unwrap();
    assert_eq!(stats.switches, None);
    assert_eq!(stats.to_response(), old);
}

#[test]
fn the_main_screen_lines_carry_the_signal_switches_and_generation() {
    let stats = PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(3),
        switches: Some(4),
        ..full()
    };
    let lines = main_lines(&stats, true, true);
    assert!(lines.iter().any(|line| line == "Network: 5G (advanced)"));
    assert!(lines.iter().any(|line| line == "Signal: 3 of 4"));
    assert!(lines
        .iter()
        .any(|line| line == "Network switches this hour: 4"));
    assert!(lines
        .iter()
        .any(|line| line.starts_with("DNS answer time:")));
}

#[test]
fn the_phone_section_hides_the_heat_line_without_the_battery_switch() {
    let stats = PhoneStats {
        net: NetworkKind::Mobile5gAdv,
        signal: Some(4),
        temp_c_x10: Some(431),
        ..full()
    };
    let lines = main_lines(&stats, true, true);
    assert!(
        lines.iter().any(|line| line == "Phone is hot (43.1 C)"),
        "the battery switch is on"
    );
    let lines = main_lines(&stats, true, false);
    assert!(
        !lines.iter().any(|line| line.starts_with("Phone is hot (")),
        "the battery switch is off: no heat line"
    );
}

#[test]
fn the_dbm_key_is_optional_and_reads_back() {
    let without = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3").unwrap();
    assert_eq!(without.dbm, None);
    assert!(!without.to_response().contains("dbm="));

    let with = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3 dbm=-97").unwrap();
    assert_eq!(with.dbm, Some(-97));
    assert!(with.to_response().contains("dbm=-97"));
}

#[test]
fn a_dbm_outside_the_range_is_malformed() {
    for text in [
        "JFS1 1 summary dbm=1",
        "JFS1 1 summary dbm=-201",
        "JFS1 1 summary dbm=abc",
        "JFS1 1 summary dbm=",
    ] {
        assert!(
            PhoneStats::parse_response(text.as_bytes()).is_err(),
            "{text}"
        );
    }
    assert_eq!(
        PhoneStats::parse_response(b"JFS1 1 summary dbm=0")
            .unwrap()
            .dbm,
        Some(0)
    );
    assert_eq!(
        PhoneStats::parse_response(b"JFS1 1 summary dbm=-200")
            .unwrap()
            .dbm,
        Some(-200)
    );
}

#[test]
fn a_0_2_2_parser_that_skips_the_new_key_still_reads_the_line() {
    let stats = PhoneStats {
        dbm: Some(-97),
        ..full()
    };
    let line = stats.to_response();
    assert!(line.contains("dbm=-97"), "{line}");

    // A 0.2.2 build knows the old keys only and skips the new one.
    let mut seen: Vec<(String, String)> = Vec::new();
    for token in line.split_whitespace().skip(3) {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        if key == "dbm" {
            continue;
        }
        seen.push((key.to_string(), value.to_string()));
    }
    for (key, value) in [
        ("net", "5g"),
        ("sig", "3"),
        ("rtt", "45"),
        ("batt", "80"),
        ("used", "123456789"),
    ] {
        assert!(
            seen.iter().any(|(k, v)| k == key && v == value),
            "{key}={value} is missing from {line}"
        );
    }
}

#[test]
fn the_longest_summary_line_stays_under_the_limit() {
    let stats = PhoneStats {
        net: NetworkKind::Mobile5gMmwave,
        signal: Some(4),
        rtt_ms: Some(u32::MAX),
        up_bps: u64::MAX,
        down_bps: u64::MAX,
        battery: Some(100),
        temp_c_x10: Some(i16::MAX),
        used_bytes: u64::MAX,
        allow_bytes: u64::MAX,
        reset_day: 31,
        month: u32::MAX,
        switches: Some(u32::MAX),
        dbm: Some(-200),
        thr: Some(6),
        hr30: Some(200),
        wg_loss_pct: Some(100),
        wg_rtt_ms: Some(u32::MAX),
        caps_link: true,
        caps_net: true,
        sconn_ms: Some(u64::MAX),
    };
    let line = stats.to_response();
    assert!(line.len() <= SUMMARY_MAX, "{}: {line}", line.len());
    assert_eq!(PhoneStats::parse_response(line.as_bytes()).unwrap(), stats);
}

#[test]
fn the_thermal_status_is_an_optional_key() {
    let without = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3").unwrap();
    assert_eq!(without.thr, None);
    assert!(!without.to_response().contains("thr="));

    let with = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3 thr=3").unwrap();
    assert_eq!(with.thr, Some(3));
    assert!(with.to_response().contains("thr=3"));
}

#[test]
fn a_thermal_status_outside_the_range_is_malformed() {
    for text in [
        "JFS1 1 summary thr=7",
        "JFS1 1 summary thr=abc",
        "JFS1 1 summary thr=",
    ] {
        assert!(
            PhoneStats::parse_response(text.as_bytes()).is_err(),
            "{text}"
        );
    }
    assert_eq!(
        PhoneStats::parse_response(b"JFS1 1 summary thr=6")
            .unwrap()
            .thr,
        Some(6)
    );
}

#[test]
fn the_thermal_headroom_forecast_is_an_optional_key() {
    // An older phone that does not know the key reads fine, and its own line
    // carries no `hr30=`.
    let without = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3").unwrap();
    assert_eq!(without.hr30, None);
    assert!(!without.to_response().contains("hr30="));

    let with = PhoneStats::parse_response(b"JFS1 1 summary v=1 net=5g sig=3 hr30=105").unwrap();
    assert_eq!(with.hr30, Some(105));
    assert!(with.to_response().contains("hr30=105"));
    // The forecast survives the round trip the responder makes.
    assert_eq!(
        PhoneStats::parse_response(with.to_response().as_bytes()).unwrap(),
        with
    );
}

#[test]
fn a_headroom_forecast_outside_the_range_is_malformed() {
    for text in [
        "JFS1 1 summary hr30=201",
        "JFS1 1 summary hr30=abc",
        "JFS1 1 summary hr30=",
    ] {
        assert!(
            PhoneStats::parse_response(text.as_bytes()).is_err(),
            "{text}"
        );
    }
}

#[test]
fn a_0_2_2_parser_that_skips_the_thermal_status_still_reads_the_line() {
    let stats = PhoneStats {
        thr: Some(3),
        ..full()
    };
    let line = stats.to_response();
    assert!(line.contains("thr=3"), "{line}");

    // A 0.2.2 build knows the old keys only and skips the new one.
    let mut seen: Vec<(String, String)> = Vec::new();
    for token in line.split_whitespace().skip(3) {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        if key == "thr" {
            continue;
        }
        seen.push((key.to_string(), value.to_string()));
    }
    for (key, value) in [("net", "5g"), ("sig", "3"), ("rtt", "45"), ("batt", "80")] {
        assert!(
            seen.iter().any(|(k, v)| k == key && v == value),
            "{key}={value} is missing from {line}"
        );
    }
}

#[test]
fn the_wireguard_loss_and_round_trip_travel_and_old_laptops_ignore_them() {
    let line = full().to_response();
    assert!(line.ends_with(" loss=3 wgrtt=41"), "{line}");
    let back = PhoneStats::parse_response(line.as_bytes()).unwrap();
    assert_eq!(back.wg_loss_pct, Some(3));
    assert_eq!(back.wg_rtt_ms, Some(41));
    // An older phone sends neither: nothing is made up.
    let old = PhoneStats {
        wg_loss_pct: None,
        wg_rtt_ms: None,
        ..full()
    };
    let back = PhoneStats::parse_response(old.to_response().as_bytes()).unwrap();
    assert_eq!(back.wg_loss_pct, None);
    // A loss above 100 % is not a loss.
    let bad = line.replace("loss=3", "loss=101");
    assert!(PhoneStats::parse_response(bad.as_bytes()).is_err());
}

#[test]
fn a_phone_that_takes_the_link_report_says_so_and_the_request_carries_it() {
    use jafnify_core::phone_stats::{request_range, request_summary_with_range};
    use jafnify_core::wifi_range::RangeReport;
    let mut stats = full();
    stats.caps_link = true;
    let line = stats.to_response();
    assert!(line.ends_with(" caps=link"), "{line}");
    assert!(
        PhoneStats::parse_response(line.as_bytes())
            .unwrap()
            .caps_link
    );
    // A future capability list still names the link.
    let more = line.replace("caps=link", "caps=foo,link");
    assert!(
        PhoneStats::parse_response(more.as_bytes())
            .unwrap()
            .caps_link
    );
    assert!(
        !PhoneStats::parse_response(full().to_response().as_bytes())
            .unwrap()
            .caps_link
    );

    let report = RangeReport {
        level_dbm: -71,
        rate_100k: Some(1300),
        missed_beacons: true,
    };
    let request = request_summary_with_range(&report);
    assert_eq!(request, "JFS1 1 summary lvl=-71 rate=1300 mb=1");
    assert_eq!(
        parse_request(request.as_bytes()),
        Ok(PhoneStatsRequest::Summary)
    );
    assert_eq!(request_range(request.as_bytes()), Some(report));
    assert_eq!(request_range(request_summary()), None);
    // Only key=value words, and only after a summary request.
    assert!(parse_request(b"JFS1 1 summary lvl").is_err());
    assert!(parse_request(b"JFS1 1 history lvl=-50").is_err());
    let longest = request_summary_with_range(&RangeReport {
        level_dbm: -120,
        rate_100k: Some(u32::MAX),
        missed_beacons: true,
    });
    assert!(
        longest.len() <= jafnify_core::phone_stats::REQUEST_MAX,
        "{longest}"
    );
}

#[test]
fn a_phone_that_may_read_the_network_says_net() {
    let stats = PhoneStats {
        caps_net: true,
        ..PhoneStats::default()
    };
    let line = stats.to_response();
    assert!(line.contains("caps=net"), "{line}");
    let back = PhoneStats::parse_response(line.as_bytes()).unwrap();
    assert!(back.caps_net);
    assert!(!back.caps_link);
}

#[test]
fn both_capabilities_share_the_one_caps_word() {
    let stats = PhoneStats {
        caps_net: true,
        caps_link: true,
        ..PhoneStats::default()
    };
    let line = stats.to_response();
    assert!(line.contains("caps=net,link"), "{line}");
    let back = PhoneStats::parse_response(line.as_bytes()).unwrap();
    assert!(back.caps_net);
    assert!(back.caps_link);
}

#[test]
fn an_old_summary_without_caps_has_no_capability() {
    let line = PhoneStats::default().to_response();
    assert!(!line.contains("caps="), "{line}");
    let back = PhoneStats::parse_response(line.as_bytes()).unwrap();
    assert!(!back.caps_net);
    assert!(!back.caps_link);
}
