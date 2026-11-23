use std::time::{Duration, Instant};

use jafnify_core::good_time::{self, GoodTimeTracker};
use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::wifi_leg::WifiLeg;

fn start() -> Instant {
    Instant::now()
}

#[test]
fn a_5g_phone_with_a_strong_signal_is_good() {
    assert!(good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        None
    ));
    assert!(good_time::is_good(
        NetworkKind::Mobile5g,
        Some(3),
        Some(60),
        None
    ));
}

#[test]
fn a_slower_network_is_not_good() {
    assert!(!good_time::is_good(
        NetworkKind::Mobile4gPlus,
        Some(4),
        Some(45),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Wifi,
        Some(4),
        Some(45),
        None
    ));
}

#[test]
fn a_weak_signal_or_a_slow_phone_is_not_good() {
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(2),
        Some(45),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(61),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        None,
        Some(45),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        None,
        None
    ));
}

#[test]
fn a_slow_wifi_leg_makes_it_not_good() {
    let slow = WifiLeg::new(80, 0, 20);
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        Some(&slow)
    ));
    let lossy = WifiLeg::new(20, 5, 20);
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        Some(&lossy)
    ));
    let quiet = WifiLeg::new(50, 1, 20);
    assert!(good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        Some(&quiet)
    ));
}

#[test]
fn there_is_no_notice_at_start_up() {
    let mut tracker = GoodTimeTracker::new();
    let now = start();
    assert!(tracker.update(true, "5G (advanced)", 4, now).is_none());
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(600))
        .is_none());
}

#[test]
fn a_notice_needs_two_minutes_of_good_after_ten_minutes_of_bad() {
    let mut tracker = GoodTimeTracker::new();
    let now = start();
    tracker.update(false, "3G", 2, now);
    tracker.update(false, "3G", 2, now + Duration::from_secs(600));
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(601))
        .is_none());
    let notice = tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(721))
        .expect("the notice");
    assert_eq!(
        notice.line(),
        "Good time for big downloads: 5G (advanced), signal 4/4"
    );
}

#[test]
fn a_short_dip_does_not_bring_a_notice() {
    let mut tracker = GoodTimeTracker::new();
    let now = start();
    tracker.update(false, "3G", 2, now);
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(300));
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(420))
        .is_none());
}

#[test]
fn every_5g_kind_counts() {
    for net in [
        NetworkKind::Mobile5g,
        NetworkKind::Mobile5gNsa,
        NetworkKind::Mobile5gMmwave,
        NetworkKind::Mobile5gAdv,
    ] {
        assert!(
            good_time::is_good(net, Some(4), Some(45), None),
            "{net:?} is a 5G kind"
        );
    }
}

#[test]
fn the_signal_and_the_delay_boundaries_are_exact() {
    assert!(good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(3),
        Some(60),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(2),
        Some(60),
        None
    ));
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(61),
        None
    ));
}

#[test]
fn a_wifi_leg_with_two_percent_loss_is_not_good() {
    let lossy = WifiLeg::new(20, 2, 20);
    assert!(!good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        Some(&lossy)
    ));
    let quiet = WifiLeg::new(20, 1, 20);
    assert!(good_time::is_good(
        NetworkKind::Mobile5gAdv,
        Some(4),
        Some(45),
        Some(&quiet)
    ));
}

#[test]
fn a_long_bad_stretch_survives_a_short_dip() {
    let mut tracker = GoodTimeTracker::new();
    let now = start();
    tracker.update(false, "3G", 2, now);
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(600));
    // the link dips for a minute
    tracker.update(false, "3G", 2, now + Duration::from_secs(700));
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(760));
    // the good period still has to hold two minutes
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(879))
        .is_none());
    let notice = tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(880))
        .expect("the notice");
    assert_eq!(notice.signal, 4);
}

#[test]
fn there_is_at_most_one_notice_an_hour() {
    let mut tracker = GoodTimeTracker::new();
    let now = start();
    tracker.update(false, "3G", 2, now);
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(601));
    let first = tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(721))
        .expect("the first notice");
    assert_eq!(first.signal, 4);
    // the conditions get bad again and come back, but inside the hour
    tracker.update(false, "3G", 2, now + Duration::from_secs(800));
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(1401));
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(1521))
        .is_none());
    // after the hour, again
    tracker.update(false, "3G", 2, now + Duration::from_secs(4000));
    tracker.update(true, "5G (advanced)", 4, now + Duration::from_secs(4601));
    assert!(tracker
        .update(true, "5G (advanced)", 4, now + Duration::from_secs(4721))
        .is_some());
}
