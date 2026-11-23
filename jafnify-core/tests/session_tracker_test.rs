//! Session tracking: intervals, drops, reconnects, rates and privacy.

use jafnify_core::session::{SessionRecord, SessionTracker};

const T0: i64 = 1_700_000_000_000;

#[test]
fn empty_tracker_reports_started_at_and_zeros() {
    let tracker = SessionTracker::new("joiner", T0);
    let stats = tracker.current();

    assert_eq!(stats.role, "joiner");
    assert_eq!(stats.started_at, T0);
    assert_eq!(stats.last_activity_at, 0);
    assert_eq!(stats.connected_ms, 0);
    assert_eq!(stats.up_bytes, 0);
    assert_eq!(stats.down_bytes, 0);
    assert_eq!(stats.avg_up_bps, 0);
    assert_eq!(stats.avg_down_bps, 0);
    assert_eq!(stats.peak_up_bps, 0);
    assert_eq!(stats.peak_down_bps, 0);
    assert_eq!(stats.drops, 0);
    assert!(stats.reconnects.is_empty());
    assert_eq!(stats.peer_endpoint, "");
    assert!(!stats.id.is_empty());
}

#[test]
fn one_connected_interval_has_totals_rates_and_peaks() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "10.1.0.2:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

    let stats = tracker.current();
    assert_eq!(stats.down_bytes, 300);
    assert_eq!(stats.up_bytes, 80);
    assert_eq!(stats.connected_ms, 2_000);
    assert_eq!(stats.avg_down_bps, 150);
    assert_eq!(stats.avg_up_bps, 40);
    assert_eq!(stats.peak_down_bps, 200);
    assert_eq!(stats.peak_up_bps, 50);
    assert_eq!(stats.last_activity_at, T0 + 2_000);
    assert_eq!(stats.peer_endpoint, "10.1.0.2:9797");
}

#[test]
fn a_drop_closes_the_interval_and_a_reconnect_records_the_gap() {
    let mut tracker = SessionTracker::new("sharer", T0);
    tracker.sample(T0, "Connected", 0, 0, "192.0.2.10:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    tracker.sample(T0 + 3_000, "Retry", 200, 50, "", "no data for 2 s");

    let dropped = tracker.current();
    assert_eq!(dropped.drops, 1);
    assert_eq!(dropped.reconnects.len(), 1);
    let record = &dropped.reconnects[0];
    assert_eq!(record.disconnected_at, T0 + 1_000);
    assert_eq!(record.detected_at, T0 + 3_000);
    assert_eq!(record.connected_at, 0);
    assert_eq!(record.duration_ms, 0);
    assert_eq!(record.reason, "no data for 2 s");
    // Connected from T0 to the drop sample, not through the gap.
    assert_eq!(dropped.connected_ms, 3_000);

    tracker.sample(T0 + 5_000, "Connected", 400, 90, "", "");

    let reconnected = tracker.current();
    assert_eq!(reconnected.drops, 1);
    assert_eq!(reconnected.reconnects.len(), 1);
    let record = &reconnected.reconnects[0];
    assert_eq!(record.disconnected_at, T0 + 1_000);
    assert_eq!(record.detected_at, T0 + 3_000);
    assert_eq!(record.connected_at, T0 + 5_000);
    assert_eq!(record.duration_ms, 4_000);
    // The non-Connected gap does not count toward connected_ms.
    assert_eq!(reconnected.connected_ms, 3_000);
}

#[test]
fn a_drop_before_any_activity_does_not_make_a_decades_long_reconnect() {
    let mut tracker = SessionTracker::new("joiner", T0);
    // First sample is only the counter baseline, so there is no activity yet.
    tracker.sample(T0, "Connected", 100, 100, "", "");
    tracker.sample(T0 + 1_000, "Retry", 100, 100, "", "no data for 1 s");

    let dropped = tracker.current();
    assert_eq!(dropped.reconnects.len(), 1);
    let record = &dropped.reconnects[0];
    assert_eq!(record.disconnected_at, T0 + 1_000);
    assert_eq!(record.detected_at, T0 + 1_000);

    tracker.sample(T0 + 3_000, "Connected", 200, 150, "", "");
    let reconnected = tracker.current();
    let record = &reconnected.reconnects[0];
    assert_eq!(record.connected_at, T0 + 3_000);
    assert_eq!(record.duration_ms, 2_000);
}

#[test]
fn a_second_drop_closes_a_still_pending_reconnect() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
    tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");
    // State is Connected again but there was no new activity, so the
    // reconnect stays pending.
    tracker.sample(T0 + 3_000, "Connected", 100, 0, "", "");
    assert_eq!(tracker.current().reconnects[0].connected_at, 0);

    tracker.sample(T0 + 4_000, "Retry", 100, 0, "", "link down");

    let stats = tracker.current();
    assert_eq!(stats.drops, 2);
    assert_eq!(stats.reconnects.len(), 2);
    let first = &stats.reconnects[0];
    assert_eq!(first.connected_at, T0 + 4_000);
    assert_eq!(first.duration_ms, 3_000);
    let second = &stats.reconnects[1];
    assert_eq!(second.disconnected_at, T0 + 4_000);
    assert_eq!(second.detected_at, T0 + 4_000);
    assert_eq!(second.connected_at, 0);
    assert_eq!(second.duration_ms, 0);
    assert!(first.connected_at <= second.disconnected_at);
    for pair in stats.reconnects.windows(2) {
        assert!(pair[0].connected_at <= pair[1].disconnected_at);
    }

    tracker.sample(T0 + 7_000, "Connected", 200, 0, "", "");
    let reconnected = tracker.current();
    let second = &reconnected.reconnects[1];
    assert_eq!(second.connected_at, T0 + 7_000);
    // The gap starts at the second drop, not the stale pre-first-drop time.
    assert_eq!(second.duration_ms, 3_000);
}

