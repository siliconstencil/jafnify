//! The diagnostic journal file: one UTC line per record and rotation.

use jafnify_linux::diag::{Diag, KEEP, MAX_BYTES};

#[test]
fn open_creates_parent_directories_and_flushes_each_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/diag.log");
    let mut diag = Diag::open(&path).unwrap();
    diag.stats(1_793_450_096_789, &[("worker_a", "1")]).unwrap();

    // Readable while the writer is still open: the record was flushed.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("ts=2026-10-31T12:34:56.789Z kind=STATS worker_a=1\n"),
        "{text}"
    );
    assert!(path.parent().unwrap().is_dir());
}

#[test]
fn an_event_is_tagged_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diag.log");
    let mut diag = Diag::open(&path).unwrap();
    diag.event(0, "peer_reset", &[("reason", "timeout")])
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with(
            "ts=1970-01-01T00:00:00.000Z kind=EVENT event=peer_reset reason=timeout\n"
        ),
        "{text}"
    );
}

#[test]
fn a_secret_passed_to_the_writer_never_reaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diag.log");
    let key = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
    let mut diag = Diag::open(&path).unwrap();
    diag.event(0, "probe", &[("key", key)]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(key), "{text}");
    assert!(!text.contains('B'), "{text}");
    assert!(text.contains("key=[redacted]"), "{text}");
}

#[test]
fn rotation_keeps_five_files_and_writes_the_next_record_to_a_fresh_active_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diag.log");
    let mut diag = Diag::open(&path).unwrap();

    // Big enough to rotate quickly, small enough to stay well under MAX_BYTES.
    let filler = "x".repeat(16 * 1024);
    let mut written = 0usize;
    while !dir.path().join("diag.4.log").exists() {
        diag.stats(1, &[("filler", filler.as_str())]).unwrap();
        written += 1;
        assert!(written < 10_000, "rotation never reached diag.4.log");
    }

    diag.stats(2, &[("marker", "after_rotation")]).unwrap();

    let active = std::fs::read_to_string(&path).unwrap();
    assert!(active.contains("marker=after_rotation"), "{active}");
    let first = std::fs::read_to_string(dir.path().join("diag.1.log")).unwrap();
    assert!(!first.contains("marker=after_rotation"), "{first}");

    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let logs = names
        .iter()
        .filter(|name| name.starts_with("diag") && name.ends_with(".log"))
        .count();
    assert_eq!(logs, KEEP, "{names:?}");
    assert!(!dir.path().join("diag.5.log").exists());
    assert!(std::fs::metadata(&path).unwrap().len() <= MAX_BYTES);
}
