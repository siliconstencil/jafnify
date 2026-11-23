//! Completed-session storage: retention, order, permissions and privacy.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jafnify_linux::session::{
    ReconnectRecord, SessionHistory, SessionRecord, SessionStats,
};

fn record(id: &str, started_at: i64) -> SessionRecord {
    SessionRecord {
        id: id.to_string(),
        role: "joiner".to_string(),
        started_at,
        last_activity_at: started_at + 1_000,
        ended_at: started_at + 2_000,
        connected_ms: 2_000,
        up_bytes: 80,
        down_bytes: 300,
        avg_up_bps: 40,
        avg_down_bps: 150,
        peak_up_bps: 50,
        peak_down_bps: 200,
        drops: 1,
        link_downs: 0,
        link_ups: 0,
        reconnects: vec![ReconnectRecord {
            disconnected_at: started_at + 500,
            detected_at: started_at + 1_000,
            connected_at: started_at + 1_500,
            duration_ms: 1_000,
            reason: "no data for 2 s".to_string(),
        }],
        peer_endpoint: "10.1.0.2:9797".to_string(),
        end_reason: "stopped".to_string(),
    }
}

fn stats(record: &SessionRecord) -> SessionStats {
    SessionStats {
        id: record.id.clone(),
        role: record.role.clone(),
        started_at: record.started_at,
        last_activity_at: record.last_activity_at,
        connected_ms: record.connected_ms,
        up_bytes: record.up_bytes,
        down_bytes: record.down_bytes,
        avg_up_bps: record.avg_up_bps,
        avg_down_bps: record.avg_down_bps,
        peak_up_bps: record.peak_up_bps,
        peak_down_bps: record.peak_down_bps,
        drops: record.drops,
        link_downs: record.link_downs,
        link_ups: record.link_ups,
        reconnects: record.reconnects.clone(),
        peer_endpoint: record.peer_endpoint.clone(),
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_missing_file_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let history = SessionHistory::load(&dir.path().join("sessions.toml"));
    assert!(history.recent(10).is_empty());
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let mut history = SessionHistory::load(&path);
    history.push(record("a", 1_000));
    history.save(&path).unwrap();

    let loaded = SessionHistory::load(&path);
    assert_eq!(loaded.recent(10), vec![record("a", 1_000)]);
}

#[test]
fn recent_is_newest_first_and_honours_the_limit() {
    let mut history = SessionHistory::default();
    history.push(record("a", 1_000));
    history.push(record("b", 2_000));
    history.push(record("c", 3_000));

    let newest = history.recent(2);
    assert_eq!(newest.len(), 2);
    assert_eq!(newest[0].id, "c");
    assert_eq!(newest[1].id, "b");
    assert_eq!(history.recent(10).len(), 3);
    assert!(history.recent(0).is_empty());
}

#[test]
fn fifty_one_pushes_keep_the_newest_fifty() {
    let mut history = SessionHistory::default();
    for i in 0..51 {
        history.push(record(&format!("id-{i}"), i as i64 * 1_000));
    }

    let kept = history.recent(100);
    assert_eq!(kept.len(), 50);
    assert_eq!(kept[0].id, "id-50");
    assert_eq!(kept[49].id, "id-1");
    assert!(kept.iter().all(|r| r.id != "id-0"));
}

#[test]
fn saved_file_is_0600_in_a_0700_directory_it_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg/sessions.toml");
    let mut history = SessionHistory::default();
    history.push(record("a", 1_000));
    history.save(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[test]
fn a_corrupt_file_loads_empty_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    fs::write(&path, "not [valid toml ==").unwrap();

    let history = SessionHistory::load(&path);
    assert!(history.recent(10).is_empty());
}

#[test]
fn a_saved_file_has_no_key_shaped_base64() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let mut history = SessionHistory::default();
    history.push(record("a", 1_000));
    history.save(&path).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    let key_shape = regex::Regex::new(r"[A-Za-z0-9+/]{42,44}=").unwrap();
    assert!(!key_shape.is_match(&text), "key-shaped text in {text}");
}

#[test]
fn load_never_writes_and_ignores_the_live_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let live = record("live", 1_000);
    let mut history = SessionHistory::default();
    history.set_live(stats(&live));
    history.save(&path).unwrap();
    let before = fs::read_to_string(&path).unwrap();
    let before_mtime = fs::metadata(&path).unwrap().modified().unwrap();

    let loaded = SessionHistory::load(&path);
    assert!(loaded.recent(10).is_empty(), "the live record is not a row");
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before_mtime);
}

#[test]
fn recover_unfinished_closes_the_live_record_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let live = record("live", 1_000);
    let mut history = SessionHistory::default();
    history.set_live(stats(&live));
    history.save(&path).unwrap();

    assert!(SessionHistory::recover_unfinished(&path).unwrap());
    let kept = SessionHistory::load(&path).recent(10);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].id, "live");
    assert_eq!(kept[0].end_reason, "App closed");
    assert_eq!(kept[0].ended_at, live.last_activity_at);

    // The live slot is gone, so a second daemon start appends nothing.
    assert!(!SessionHistory::recover_unfinished(&path).unwrap());
    assert_eq!(SessionHistory::load(&path).recent(10).len(), 1);
}

#[test]
fn a_live_record_with_an_id_already_in_the_history_is_not_appended_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let mut history = SessionHistory::default();
    history.push(record("same", 1_000));
    history.set_live(stats(&record("same", 1_000)));
    history.save(&path).unwrap();

    assert!(!SessionHistory::recover_unfinished(&path).unwrap());
    let kept = SessionHistory::load(&path).recent(10);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].end_reason, "stopped");
}

#[test]
fn a_completed_push_clears_the_live_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let mut history = SessionHistory::default();
    history.set_live(stats(&record("live", 1_000)));
    history.push(record("done", 2_000));
    history.save(&path).unwrap();

    let loaded = SessionHistory::load(&path);
    let kept = loaded.recent(10);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].id, "done");
    assert_eq!(kept[0].end_reason, "stopped");
}

#[test]
fn a_live_record_without_activity_is_not_written_to_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    let mut live = record("live", 1_000);
    live.connected_ms = 0;
    live.up_bytes = 0;
    live.down_bytes = 0;
    live.drops = 0;
    live.reconnects.clear();
    let mut history = SessionHistory::default();
    history.set_live(stats(&live));
    history.save(&path).unwrap();

    assert!(!SessionHistory::recover_unfinished(&path).unwrap());
    let loaded = SessionHistory::load(&path);
    assert!(loaded.recent(10).is_empty());
    // The empty live record is cleared from the file.
    assert!(!fs::read_to_string(&path).unwrap().contains("live"));
}
