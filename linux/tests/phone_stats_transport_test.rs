use std::time::{Duration, Instant};

use jafnify_core::phone_stats::{NetworkKind, PhoneStats};
use jafnify_linux::phone_stats::{
    step, PhoneStatsShared, PhoneStatsTransport, PhoneSummary, TransportError, INTERVAL_IDLE,
    INTERVAL_WINDOW,
};

#[derive(Default)]
struct Fake {
    calls: usize,
    ranges: Vec<Option<jafnify_core::wifi_range::RangeReport>>,
    reply: Option<PhoneStats>,
    fail: bool,
    error: Option<TransportError>,
}

impl PhoneStatsTransport for Fake {
    fn request_summary(
        &mut self,
        range: Option<&jafnify_core::wifi_range::RangeReport>,
    ) -> Result<Option<PhoneSummary>, TransportError> {
        self.calls += 1;
        self.ranges.push(range.copied());
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.fail {
            Err(TransportError::Timeout)
        } else {
            Ok(self
                .reply
                .clone()
                .map(|stats| PhoneSummary { stats, exit: None }))
        }
    }
}

fn stats() -> PhoneStats {
    PhoneStats {
        net: NetworkKind::Wifi,
        signal: Some(3),
        ..Default::default()
    }
}

#[test]
fn off_and_disconnected_make_no_request() {
    let shared = PhoneStatsShared::new();
    let mut fake = Fake::default();
    step(&shared, &mut fake, Instant::now());
    assert_eq!(fake.calls, 0);

    shared.set(false, true, false);
    step(&shared, &mut fake, Instant::now());
    assert_eq!(fake.calls, 0);

    shared.set(true, false, false);
    step(&shared, &mut fake, Instant::now());
    assert_eq!(fake.calls, 0);

    shared.set(true, true, false);
    step(&shared, &mut fake, Instant::now());
    assert_eq!(fake.calls, 1);
}

#[test]
fn a_success_fills_the_cache_and_uses_the_closed_interval() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);

    assert_eq!(step(&shared, &mut fake, now), INTERVAL_IDLE);
    let (cached, age, unavailable) = shared.snapshot(now);
    assert_eq!(cached, Some(stats()));
    assert_eq!(age, Some(0));
    assert!(!unavailable);
}

#[test]
fn an_open_window_polls_faster() {
    let shared = PhoneStatsShared::new();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, true);
    assert_eq!(step(&shared, &mut fake, Instant::now()), INTERVAL_WINDOW);
}

#[test]
fn three_timeouts_without_a_value_mark_unavailable() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        fail: true,
        ..Default::default()
    };
    shared.set(true, true, false);

    for _ in 0..2 {
        step(&shared, &mut fake, now);
    }
    assert!(!shared.snapshot(now).2);
    step(&shared, &mut fake, now);
    let (cached, _, unavailable) = shared.snapshot(now);
    assert_eq!(cached, None);
    assert!(unavailable);
}

#[test]
fn a_success_after_timeouts_clears_unavailable() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        fail: true,
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..3 {
        step(&shared, &mut fake, now);
    }
    assert!(shared.snapshot(now).2);

    fake.fail = false;
    fake.reply = Some(stats());
    step(&shared, &mut fake, now);
    let (cached, _, unavailable) = shared.snapshot(now);
    assert_eq!(cached, Some(stats()));
    assert!(!unavailable);
}

#[test]
fn a_timeout_keeps_the_last_value_and_does_not_mark_unavailable() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    step(&shared, &mut fake, now);

    fake.fail = true;
    for _ in 0..5 {
        step(&shared, &mut fake, now);
    }
    let (cached, _, unavailable) = shared.snapshot(now);
    assert_eq!(cached, Some(stats()));
    assert!(!unavailable, "an old value is stale, not unavailable");
}

#[test]
fn the_age_grows_from_the_last_success() {
    let shared = PhoneStatsShared::new();
    let start = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    step(&shared, &mut fake, start);

    fake.fail = true;
    step(&shared, &mut fake, start + Duration::from_secs(16));
    assert_eq!(shared.snapshot(start + Duration::from_secs(16)).1, Some(16));
}

