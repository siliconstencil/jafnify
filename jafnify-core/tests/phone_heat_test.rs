use jafnify_core::notification::Notification;
use jafnify_core::phone_heat::{self, Heat, HeatTracker, Warning};

#[test]
fn the_phone_knows_when_android_is_not_throttling() {
    assert_eq!(
        phone_heat::phone_tile_line(Some(300), Some(0)),
        Some("Android: no throttling".to_string())
    );
    assert_eq!(
        phone_heat::phone_tile_line(Some(300), Some(1)),
        Some("Android: no throttling".to_string())
    );
}

#[test]
fn the_phone_says_when_android_is_throttling_a_little() {
    assert_eq!(
        phone_heat::phone_tile_line(Some(300), Some(2)),
        Some("Android: light throttling".to_string())
    );
}

#[test]
fn the_phone_says_when_android_is_throttling() {
    assert_eq!(
        phone_heat::phone_tile_line(Some(300), Some(3)),
        Some("Android says the phone is limiting itself".to_string())
    );
}

#[test]
fn a_phone_without_the_thermal_status_says_only_that_it_is_warm() {
    assert_eq!(
        phone_heat::phone_tile_line(Some(430), None),
        Some("Phone is warm (43 C)".to_string())
    );
    assert_eq!(phone_heat::phone_tile_line(Some(300), None), None);
}

#[test]
fn a_phone_below_39_degrees_is_normal() {
    assert_eq!(phone_heat::level(Some(389)), Some(Heat::Normal));
}

#[test]
fn a_phone_between_39_and_42_degrees_is_warm() {
    assert_eq!(phone_heat::level(Some(390)), Some(Heat::Warm));
    assert_eq!(phone_heat::level(Some(419)), Some(Heat::Warm));
}

#[test]
fn a_phone_at_42_degrees_is_hot() {
    assert_eq!(phone_heat::level(Some(420)), Some(Heat::Hot));
    assert_eq!(phone_heat::level(Some(431)), Some(Heat::Hot));
}

#[test]
fn an_unknown_temperature_has_no_level() {
    assert_eq!(phone_heat::level(None), None);
}

#[test]
fn a_hot_phone_stays_hot_until_it_cools_down() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(430), None).expect("the reading");
    assert!(change.became_hot);
    assert_eq!(tracker.level(), Heat::Hot);

    let change = tracker.update(Some(410), None).expect("the reading");
    assert!(!change.became_hot);
    assert_eq!(tracker.level(), Heat::Hot, "39.0 C is not cool enough yet");

    let change = tracker.update(Some(389), None).expect("the reading");
    assert!(change.cooled);
    assert_eq!(tracker.level(), Heat::Normal);
}

#[test]
fn an_unknown_reading_does_not_move_the_state() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(430), None);
    assert!(tracker.update(None, None).is_none());
    assert_eq!(tracker.level(), Heat::Hot);
}

#[test]
fn a_reset_forgets_the_phone() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(430), None);
    tracker.reset();
    assert_eq!(tracker.level(), Heat::Normal);
}

#[test]
fn the_heat_edges_are_exact() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(419), None).expect("the reading");
    assert_eq!(change.level, Heat::Warm);
    let change = tracker.update(Some(420), None).expect("the reading");
    assert!(change.became_hot);
    assert_eq!(change.level, Heat::Hot);
    // 39.0 C is not cool enough: the phone stays hot.
    tracker.update(Some(390), None);
    assert_eq!(tracker.level(), Heat::Hot);
    tracker.update(Some(389), None);
    assert_eq!(tracker.level(), Heat::Normal);
}

#[test]
fn a_hot_phone_keeps_the_temperature_that_made_it_hot() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(431), None);
    assert_eq!(tracker.hot_temp_c_x10(), Some(431));
    // The phone cools a little, but not enough: the line keeps the number
    // that made it hot, so it never calls 40 C hot.
    tracker.update(Some(400), None);
    assert_eq!(tracker.level(), Heat::Hot);
    assert_eq!(tracker.hot_temp_c_x10(), Some(431));
    tracker.update(Some(389), None);
    assert_eq!(tracker.hot_temp_c_x10(), None);
}

