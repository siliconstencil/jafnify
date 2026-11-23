//! The desktop notification lines and the rate model.

use std::time::{Duration, Instant};

use jafnify_core::notification::{
    connection_transition, Notification, NotificationPolicy, HOUR, MAX_PER_HOUR, SAME_KIND_DEDUPE,
};
use jafnify_core::orchestrator::ConnectionState;

#[test]
fn every_notification_has_one_fixed_line() {
    assert_eq!(Notification::Connected.line(), "Jafnify connected.");
    assert_eq!(Notification::Lost.line(), "Jafnify lost the connection.");
    assert_eq!(Notification::Retrying.line(), "Jafnify is retrying.");
    assert_eq!(
        Notification::PhoneLostMobileData.line(),
        "Phone lost mobile data."
    );
    assert_eq!(
        Notification::PhoneGenerationDrop {
            from: "5G",
            to: "4G"
        }
        .line(),
        "Phone network changed: 5G to 4G."
    );
}

#[test]
fn a_transition_produces_one_line() {
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert_eq!(
        policy.decide(Notification::Connected, now),
        Some("Jafnify connected.".to_string())
    );
    assert_eq!(
        policy.decide(Notification::Lost, now),
        Some("Jafnify lost the connection.".to_string())
    );
}

#[test]
fn a_connection_transition_picks_the_one_line() {
    assert_eq!(
        connection_transition(ConnectionState::Searching, ConnectionState::Connected),
        Some(Notification::Connected)
    );
    assert_eq!(
        connection_transition(ConnectionState::Connected, ConnectionState::Searching),
        Some(Notification::Lost)
    );
    assert_eq!(
        connection_transition(ConnectionState::Connected, ConnectionState::Retry),
        Some(Notification::Lost),
        "a lost connection is one line, not two"
    );
    assert_eq!(
        connection_transition(ConnectionState::Searching, ConnectionState::Retry),
        Some(Notification::Retrying)
    );
    assert_eq!(
        connection_transition(ConnectionState::Connected, ConnectionState::Connected),
        None
    );
    assert_eq!(
        connection_transition(ConnectionState::Idle, ConnectionState::Searching),
        None
    );
}

#[test]
fn the_same_notification_is_dropped_for_five_minutes() {
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert!(policy.decide(Notification::Connected, now).is_some());
    assert!(
        policy
            .decide(Notification::Connected, now + SAME_KIND_DEDUPE / 2)
            .is_none(),
        "the same line must not repeat inside the dedupe window"
    );
    assert!(
        policy
            .decide(
                Notification::Connected,
                now + SAME_KIND_DEDUPE + Duration::from_secs(1)
            )
            .is_some(),
        "the same line may come again after the dedupe window"
    );
}

#[test]
fn a_different_notification_is_not_blocked_by_the_dedupe() {
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert!(policy.decide(Notification::Connected, now).is_some());
    assert!(policy.decide(Notification::Retrying, now).is_some());
    assert!(policy.decide(Notification::Lost, now).is_some());
}

#[test]
fn at_most_five_notifications_go_out_in_an_hour() {
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    // Retrying first: once an outage is announced, Retrying is held back.
    let kinds = [
        Notification::Retrying,
        Notification::Connected,
        Notification::Lost,
        Notification::PhoneLostMobileData,
        Notification::PhoneGenerationDrop {
            from: "5G",
            to: "4G",
        },
    ];
    for kind in kinds {
        assert!(policy.decide(kind, now).is_some());
    }
    assert!(
        policy
            .decide(Notification::Connected, now + Duration::from_secs(1))
            .is_none(),
        "the sixth notification in the hour must be dropped"
    );
    assert!(
        policy
            .decide(Notification::Connected, now + HOUR + Duration::from_secs(1))
            .is_some(),
        "the hourly count starts again after an hour"
    );
    assert_eq!(MAX_PER_HOUR, 5);
}

#[test]
fn a_user_stop_is_not_a_lost_connection() {
    use jafnify_core::ConnectionState;
    assert_eq!(
        connection_transition(ConnectionState::Connected, ConnectionState::Idle),
        None
    );
    assert_eq!(
        connection_transition(ConnectionState::Retry, ConnectionState::Idle),
        None
    );
}

#[test]
fn one_outage_says_retrying_at_most_once() {
    use std::time::{Duration, Instant};
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert!(policy.decide(Notification::Connected, now).is_some());
    assert!(policy
        .decide(Notification::Lost, now + Duration::from_secs(10))
        .is_some());
    // The retry cycles of the same outage stay silent, even after the
    // five-minute same-kind dedupe.
    for minutes in [1, 6, 12, 30] {
        assert!(policy
            .decide(
                Notification::Retrying,
                now + Duration::from_secs(minutes * 60)
            )
            .is_none());
    }
    // Back up: a later outage is announced again.
    assert!(policy
        .decide(Notification::Connected, now + Duration::from_secs(40 * 60))
        .is_some());
    assert!(policy
        .decide(Notification::Lost, now + Duration::from_secs(50 * 60))
        .is_some());
}

#[test]
fn a_first_start_that_never_connects_says_retrying_once() {
    use std::time::{Duration, Instant};
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert!(policy.decide(Notification::Retrying, now).is_some());
    assert!(policy
        .decide(Notification::Retrying, now + Duration::from_secs(600))
        .is_none());
}

#[test]
fn the_hot_phone_notification_carries_the_temperature() {
    let notification = Notification::PhoneHot { temp_c_x10: 431 };
    assert_eq!(notification.line(), "Phone is hot (43.1 C)");
    // The phone does not report its throttling, so the app must not claim a
    // slowdown; it only says that a hot phone can slow the data down.
    assert_eq!(
        notification.body(),
        Some("Hot phones can slow mobile data down; this one does not report throttling.")
    );
}

#[test]
fn the_throttling_notification_says_what_android_says() {
    let notification = Notification::PhoneThrottling;
    assert_eq!(notification.line(), "Phone is throttling because of heat");
    assert_eq!(
        notification.body(),
        Some("Android says the phone is limiting itself; mobile data can slow down. Move it out of the sun; unplug the charger if it is charging.")
    );
}

#[test]
fn the_good_time_notification_carries_the_network_and_the_signal() {
    let notification = Notification::GoodTime {
        net: "5G (advanced)",
        signal: 4,
    };
    assert_eq!(
        notification.line(),
        "Good time for big downloads: 5G (advanced), signal 4/4"
    );
    assert_eq!(notification.body(), None);
}

#[test]
fn the_old_notifications_have_no_body() {
    assert_eq!(Notification::Connected.body(), None);
    assert_eq!(Notification::Lost.body(), None);
}

#[test]
fn a_second_hot_episode_is_not_deduped() {
    let mut policy = NotificationPolicy::new();
    let now = Instant::now();
    assert!(policy
        .decide(Notification::PhoneHot { temp_c_x10: 431 }, now)
        .is_some());
    assert!(
        policy
            .decide(
                Notification::PhoneHot { temp_c_x10: 431 },
                now + Duration::from_secs(10)
            )
            .is_some(),
        "the phone cooled down and got hot again: that is news"
    );
}
