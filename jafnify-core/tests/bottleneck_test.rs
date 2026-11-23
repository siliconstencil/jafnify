use jafnify_core::bottleneck::{line, passive_line, LinkLeg};
use jafnify_core::phone_stats::{NetworkKind, PhoneStats};

fn phone(net: NetworkKind, rtt_ms: Option<u32>) -> PhoneStats {
    PhoneStats {
        net,
        rtt_ms,
        ..Default::default()
    }
}

fn link(avg_ms: u64, loss_pct: u64) -> LinkLeg {
    LinkLeg { avg_ms, loss_pct }
}

const BOTH: &str = "Slow because of: Wi-Fi to the phone and mobile network";
const WIFI: &str = "Slow because of: Wi-Fi to the phone";
const PHONE: &str = "Slow because of: mobile network";
const NONE: &str = "No clear bottleneck";
const NEED_LINK: &str = "Not enough data: run the link test.";
const NEED_PHONE: &str = "Not enough data: phone measurements are off or unavailable.";
const NEED_BOTH: &str = "Not enough data yet.";

#[test]
fn missing_sources_get_the_three_not_enough_lines() {
    assert_eq!(line(None, None), NEED_BOTH);
    assert_eq!(
        line(None, Some(&phone(NetworkKind::Wifi, Some(20)))),
        NEED_LINK
    );
    assert_eq!(line(Some(link(20, 0)), None), NEED_PHONE);
}

#[test]
fn the_pc_leg_boundaries_are_fifty_ms_and_two_percent_loss() {
    let quiet = phone(NetworkKind::Wifi, Some(20));
    assert_eq!(line(Some(link(50, 1)), Some(&quiet)), NONE);
    assert_eq!(line(Some(link(50, 2)), Some(&quiet)), WIFI);
    assert_eq!(line(Some(link(51, 0)), Some(&quiet)), WIFI);
}

#[test]
fn the_phone_leg_boundaries_are_one_hundred_ms_and_no_network() {
    let quiet_link = Some(link(20, 0));
    assert_eq!(
        line(quiet_link, Some(&phone(NetworkKind::Mobile4g, Some(100)))),
        NONE
    );
    assert_eq!(
        line(quiet_link, Some(&phone(NetworkKind::Mobile4g, Some(101)))),
        PHONE
    );
    assert_eq!(
        line(quiet_link, Some(&phone(NetworkKind::None, None))),
        PHONE
    );
}

#[test]
fn the_future_2g_and_3g_words_are_slow() {
    let quiet_link = Some(link(20, 0));
    assert_eq!(
        line(quiet_link, Some(&phone(NetworkKind::Mobile2g, None))),
        PHONE
    );
    assert_eq!(
        line(quiet_link, Some(&phone(NetworkKind::Mobile3g, None))),
        PHONE
    );
}

#[test]
fn both_legs_slow_gets_the_combined_line() {
    assert_eq!(
        line(
            Some(link(80, 5)),
            Some(&phone(NetworkKind::Mobile4g, Some(150)))
        ),
        BOTH
    );
}

#[test]
fn a_quiet_wifi_phone_with_a_quiet_link_has_no_bottleneck() {
    assert_eq!(
        line(Some(link(10, 0)), Some(&phone(NetworkKind::Wifi, Some(45)))),
        NONE
    );
}

const LIKELY_WIFI: &str = "Likely slow part: Wi-Fi to the phone";
const LIKELY_BOTH: &str = "Likely slow parts: Wi-Fi to the phone and mobile network";

#[test]
fn the_passive_wifi_verdict_only_says_likely() {
    let quiet = phone(NetworkKind::Wifi, Some(20));
    assert_eq!(passive_line(Some(link(80, 0)), Some(&quiet)), LIKELY_WIFI);
}

#[test]
fn the_passive_wifi_verdict_keeps_the_same_boundaries() {
    let quiet = phone(NetworkKind::Wifi, Some(20));
    assert_eq!(passive_line(Some(link(50, 1)), Some(&quiet)), NONE);
    assert_eq!(passive_line(Some(link(50, 2)), Some(&quiet)), LIKELY_WIFI);
    assert_eq!(passive_line(Some(link(51, 0)), Some(&quiet)), LIKELY_WIFI);
}

#[test]
fn the_passive_verdict_hedges_the_wifi_part_of_both_legs() {
    assert_eq!(
        passive_line(
            Some(link(80, 5)),
            Some(&phone(NetworkKind::Mobile4g, Some(150)))
        ),
        LIKELY_BOTH
    );
}

#[test]
fn the_passive_verdict_leaves_the_mobile_verdict_alone() {
    let quiet_link = Some(link(20, 0));
    assert_eq!(
        passive_line(quiet_link, Some(&phone(NetworkKind::Mobile4g, Some(101)))),
        PHONE
    );
    assert_eq!(
        passive_line(quiet_link, Some(&phone(NetworkKind::None, None))),
        PHONE
    );
}

#[test]
fn the_passive_verdict_keeps_the_not_enough_lines() {
    assert_eq!(passive_line(None, None), NEED_BOTH);
    assert_eq!(
        passive_line(None, Some(&phone(NetworkKind::Wifi, Some(20)))),
        NEED_LINK
    );
    assert_eq!(passive_line(Some(link(20, 0)), None), NEED_PHONE);
}