#[test]
fn a_phone_that_limits_itself_says_so() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(430), Some(3)).expect("the reading");
    assert_eq!(change.warning, Some(Warning::Throttling));
    assert!(tracker.throttling());
}

#[test]
fn the_throttling_state_is_cleared_when_the_phone_calms_down() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(430), Some(3));
    let change = tracker.update(Some(380), Some(1)).expect("the reading");
    assert!(change.stopped_throttling);
    assert!(!tracker.throttling());
}

#[test]
fn a_moderate_phone_is_not_warned_about() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(430), Some(2)).expect("the reading");
    assert_eq!(change.warning, None);
    assert!(!tracker.throttling());
}

#[test]
fn a_phone_that_reports_the_heat_is_not_warned_about_by_the_temperature() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(430), Some(0)).expect("the reading");
    assert_eq!(change.warning, None, "the phone itself says it is fine");
    assert!(change.became_hot, "the temperature still crossed");
}

#[test]
fn an_old_phone_is_warned_about_by_the_temperature_after_two_readings() {
    let mut tracker = HeatTracker::new();
    let change = tracker.update(Some(430), None).expect("the reading");
    assert_eq!(change.warning, None, "one reading is never enough");
    let change = tracker.update(Some(430), None).expect("the reading");
    assert_eq!(change.warning, Some(Warning::Hot { temp_c_x10: 430 }));
    // The warning is one episode: more hot readings do not repeat it.
    let change = tracker.update(Some(435), None).expect("the reading");
    assert_eq!(change.warning, None);
}

#[test]
fn the_second_hot_reading_may_be_exactly_at_the_edge() {
    let mut tracker = HeatTracker::new();
    assert_eq!(tracker.update(Some(420), None).unwrap().warning, None);
    assert_eq!(
        tracker.update(Some(420), None).unwrap().warning,
        Some(Warning::Hot { temp_c_x10: 420 })
    );
}

#[test]
fn a_reading_below_the_edge_breaks_the_row() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(430), None);
    tracker.update(Some(419), None);
    assert_eq!(tracker.update(Some(430), None).unwrap().warning, None);
    assert_eq!(
        tracker.update(Some(430), None).unwrap().warning,
        Some(Warning::Hot { temp_c_x10: 430 })
    );
}

#[test]
fn a_phone_that_ever_reported_its_heat_is_never_warned_by_temperature() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(380), Some(0));
    assert!(tracker.thr_seen());
    // The thermal key is missing in the next summary: the phone already
    // spoke in this session, so the temperature alone stays quiet.
    tracker.update(Some(430), None);
    assert_eq!(tracker.update(Some(430), None).unwrap().warning, None);
    // A reset (a new session) forgets that.
    tracker.reset();
    assert!(!tracker.thr_seen());
}

#[test]
fn a_cool_phone_can_be_warned_again_after_a_new_hot_episode() {
    let mut tracker = HeatTracker::new();
    tracker.update(Some(430), None);
    assert!(tracker.update(Some(430), None).unwrap().warning.is_some());
    tracker.update(Some(380), None);
    tracker.update(Some(430), None);
    assert!(
        tracker.update(Some(430), None).unwrap().warning.is_some(),
        "cooling down starts a new episode"
    );
}

#[test]
fn the_throttling_notification_says_the_phone_limits_itself() {
    let notification = Notification::PhoneThrottling;
    assert_eq!(notification.line(), "Phone is throttling because of heat");
    assert_eq!(
        notification.body(),
        Some(
            "Android says the phone is limiting itself; mobile data can slow down. Move it out of the sun; unplug the charger if it is charging."
        )
    );
}

