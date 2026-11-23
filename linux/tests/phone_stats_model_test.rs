use jafnify_core::bottleneck::LinkLeg;
use jafnify_core::phone_stats::{NetworkKind, PhoneStats, CONNECT_FIRST_LINE, UNAVAILABLE_LINE};
use jafnify_core::wifi_leg::WifiLeg;
use jafnify_linux::flags::{good_time_lines, heat_lines, phone_stats_lines};

fn stats() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Mobile5g,
        signal: Some(3),
        rtt_ms: Some(45),
        battery: Some(80),
        ..Default::default()
    }
}

fn no_link() -> Option<LinkLeg> {
    None
}

#[test]
fn the_flag_off_shows_nothing() {
    assert!(phone_stats_lines(false, true, Some(&stats()), Some(0), false, None, None).is_empty());
}

#[test]
fn on_but_disconnected_says_connect_first() {
    assert_eq!(
        phone_stats_lines(true, false, None, None, false, None, None),
        [
            CONNECT_FIRST_LINE.to_string(),
            "Not enough data yet.".to_string()
        ]
    );
}

#[test]
fn on_and_connected_shows_the_read_only_lines() {
    let lines = phone_stats_lines(
        true,
        true,
        Some(&stats()),
        Some(0),
        false,
        Some(LinkLeg {
            avg_ms: 20,
            loss_pct: 0,
        }),
        None,
    );
    assert_eq!(lines[0], "Network: 5G");
    assert!(lines.iter().any(|line| line == "DNS answer time: 45 ms"));
    assert!(lines.iter().any(|line| line == "Phone battery: 80%."));
    assert_eq!(lines.last().unwrap(), "No clear bottleneck");
}

#[test]
fn the_dns_line_is_shown_with_or_without_a_sample() {
    let with = phone_stats_lines(true, true, Some(&stats()), Some(0), false, None, None);
    assert!(with.iter().any(|line| line == "DNS answer time: 45 ms"));

    let mut unknown = stats();
    unknown.rtt_ms = None;
    let without = phone_stats_lines(true, true, Some(&unknown), Some(0), false, None, None);
    assert!(without
        .iter()
        .any(|line| line == "DNS answer time: not enough data"));
}

#[test]
fn the_bottleneck_line_uses_the_last_link_and_the_phone() {
    let slow_link = Some(LinkLeg {
        avg_ms: 80,
        loss_pct: 0,
    });
    assert_eq!(
        phone_stats_lines(true, true, Some(&stats()), Some(0), false, slow_link, None)
            .last()
            .unwrap(),
        "Slow because of: Wi-Fi to the phone"
    );

    let mut slow_phone = stats();
    slow_phone.rtt_ms = Some(150);
    assert_eq!(
        phone_stats_lines(
            true,
            true,
            Some(&slow_phone),
            Some(0),
            false,
            Some(LinkLeg {
                avg_ms: 20,
                loss_pct: 0,
            }),
            None,
        )
        .last()
        .unwrap(),
        "Slow because of: mobile network"
    );

    assert_eq!(
        phone_stats_lines(true, true, Some(&stats()), Some(0), false, no_link(), None)
            .last()
            .unwrap(),
        "Not enough data: run the link test."
    );
}

#[test]
fn a_phone_that_never_answered_is_unavailable() {
    assert_eq!(
        phone_stats_lines(true, true, None, None, true, None, None),
        [
            UNAVAILABLE_LINE.to_string(),
            "Not enough data yet.".to_string()
        ]
    );
}

#[test]
fn an_old_value_is_marked_stale() {
    let lines = phone_stats_lines(true, true, Some(&stats()), Some(16), false, None, None);
    assert_eq!(lines[lines.len() - 2], "Phone measurements are stale.");
    assert_eq!(lines.last().unwrap(), "Not enough data: run the link test.");
}

#[test]
fn the_passive_leg_wins_over_the_link_test() {
    let wifi = WifiLeg::new(80, 0, 30);
    let lines = phone_stats_lines(
        true,
        true,
        Some(&stats()),
        Some(0),
        false,
        Some(LinkLeg {
            avg_ms: 20,
            loss_pct: 0,
        }),
        Some(&wifi),
    );
    assert!(lines
        .iter()
        .any(|line| line == "Likely slow part: Wi-Fi to the phone"));
    assert!(!lines
        .iter()
        .any(|line| line == "Slow because of: Wi-Fi to the phone"));
}

#[test]
fn the_wifi_line_sits_under_the_verdict() {
    let wifi = WifiLeg::new(80, 3, 30);
    let lines = phone_stats_lines(
        true,
        true,
        Some(&stats()),
        Some(0),
        false,
        None,
        Some(&wifi),
    );
    let verdict = lines
        .iter()
        .position(|line| line == "Likely slow part: Wi-Fi to the phone")
        .expect("the verdict line");
    assert_eq!(
        lines[verdict + 1],
        "Wi-Fi to phone: 80 ms, 3% lost (last 30 checks)"
    );
}

#[test]
fn a_quiet_passive_leg_has_no_bottleneck() {
    let wifi = WifiLeg::new(20, 0, 30);
    let lines = phone_stats_lines(
        true,
        true,
        Some(&stats()),
        Some(0),
        false,
        None,
        Some(&wifi),
    );
    assert!(lines.iter().any(|line| line == "No clear bottleneck"));
}

#[test]
fn the_heat_line_replaces_the_battery_temperature_line() {
    let mut hot = stats();
    hot.temp_c_x10 = Some(430);
    let lines = phone_stats_lines(true, true, Some(&hot), Some(0), false, None, None);
    assert!(
        lines.iter().any(|line| line == "Phone is hot: 43 C."),
        "the battery feature has its own temperature line"
    );
    let lines = heat_lines(lines, true, Some(430));
    assert!(lines.iter().any(|line| line == "Phone is hot (43 C)"));
    assert!(
        !lines.iter().any(|line| line == "Phone is hot: 43 C."),
        "the phone shows one temperature, not two"
    );
}

#[test]
fn no_heat_line_while_the_switch_is_off() {
    let lines = heat_lines(Vec::new(), false, Some(430));
    assert!(lines.is_empty());
}

#[test]
fn no_heat_line_while_the_phone_is_not_hot() {
    let lines = heat_lines(Vec::new(), true, None);
    assert!(lines.is_empty());
}

#[test]
fn the_heat_line_shows_the_temperature_that_made_the_phone_hot() {
    let lines = heat_lines(Vec::new(), true, Some(431));
    assert_eq!(lines, vec!["Phone is hot (43.1 C)".to_string()]);
}

#[test]
fn the_good_time_line_says_it() {
    let lines = good_time_lines(Vec::new(), true, true);
    assert_eq!(lines, vec!["Good time for big downloads".to_string()]);
}

#[test]
fn no_good_time_line_while_the_switch_is_off() {
    let lines = good_time_lines(Vec::new(), false, true);
    assert!(lines.is_empty());
}
