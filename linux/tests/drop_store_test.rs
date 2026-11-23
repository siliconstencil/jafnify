use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use jafnify_linux::drop_store::{DropStore, MAX_LINES};

fn line(index: usize) -> String {
    format!("JFD1 1 drop at={index} reason=peer-left")
}

#[test]
fn a_missing_file_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = DropStore::load(&dir.path().join("drops.tsv"));
    assert!(store.records().is_empty());
    assert!(store.lines().is_empty());
}

#[test]
fn an_appended_line_is_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    store.append(line(1)).unwrap();
    assert_eq!(store.lines(), &[line(1)]);

    let reread = DropStore::load(&path);
    assert_eq!(reread.lines(), &[line(1)]);
}

#[test]
fn the_file_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    store.append(line(1)).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the drop file must be private");
}

#[test]
fn the_file_keeps_the_newest_lines_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    for index in 0..MAX_LINES * 3 {
        store.append(line(index)).unwrap();
    }
    assert_eq!(store.lines().len(), MAX_LINES);
    assert_eq!(store.lines()[MAX_LINES - 1], line(MAX_LINES * 3 - 1));

    let text = fs::read_to_string(&path).unwrap();
    let written: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // The file is compacted when it grows past twice the bound, the same way
    // the connection log does it.
    assert!(written.len() <= 2 * MAX_LINES, "the file must be compacted");
    assert_eq!(
        written[written.len() - 1],
        line(MAX_LINES * 3 - 1),
        "the newest line must survive"
    );
}

#[test]
fn a_compacted_file_is_appended_to_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    for index in 0..MAX_LINES * 2 + 1 {
        store.append(line(index)).unwrap();
    }
    // The compaction wrote the newest lines. The next line belongs in that
    // file: the store must not write the whole file again for every drop.
    let before = fs::metadata(&path).unwrap().ino();
    store.append(line(MAX_LINES * 2 + 1)).unwrap();
    let after = fs::metadata(&path).unwrap().ino();
    assert_eq!(before, after, "the file was written again from scratch");
}

#[test]
fn a_write_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing/drops.tsv");
    let mut store = DropStore::load(&path);
    assert!(
        store.append(line(1)).is_err(),
        "a failed write must be reported"
    );
}

#[test]
fn a_corrupt_line_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    fs::write(&path, "not a drop line\n").unwrap();
    let store = DropStore::load(&path);
    assert!(store.records().is_empty());
}

#[test]
fn clearing_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    store.append(line(1)).unwrap();
    assert!(path.exists());

    store.clear();
    assert!(!path.exists());
    assert!(store.lines().is_empty());
    assert!(store.records().is_empty());
}

#[test]
fn the_file_holds_only_the_lines_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    store
        .append("JFD1 1 drop at=1 reason=peer-left".to_string())
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.trim(), "JFD1 1 drop at=1 reason=peer-left");
    assert!(!text.contains("10.233.0"));
    assert!(!text.contains("peer "));
}

#[test]
fn the_store_never_writes_outside_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drops.tsv");
    let mut store = DropStore::load(&path);
    store.append(line(1)).unwrap();
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| name == "drops.tsv" || name.starts_with('.')),
        "unexpected files: {names:?}"
    );
    let _ = Path::new("");
}
