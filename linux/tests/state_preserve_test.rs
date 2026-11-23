//! A state file the app cannot read is kept, never overwritten: the pairings
//! in it must survive a downgrade or a damaged file.

use jafnify_linux::settings::{load_state_or_preserve, save_state, LinuxStateFile};

#[test]
fn an_unreadable_state_file_is_moved_aside_with_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let text = "peers = []\n\n[settings]\nrole = \"Joiner\"\nfield_from_a_newer_version = 1\n";
    std::fs::write(&path, text).unwrap();
    let state = load_state_or_preserve(&path);
    assert!(state.peers.is_empty());
    assert!(
        !path.exists(),
        "the unreadable file must not stay where a save would replace it"
    );
    let kept = dir.path().join("state.toml.unreadable");
    assert_eq!(std::fs::read_to_string(kept).unwrap(), text);
}

#[test]
fn a_second_unreadable_file_does_not_replace_the_first_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    std::fs::write(&path, "not toml [").unwrap();
    load_state_or_preserve(&path);
    std::fs::write(&path, "also not toml [").unwrap();
    load_state_or_preserve(&path);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state.toml.unreadable")).unwrap(),
        "not toml ["
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state.toml.unreadable.1")).unwrap(),
        "also not toml ["
    );
}

#[test]
fn a_readable_state_file_stays_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    save_state(&path, &LinuxStateFile::default()).unwrap();
    load_state_or_preserve(&path);
    assert!(path.exists());
    assert!(!dir.path().join("state.toml.unreadable").exists());
}

#[test]
fn a_missing_state_file_gives_defaults_and_no_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let state = load_state_or_preserve(&path);
    assert!(state.peers.is_empty());
    assert!(!dir.path().join("state.toml.unreadable").exists());
}