#[test]
fn finish_closes_a_pending_reconnect() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
    tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");
    // Connected again with no new activity leaves the reconnect pending.
    tracker.sample(T0 + 3_000, "Connected", 100, 0, "", "");
    assert_eq!(tracker.current().reconnects[0].connected_at, 0);

    let record = tracker.finish(T0 + 4_000, "stopped");
    assert_eq!(record.ended_at, T0 + 4_000);
    let reconnect = &record.reconnects[0];
    assert_eq!(reconnect.connected_at, T0 + 4_000);
    assert_eq!(reconnect.duration_ms, 3_000);

    let current = tracker.current();
    assert_eq!(current.reconnects, record.reconnects);
}

#[test]
fn finish_while_disconnected_leaves_the_reconnect_pending() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
    tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");

    let record = tracker.finish(T0 + 3_000, "lost");
    assert_eq!(record.ended_at, T0 + 3_000);
    assert_eq!(record.drops, 1);
    let reconnect = &record.reconnects[0];
    assert_eq!(reconnect.disconnected_at, T0 + 1_000);
    assert_eq!(reconnect.detected_at, T0 + 2_000);
    assert_eq!(reconnect.connected_at, 0);
    assert_eq!(reconnect.duration_ms, 0);
    assert_eq!(tracker.current().reconnects, record.reconnects);
}

#[test]
fn a_drop_uses_only_its_own_sample_reason() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "old reason");
    tracker.sample(T0 + 1_000, "Retry", 0, 0, "", "");
    assert_eq!(tracker.current().reconnects[0].reason, "");

    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "old reason");
    tracker.sample(T0 + 1_000, "Retry", 0, 0, "", "fresh reason");
    assert_eq!(tracker.current().reconnects[0].reason, "fresh reason");
}

#[test]
fn samples_after_finish_do_not_change_the_session() {
    let mut tracker = SessionTracker::new("sharer", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

    let record = tracker.finish(T0 + 3_000, "stopped");
    let after = tracker.current();

    tracker.sample(T0 + 4_000, "Connected", 500, 100, "", "");
    tracker.sample(T0 + 6_000, "Connected", 700, 150, "", "");
    let later = tracker.current();

    assert_eq!(later.connected_ms, after.connected_ms);
    assert_eq!(later.down_bytes, after.down_bytes);
    assert_eq!(later.up_bytes, after.up_bytes);
    assert_eq!(later.last_activity_at, after.last_activity_at);
    assert_eq!(later.peak_down_bps, after.peak_down_bps);
    assert_eq!(later.drops, after.drops);
    assert_eq!(later.reconnects, after.reconnects);

    let again = tracker.finish(T0 + 7_000, "stopped");
    assert_eq!(again.ended_at, record.ended_at);
    assert_eq!(again.connected_ms, record.connected_ms);
}

#[test]
fn a_counter_reset_starts_a_new_baseline_without_a_huge_peak() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 500, 0, "", "");

    tracker.sample(T0 + 2_000, "Connected", 10, 0, "", "");
    let reset = tracker.current();
    assert_eq!(reset.down_bytes, 500);
    assert_eq!(reset.peak_down_bps, 500);
    assert_eq!(reset.drops, 0);

    tracker.sample(T0 + 3_000, "Connected", 60, 0, "", "");
    let resumed = tracker.current();
    assert_eq!(resumed.down_bytes, 550);
    assert_eq!(resumed.peak_down_bps, 500);
    assert_eq!(resumed.drops, 0);
}