#[test]
fn turning_the_flag_off_stops_requests_but_keeps_the_value() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    step(&shared, &mut fake, now);
    assert_eq!(fake.calls, 1);

    shared.set(false, true, false);
    step(&shared, &mut fake, now);
    assert_eq!(fake.calls, 1);
    assert_eq!(shared.snapshot(now).0, Some(stats()));
}

#[test]
fn an_error_drops_the_socket_so_a_recreated_jafn0_is_used() {
    use jafnify_linux::phone_stats::{should_rebind, TransportError};
    assert!(should_rebind(&Err(TransportError::Io(
        "No such device".into()
    ))));
    assert!(should_rebind(&Err(TransportError::Timeout)));
    assert!(!should_rebind(&Err(TransportError::Malformed)));
    assert!(!should_rebind(&Ok(None)));
}

#[test]
fn a_timeout_counts_as_a_lost_sample() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..9 {
        step(&shared, &mut fake, now);
    }
    fake.fail = true;
    step(&shared, &mut fake, now);

    assert_eq!(shared.wifi_samples(), 10);
    let leg = shared.wifi_leg().expect("the window holds enough samples");
    assert_eq!(leg.loss_pct, 10);
    assert_eq!(leg.samples, 10);
}

#[test]
fn a_transport_error_is_not_a_sample() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        error: Some(TransportError::Io("No such device".into())),
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..10 {
        step(&shared, &mut fake, now);
    }
    assert_eq!(shared.wifi_samples(), 0);
    assert!(shared.wifi_leg().is_none());
}

#[test]
fn a_malformed_reply_is_not_a_sample() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        error: Some(TransportError::Malformed),
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..10 {
        step(&shared, &mut fake, now);
    }
    assert_eq!(shared.wifi_samples(), 0);
}

#[test]
fn the_window_starts_over_when_the_session_changes() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..10 {
        step(&shared, &mut fake, now);
    }
    assert!(shared.wifi_leg().is_some());

    shared.set(true, false, false);
    assert_eq!(shared.wifi_samples(), 0);
    assert!(shared.wifi_leg().is_none());
}

#[test]
fn the_window_starts_over_when_the_switch_goes_off() {
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    let mut fake = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    shared.set(true, true, false);
    for _ in 0..10 {
        step(&shared, &mut fake, now);
    }
    assert!(shared.wifi_leg().is_some());

    shared.set(false, true, false);
    assert_eq!(shared.wifi_samples(), 0);
}

#[test]
fn the_link_report_goes_only_to_a_phone_that_takes_it() {
    use jafnify_core::wifi_range::RangeReport;
    let report = RangeReport {
        level_dbm: -66,
        rate_100k: Some(4333),
        missed_beacons: false,
    };
    let shared = PhoneStatsShared::new();
    let now = Instant::now();
    shared.set(true, true, false);
    shared.set_range(Some(report));

    // An older phone: it never said caps=link, so it never gets the words
    // it would reject.
    let mut old = Fake {
        reply: Some(stats()),
        ..Default::default()
    };
    step(&shared, &mut old, now);
    step(&shared, &mut old, now + Duration::from_secs(5));
    assert_eq!(old.ranges, vec![None, None]);

    // A newer phone: the first request is plain, the ones after its answer
    // carry the report.
    let shared = PhoneStatsShared::new();
    shared.set(true, true, false);
    shared.set_range(Some(report));
    let mut new = Fake {
        reply: Some(PhoneStats {
            caps_link: true,
            ..stats()
        }),
        ..Default::default()
    };
    step(&shared, &mut new, now);
    step(&shared, &mut new, now + Duration::from_secs(5));
    assert_eq!(new.ranges, vec![None, Some(report)]);

    // The range switch off: nothing more is sent.
    shared.set_range(None);
    step(&shared, &mut new, now + Duration::from_secs(10));
    assert_eq!(new.ranges.last(), Some(&None));
}
