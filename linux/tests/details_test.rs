//! The Linux Details table: the same sections, labels and order as the
//! phone's (Connection, This session), pinned so the two cannot drift.

use jafnify_linux::control::{HostHintRow, Snapshot};
use jafnify_linux::details::{handshake_age, history_rows, sections};
use jafnify_linux::session::{SessionRecord, SessionStats};

fn labels(rows: &[jafnify_linux::details::DetailRow]) -> Vec<&str> {
    rows.iter().map(|r| r.label.as_str()).collect()
}

#[test]
fn idle_details_have_only_the_connection_section() {
    let snap = Snapshot::default();
    let s = sections(&snap, None);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].title, "Connection");
    assert_eq!(
        labels(&s[0].rows),
        vec![
            "Status",
            "Path",
            "MTU",
            "DNS",
            "Phone address",
            "Pair id",
            "Last error"
        ]
    );
    assert_eq!(s[0].rows[1].value, "unknown");
    assert_eq!(s[0].rows[2].value, "1280");
    assert_eq!(s[0].rows[3].value, "system resolver (not changed)");
    assert_eq!(s[0].rows[4].value, "unknown");
    assert_eq!(s[0].rows[5].value, "none");
    assert_eq!(s[0].rows[6].value, "none");
}

#[test]
fn the_host_hints_get_their_own_section() {
    let snap = Snapshot {
        host_hints: vec![HostHintRow {
            title: "Wi-Fi power saving is on for wlan0.".to_string(),
            help: "This can add short stalls. Jafnify does not change it; turn it off in your network settings if stalls persist.".to_string(),
        }],
        ..Snapshot::default()
    };
    let s = sections(&snap, None);
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].title, "This computer");
    assert_eq!(s[1].rows.len(), 1);
    assert_eq!(s[1].rows[0].label, "Wi-Fi power saving is on for wlan0.");
    assert!(s[1].rows[0].value.starts_with("This can add short stalls."));
}

#[test]
fn nothing_detected_means_no_host_section() {
    let s = sections(&Snapshot::default(), None);
    assert!(s.iter().all(|section| section.title != "This computer"));
}

#[test]
fn the_connection_details_use_the_core_wording() {
    let snap = Snapshot {
        path: Some("Direct over hotspot".to_string()),
        health_line: Some("Direct over hotspot".to_string()),
        dns_through_tunnel: true,
        peer_endpoint: Some("10.174.190.72:9797".to_string()),
        ..Snapshot::default()
    };
    let rows = &sections(&snap, None)[0].rows;
    let labels = labels(rows);
    assert_eq!(
        labels,
        vec![
            "Status",
            "Path",
            "MTU",
            "DNS",
            "Phone address",
            "Pair id",
            "Last error"
        ]
    );
    assert_eq!(rows[1].value, "Direct over hotspot");
    assert_eq!(rows[2].value, "1280");
    assert_eq!(rows[3].value, "through the tunnel");
    assert_eq!(rows[4].value, "10.174.190.72:9797");
    assert!(
        !labels.contains(&"Link health"),
        "the verdict repeats the path"
    );
}

#[test]
fn a_connected_snapshot_shows_the_session_in_the_phone_order() {
    let snap = Snapshot {
        status_text: "Connected".to_string(),
        health_detail: Some("Last handshake 18 s ago".to_string()),
        peer_key_prefix: Some("nksAwHpu2seZ".to_string()),
        pair_id: Some("14e72b1d".to_string()),
        current_session: SessionStats {
            started_at: 1_700_000_000_000,
            connected_ms: 871_000,
            up_bytes: 2048,
            down_bytes: 4096,
            peer_endpoint: "10.174.190.72:9797".to_string(),
            ..SessionStats::default()
        },
        ..Snapshot::default()
    };
    let s = sections(&snap, Some("boom"));
    assert_eq!(
        labels(&s[0].rows),
        vec![
            "Status",
            "Path",
            "MTU",
            "DNS",
            "Phone address",
            "Last handshake",
            "Peer key",
            "Pair id",
            "Last error"
        ]
    );
    assert_eq!(s[0].rows[5].value, "18 s ago");
    assert_eq!(s[0].rows[8].value, "boom");
    assert_eq!(s[1].title, "This session");
    assert_eq!(
        labels(&s[1].rows),
        vec![
            "Started",
            "Connected for",
            "Down",
            "Up",
            "Average",
            "Peak",
            "Drops",
            "Reconnects",
            "Link losses",
            "Peer address"
        ]
    );
    assert_eq!(s[1].rows[1].value, "14:31");
    assert_eq!(s[1].rows[9].value, "10.174.190.72:9797");
}

#[test]
fn a_host_dns_override_is_a_warning_row() {
    let snap = Snapshot {
        dns_override: Some("System DNS override in /etc/x.conf".to_string()),
        ..Snapshot::default()
    };
    let rows = &sections(&snap, None)[0].rows;
    let last = rows.last().unwrap();
    assert_eq!(last.label, "System DNS");
    assert!(last.warn);
}

#[test]
fn history_rows_add_the_end_and_read_unknown_without_an_endpoint() {
    let r = SessionRecord::default();
    let rows = history_rows(&r);
    assert_eq!(rows[1].label, "Ended");
    assert_eq!(rows.last().unwrap().value, "unknown");
}

#[test]
fn the_handshake_lead_is_dropped_only_when_present() {
    assert_eq!(handshake_age("Last handshake 5 s ago"), "5 s ago");
    assert_eq!(handshake_age("none yet"), "none yet");
}