#[test]
fn finish_sets_the_end_time_and_reason_and_leaves_current_coherent() {
    let mut tracker = SessionTracker::new("sharer", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

    let record = tracker.finish(T0 + 3_000, "stopped");
    assert_eq!(record.ended_at, T0 + 3_000);
    assert_eq!(record.end_reason, "stopped");
    assert_eq!(record.started_at, T0);
    assert_eq!(record.down_bytes, 300);
    assert_eq!(record.up_bytes, 80);
    assert_eq!(record.connected_ms, 3_000);
    assert_eq!(record.avg_down_bps, 100);
    assert_eq!(record.avg_up_bps, 26);
    assert_eq!(record.peak_down_bps, 200);
    assert_eq!(record.peak_up_bps, 50);

    let current = tracker.current();
    assert_eq!(current.id, record.id);
    assert_eq!(current.started_at, T0);
    assert_eq!(current.down_bytes, 300);
    assert_eq!(current.connected_ms, 3_000);
}

#[test]
fn link_counters_start_at_zero_and_increment() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    let before = tracker.current();
    assert_eq!(before.link_downs, 0);
    assert_eq!(before.link_ups, 0);

    tracker.link_down();
    tracker.link_down();
    tracker.link_up();
    let after = tracker.current();
    assert_eq!(after.link_downs, 2);
    assert_eq!(after.link_ups, 1);
}

#[test]
fn finish_copies_the_link_counters() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.link_down();
    tracker.link_up();
    tracker.link_up();

    let record = tracker.finish(T0 + 1_000, "stopped");
    assert_eq!(record.link_downs, 1);
    assert_eq!(record.link_ups, 2);
    assert_eq!(tracker.current().link_downs, 1);
    assert_eq!(tracker.current().link_ups, 2);
}

#[test]
fn link_counters_are_no_ops_after_finish() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "", "");
    tracker.link_down();
    tracker.finish(T0 + 1_000, "stopped");

    tracker.link_down();
    tracker.link_up();
    assert_eq!(tracker.current().link_downs, 1);
    assert_eq!(tracker.current().link_ups, 0);
    let record = tracker.finish(T0 + 2_000, "stopped");
    assert_eq!(record.link_downs, 1);
    assert_eq!(record.link_ups, 0);
}

#[test]
fn an_old_record_without_link_counters_defaults_to_zero() {
    let text = "id = \"old\"\nrole = \"joiner\"\nstarted_at = 1\n";
    let record: SessionRecord = toml::from_str(text).unwrap();
    assert_eq!(record.link_downs, 0);
    assert_eq!(record.link_ups, 0);
}

#[test]
fn a_serialized_record_has_no_key_shaped_base64() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "10.1.0.2:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    let record = tracker.finish(T0 + 2_000, "app_exit");

    let text = toml::to_string(&record).unwrap();
    assert_eq!(
        jafnify_core::redact::redact_key_shaped(&text),
        text,
        "key-shaped text in {text}"
    );
}

#[test]
fn a_peer_restart_counts_one_drop_that_reconnected_at_once() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "192.0.2.10:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    tracker.peer_restart(T0 + 1_500, "Peer restarted");

    let stats = tracker.current();
    assert_eq!(stats.drops, 1);
    assert_eq!(stats.reconnects.len(), 1);
    let record = &stats.reconnects[0];
    assert_eq!(record.disconnected_at, T0 + 1_000);
    assert_eq!(record.detected_at, T0 + 1_500);
    assert_eq!(record.connected_at, T0 + 1_500);
    assert_eq!(record.duration_ms, 500);
    assert_eq!(record.reason, "Peer restarted");

    tracker.finish(T0 + 2_000, "stop");
    tracker.peer_restart(T0 + 3_000, "Peer restarted");
    assert_eq!(tracker.current().drops, 1);
}
