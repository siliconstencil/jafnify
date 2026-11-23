use jafnify_core::state::{Peer, Role, StateError, StateStore, TomlStateStore};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn peer(id: &str) -> Peer {
    Peer {
        id: id.to_string(),
        name: format!("device-{id}"),
        public_key: "k".to_string(),
        role: Role::Sharer,
        last_connected: Some(1),
        private_key: None,
        last_endpoint: None,
    }
}

fn known_peer() -> Peer {
    Peer {
        private_key: Some("EXAMPLE-not-a-real-private-key".to_string()),
        last_endpoint: Some("192.168.43.1:9797".to_string()),
        ..peer("pair")
    }
}

#[test]
fn save_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let store = TomlStateStore::new(path.clone());
    let peers = vec![peer("a"), peer("b")];
    store.save(&peers).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded, peers);
}

#[test]
fn missing_file_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = TomlStateStore::new(dir.path().join("nope.toml"));
    assert_eq!(store.load().unwrap(), Vec::<Peer>::new());
}

#[test]
fn corrupt_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(&path, "not [valid toml ==").unwrap();
    let store = TomlStateStore::new(path);
    assert!(matches!(store.load(), Err(StateError::Corrupt(_))));
}

#[test]
fn save_is_atomic_no_temp_left() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let store = TomlStateStore::new(path.clone());
    store.save(&[peer("x")]).unwrap();
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

#[test]
fn known_peer_round_trips_with_private_key_and_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = TomlStateStore::new(dir.path().join("state.toml"));
    let peers = vec![known_peer()];
    store.save(&peers).unwrap();
    assert_eq!(store.load().unwrap(), peers);
}

// State files written before the known-peer fields existed must still load.
#[test]
fn old_state_toml_without_new_fields_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(
        &path,
        "[[peers]]\nid = \"old\"\nname = \"phone\"\npublic_key = \"k\"\nrole = \"Sharer\"\nlast_connected = 5\n",
    )
    .unwrap();
    let loaded = TomlStateStore::new(path).load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "old");
    assert_eq!(loaded[0].private_key, None);
    assert_eq!(loaded[0].last_endpoint, None);
}

#[test]
fn unknown_peer_field_is_still_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(
        &path,
        "[[peers]]\nid = \"a\"\nname = \"n\"\npublic_key = \"k\"\nrole = \"Sharer\"\nbogus = 1\n",
    )
    .unwrap();
    assert!(matches!(
        TomlStateStore::new(path).load(),
        Err(StateError::Corrupt(_))
    ));
}

#[cfg(unix)]
#[test]
fn state_file_is_written_0600_in_a_0700_dir() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("jafnify");
    let path = state_dir.join("state.toml");
    TomlStateStore::new(path.clone())
        .save(&[known_peer()])
        .unwrap();
    let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let dir_mode = fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "state file mode {file_mode:o}");
    assert_eq!(dir_mode, 0o700, "state dir mode {dir_mode:o}");
}

// The file holds a private key: it must never exist with wider permissions,
// not even for an instant (temp file is created 0600, not chmod-ed later).
#[cfg(unix)]
#[test]
fn temp_file_is_created_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    // A stale, world-readable temp file left by a crashed writer.
    let stale = dir.path().join(".state.toml.tmp");
    fs::write(&stale, "stale").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
    TomlStateStore::new(path.clone())
        .save(&[known_peer()])
        .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn resave_tightens_an_existing_0644_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(&path, "peers = []\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    TomlStateStore::new(path.clone())
        .save(&[known_peer()])
        .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

// An existing user-chosen directory (e.g. $HOME-relative config dir) must
// not be silently re-permissioned: only a dir we create is 0700.
#[cfg(unix)]
#[test]
fn save_tightens_a_dir_it_creates_but_leaves_existing_dir_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    TomlStateStore::new(dir.path().join("state.toml"))
        .save(&[peer("x")])
        .unwrap();
    let mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}
