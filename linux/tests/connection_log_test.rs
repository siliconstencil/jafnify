//! The Linux connection-log file: save, persistence and the snapshot shape.

use std::os::unix::fs::PermissionsExt;

use jafnify_linux::connection_log::{self, ConnectionLog, MAX_LINES};
use jafnify_linux::control::Snapshot;

/// 2023-11-14 22:13:20 UTC.
const T: i64 = 1_700_000_000_000;

#[test]
fn a_serialized_snapshot_never_carries_a_key_shaped_string() {
    let key = format!("{}=", "B".repeat(43));
    let mut log = ConnectionLog::new();
    log.push("drop", &format!("peer {key} vanished"), T);

    let snapshot = Snapshot {
        connection_log: log.tail(MAX_LINES),
        ..Snapshot::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains(&key), "{json}");
    assert!(json.contains("[redacted]"), "{json}");
    assert!(!json.contains("PrivateKey"));
    assert!(!json.contains("private_key"));
}

#[test]
fn an_old_snapshot_payload_loads_with_empty_new_fields() {
    let old = r#"{"state":"idle","status_text":"Not connected","paired_name":null,
        "paired":false,"denied":false,"retry_reason":null,"no_peer":false,
        "connected_secs":null,"traffic":null,"last_connected_ago":null,
        "retry_secs_left":null,"firewall":null,"firewall_iface":null,
        "last_error":null,"role":"joiner","autostart":false,"kill_switch":false,
        "tray_only_when_closed":false,"terms_ok":true,"ufw_active":false,
        "pair_id":null,"peer_key_prefix":null}"#;
    let snapshot: Snapshot = serde_json::from_str(old).unwrap();
    assert!(snapshot.connection_log.is_empty());
    assert!(snapshot.session_history.is_empty());
    assert_eq!(snapshot.current_session, Default::default());
    assert_eq!(snapshot.firewall_iface, None);
}

#[test]
fn a_snapshot_with_the_old_firewall_subnet_field_still_loads() {
    let old = r#"{"state":"searching","status_text":"Searching...","paired_name":null,
        "paired":false,"denied":false,"retry_reason":null,"no_peer":false,
        "connected_secs":null,"traffic":null,"last_connected_ago":null,
        "retry_secs_left":null,"firewall":"ufw","firewall_subnet":"10.20.89.0/24",
        "last_error":null,"role":"joiner","autostart":false,"kill_switch":false,
        "tray_only_when_closed":false,"terms_ok":true,"ufw_active":true,
        "pair_id":null,"peer_key_prefix":null}"#;
    let snapshot: Snapshot = serde_json::from_str(old).unwrap();
    assert_eq!(snapshot.firewall.as_deref(), Some("ufw"));
    // The old daemon has no interface field; serde default keeps it loading.
    assert_eq!(snapshot.firewall_iface, None);
}

#[test]
fn save_log_uses_the_stamped_name_mode_and_exact_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = connection_log::save_log_at(dir.path(), "one\ntwo", T).unwrap();

    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "jafnify-log-20231114-221320.txt"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn save_log_uniquifies_without_overwriting_in_the_same_second() {
    let dir = tempfile::tempdir().unwrap();
    let first = connection_log::save_log_at(dir.path(), "first", T).unwrap();
    let second = connection_log::save_log_at(dir.path(), "second", T).unwrap();

    assert_eq!(
        first.file_name().unwrap().to_string_lossy(),
        "jafnify-log-20231114-221320.txt"
    );
    assert_eq!(
        second.file_name().unwrap().to_string_lossy(),
        "jafnify-log-20231114-221320-1.txt"
    );
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
}

#[test]
fn save_log_does_not_follow_a_preexisting_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "safe").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("jafnify-log-20231114-221320.txt"))
        .unwrap();

    let path = connection_log::save_log_at(dir.path(), "log", T).unwrap();
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "jafnify-log-20231114-221320-1.txt"
    );
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "safe");
}

#[test]
fn save_log_reports_a_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert!(connection_log::save_log_at(&missing, "log", T).is_err());
}

#[test]
fn a_persistent_log_shows_earlier_runs_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connection.log");
    {
        let mut log = ConnectionLog::persistent(path.clone());
        assert!(log.is_empty());
        log.push(
            "session",
            &connection_log::session_started_line("joiner"),
            T,
        );
        log.push("state", "state Connected -> Retry", T + 1_000);
    }
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the log file is private");

    let mut log = ConnectionLog::persistent(path.clone());
    assert_eq!(log.len(), 2);
    assert_eq!(log.last_kind(), Some(connection_log::EARLIER_KIND));
    assert!(log.tail(2)[0].ends_with("session started (joiner)"));
    log.push(
        "session",
        &connection_log::session_started_line("joiner"),
        T + 9_000,
    );
    let again = ConnectionLog::persistent(path);
    assert_eq!(again.len(), 3);
    assert_eq!(again.tail(3), log.tail(3));
}

#[test]
fn a_persistent_log_keeps_one_line_per_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connection.log");
    let mut log = ConnectionLog::persistent(path.clone());
    log.push("drop", "first\nsecond\r\nthird", T);
    let again = ConnectionLog::persistent(path);
    assert_eq!(again.len(), 1);
    assert!(
        again.tail(1)[0].ends_with("first second  third"),
        "{:?}",
        again.tail(1)
    );
}

#[test]
fn a_persistent_log_file_stays_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connection.log");
    let mut log = ConnectionLog::persistent(path.clone());
    let total = 3 * MAX_LINES + 7;
    for i in 0..total {
        log.push("state", &format!("line {i}"), T + i as i64);
    }
    let lines = std::fs::read_to_string(&path).unwrap().lines().count();
    assert!(lines <= 2 * MAX_LINES, "the file grew to {lines} lines");
    let again = ConnectionLog::persistent(path);
    assert_eq!(again.len(), MAX_LINES);
    assert!(again.tail(1)[0].ends_with(&format!("line {}", total - 1)));
    assert_eq!(again.tail(MAX_LINES), log.tail(MAX_LINES));
}

#[test]
fn an_unwritable_log_file_still_logs_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be: every write fails.
    let path = dir.path().join("connection.log");
    std::fs::create_dir(&path).unwrap();
    let mut log = ConnectionLog::persistent(path);
    log.push("state", "state Idle -> Searching", T);
    assert_eq!(log.len(), 1);
}
