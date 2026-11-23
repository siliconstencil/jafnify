//! The Linux session rows and record privacy. The tracker rules themselves
//! moved to `jafnify-core/tests/session_tracker_test.rs`.

use jafnify_linux::details::session_rows;
use jafnify_linux::session::{SessionHistory, SessionTracker};

const T0: i64 = 1_700_000_000_000;

#[test]
fn a_known_rx_tx_pair_reads_as_laptop_upload_and_download() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 900, 100, "10.1.0.2:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 1_900, 600, "", "");

    let stats = tracker.current();
    assert_eq!(stats.down_bytes, 1_000, "rx is the laptop download");
    assert_eq!(stats.up_bytes, 500, "tx is the laptop upload");

    let rows = session_rows(&stats);
    let down = rows.iter().find(|row| row.label == "Down").unwrap();
    let up = rows.iter().find(|row| row.label == "Up").unwrap();
    assert_eq!(down.value, "1.0 KB");
    assert_eq!(up.value, "500 B");
}

#[test]
fn a_history_file_is_read_newest_first_even_when_it_was_written_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    std::fs::write(
        &path,
        "[[sessions]]\nid=\"old\"\nstarted_at=100\nended_at=150\n\
         [[sessions]]\nid=\"new\"\nstarted_at=300\nended_at=350\n\
         [[sessions]]\nid=\"mid\"\nstarted_at=200\nended_at=250\n",
    )
    .unwrap();
    let history = SessionHistory::load(&path);
    let ids: Vec<String> = history
        .recent(10)
        .into_iter()
        .map(|record| record.id)
        .collect();
    assert_eq!(ids, ["new", "mid", "old"]);
}

#[test]
fn a_serialized_record_has_no_key_shaped_base64() {
    let mut tracker = SessionTracker::new("joiner", T0);
    tracker.sample(T0, "Connected", 0, 0, "10.1.0.2:9797", "");
    tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
    let record = tracker.finish(T0 + 2_000, "app_exit");

    let text = toml::to_string(&record).unwrap();
    let key_shape = regex::Regex::new(r"[A-Za-z0-9+/]{42,44}=").unwrap();
    assert!(!key_shape.is_match(&text), "key-shaped text in {text}");
}
