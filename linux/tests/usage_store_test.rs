//! The Linux hourly usage store: line round trip, merge, cap, corruption,
//! atomic private writes, and the OFF path that creates no file.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::usage::UsageBucket;
use jafnify_linux::usage_store::UsageStore;

const H: i64 = 3_600_000;

fn bucket(hour: i64, up: u64, down: u64) -> UsageBucket {
    UsageBucket {
        hour_start_ms: hour,
        up_bytes: up,
        down_bytes: down,
        min_up_bps: 10,
        min_down_bps: 20,
        peak_up_bps: 30,
        peak_down_bps: 40,
        connected_ms: 60_000,
        samples: 3,
        network: NetworkKind::Wifi,
    }
}

#[test]
fn a_saved_bucket_round_trips_and_keeps_time_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.tsv");
    let mut store = UsageStore::load(&path);
    assert!(!path.exists(), "load alone must not create the file");

    store.push(bucket(2 * H, 20, 40));
    store.push(bucket(H, 10, 30));

    let loaded = UsageStore::load(&path);
    let buckets = loaded.buckets();
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].hour_start_ms, H);
    assert_eq!(buckets[0].up_bytes, 10);
    assert_eq!(buckets[0].down_bytes, 30);
    assert_eq!(buckets[1].hour_start_ms, 2 * H);
}

#[test]
fn the_same_hour_merges_instead_of_counting_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.tsv");
    let mut store = UsageStore::load(&path);

    store.push(bucket(H, 10, 30));
    store.push(bucket(H, 5, 7));

    let buckets = store.buckets();
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].up_bytes, 15);
    assert_eq!(buckets[0].down_bytes, 37);
    assert_eq!(buckets[0].samples, 6);
}

#[test]
fn the_cap_keeps_the_newest_720_hours() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.tsv");
    let mut store = UsageStore::load(&path);

    for hour in 0..721 {
        store.push(bucket(hour * H, 1, 2));
    }

    let loaded = UsageStore::load(&path);
    let buckets = loaded.buckets();
    assert_eq!(buckets.len(), 720);
    assert_eq!(buckets[0].hour_start_ms, H);
    assert_eq!(buckets[719].hour_start_ms, 720 * H);
}

#[test]
fn corrupt_lines_are_skipped_and_a_valid_line_survives() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.tsv");
    let valid = bucket(H, 10, 30).line();
    fs::write(&path, format!("not a line\n{valid}\nJFU1 1 hour=bad\n")).unwrap();

    let store = UsageStore::load(&path);
    assert_eq!(store.buckets().len(), 1);
    assert_eq!(store.buckets()[0].hour_start_ms, H);
}

#[test]
fn writes_are_owner_only_and_leave_no_temp_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("state");
    let path = dir.join("usage.tsv");
    let mut store = UsageStore::load(&path);
    store.push(bucket(H, 10, 30));

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "usage file mode");
    let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "usage dir mode");

    let leftovers: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}

#[test]
fn an_old_line_without_min_fields_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.tsv");
    fs::write(
        &path,
        "JFU1 1 hour=3600000 up=10 down=30 peak_up=30 peak_down=40 connected_ms=60000 samples=3 net=wifi\n",
    )
    .unwrap();

    let store = UsageStore::load(&path);
    assert_eq!(store.buckets().len(), 1);
    assert_eq!(store.buckets()[0].min_up_bps, 0);
    assert_eq!(store.buckets()[0].min_down_bps, 0);
}