#[test]
fn the_tile_line_says_what_the_phone_reports() {
    assert_eq!(
        phone_heat::tile_line(Some(430), Some(3)),
        Some("Android says the phone is limiting itself".to_string())
    );
    assert_eq!(
        phone_heat::tile_line(Some(430), Some(2)),
        Some("Android: light throttling".to_string())
    );
    assert_eq!(phone_heat::tile_line(Some(430), Some(1)), None);
}

#[test]
fn the_tile_line_of_a_warm_phone_never_claims_a_slowdown() {
    let line = phone_heat::tile_line(Some(431), None).expect("the phone is warm");
    assert_eq!(line, "Phone is warm (43.1 C)");
    assert!(!line.contains("slow"), "{line}");
}

#[test]
fn the_line_says_the_temperature() {
    assert_eq!(phone_heat::line(431), "Phone is hot (43.1 C)");
    assert_eq!(phone_heat::title(431), "Phone is hot (43.1 C)");
}

#[test]
fn the_help_of_an_old_phone_never_claims_a_slowdown() {
    assert_eq!(
        phone_heat::help(),
        "Hot phones can slow mobile data down; this one does not report throttling."
    );
}

#[test]
fn the_heat_line_replaces_the_battery_temperature_line() {
    let mut lines = vec![
        "Network: 5G (advanced)".to_string(),
        "Phone is hot: 43.1 C.".to_string(),
    ];
    phone_heat::replace_temperature_line(&mut lines);
    assert_eq!(lines, vec!["Network: 5G (advanced)".to_string()]);
}

#[test]
fn any_temperature_line_is_replaced() {
    let mut lines = vec![
        "Network: 5G (advanced)".to_string(),
        "Phone temperature: 39.5 C.".to_string(),
    ];
    phone_heat::replace_temperature_line(&mut lines);
    assert_eq!(lines, vec!["Network: 5G (advanced)".to_string()]);
}

#[test]
fn the_headroom_forecast_speaks_only_when_it_is_high() {
    assert_eq!(phone_heat::headroom_line(None), None);
    assert_eq!(phone_heat::headroom_line(Some(0)), None);
    assert_eq!(phone_heat::headroom_line(Some(84)), None);
    assert_eq!(
        phone_heat::headroom_line(Some(85)),
        Some("Likely warmer soon".to_string())
    );
    assert_eq!(
        phone_heat::headroom_line(Some(99)),
        Some("Likely warmer soon".to_string())
    );
    assert_eq!(
        phone_heat::headroom_line(Some(100)),
        Some("Likely throttling within 30 s".to_string())
    );
    assert_eq!(
        phone_heat::headroom_line(Some(200)),
        Some("Likely throttling within 30 s".to_string())
    );
}

#[test]
fn a_phone_that_already_throttles_keeps_that_line_over_the_forecast() {
    use jafnify_core::tiles::{heat_tile, phone_tile, TrendBuffer};
    let trend = TrendBuffer::default();
    // Android reports throttling now: the real fault keeps its line.
    let heat = heat_tile(Some(450), Some(4), Some(120), &trend).unwrap();
    let facts = format!("{heat:?}");
    assert!(
        facts.contains("Android says the phone is limiting itself"),
        "{facts}"
    );
    assert!(!facts.contains("Likely throttling within 30 s"), "{facts}");
    let phone = phone_tile(Some(450), Some(2), Some(120), Some(60), &trend, None).unwrap();
    let facts = format!("{phone:?}");
    assert!(facts.contains("Android: light throttling"), "{facts}");
    assert!(!facts.contains("Likely"), "{facts}");
    // No throttling yet: the forecast speaks.
    let calm = heat_tile(Some(450), Some(0), Some(120), &trend).unwrap();
    assert!(format!("{calm:?}").contains("Likely throttling within 30 s"));
    assert_eq!(
        phone_heat::forecast_line(Some(90), None),
        Some("Likely warmer soon".to_string())
    );
}
