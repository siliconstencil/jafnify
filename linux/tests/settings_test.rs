// linux/tests/settings_test.rs
use std::fs;

use jafnify_core::names::display_name;
use jafnify_core::state::{Peer, Role};
use jafnify_linux::settings::{
    load_state, save_state, LinuxStateFile, Settings, SettingsError, WindowGeometry,
    MAX_WINDOW_SIDE, MIN_WINDOW_SIDE,
};
use jafnify_linux::theme::ThemeMode;

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

#[test]
fn settings_default_is_joiner_no_autostart() {
    let s = Settings::default();
    assert_eq!(s.role, Role::Joiner);
    assert!(!s.autostart);
}

#[test]
fn the_tray_is_visible_by_default() {
    assert!(!Settings::default().tray_only_when_closed);
}

#[test]
fn state_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let state = LinuxStateFile {
        peers: vec![peer("a"), peer("b")],
        settings: Settings {
            role: Role::Sharer,
            autostart: true,
            kill_switch: false,
            tray_only_when_closed: true,
            details_expanded: false,
            advanced_expanded: false,
            theme: ThemeMode::System,
            window: WindowGeometry::default(),
        },
    };
    save_state(&path, &state).unwrap();
    let loaded = load_state(&path).unwrap();
    assert_eq!(loaded.peers, state.peers);
    assert_eq!(loaded.settings, state.settings);
    assert!(loaded.settings.tray_only_when_closed);
}

#[test]
fn missing_file_loads_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load_state(&dir.path().join("nope.toml")).unwrap();
    assert!(loaded.peers.is_empty());
    assert_eq!(loaded.settings, Settings::default());
}

#[test]
fn corrupt_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(&path, "not [valid toml ==").unwrap();
    let err = load_state(&path).unwrap_err();
    assert!(matches!(err, SettingsError::Corrupt(_)));
}

#[test]
fn save_is_atomic_no_temp_left() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    save_state(&path, &LinuxStateFile::default_state()).unwrap();
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

// The state file can hold a pair's private key.
#[test]
fn state_file_is_0600_in_a_0700_dir_it_created() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg/state.toml");
    save_state(&path, &LinuxStateFile::default_state()).unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[test]
fn resave_tightens_an_existing_0644_file_and_leaves_its_dir() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(&path, "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    save_state(&path, &LinuxStateFile::default_state()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn stale_temp_file_does_not_keep_its_old_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let tmp = dir.path().join(".state.toml.tmp");
    fs::write(&tmp, "stale").unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
    save_state(&path, &LinuxStateFile::default_state()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn peer_with_key_and_endpoint_round_trips_and_old_files_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let mut p = peer("a");
    p.private_key = Some("priv".into());
    p.last_endpoint = Some("10.1.0.2:9797".into());
    let state = LinuxStateFile {
        peers: vec![p],
        settings: Settings::default(),
    };
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap(), state);
    fs::write(
        &path,
        "[[peers]]\nid = \"x\"\nname = \"n\"\npublic_key = \"k\"\nrole = \"Sharer\"\n\n[settings]\nrole = \"Joiner\"\nautostart = false\n",
    )
    .unwrap();
    let loaded = load_state(&path).unwrap();
    assert_eq!(loaded.peers[0].private_key, None);
}

#[test]
fn load_state_upgrades_a_legacy_key_prefix_name_but_not_a_custom_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let public_key = "EXAMPLEKEYAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let mut legacy = peer("0123456789abcdef0123456789abcdef");
    legacy.name = public_key.chars().take(12).collect();
    legacy.public_key = public_key.to_string();
    legacy.private_key = Some("EXAMPLE-not-a-real-private-key".into());
    let mut custom = legacy.clone();
    custom.name = "My phone".into();
    let state = LinuxStateFile {
        peers: vec![legacy.clone(), custom],
        settings: Settings::default(),
    };
    save_state(&path, &state).unwrap();

    let loaded = load_state(&path).unwrap();
    assert_eq!(loaded.peers[0].name, display_name(Role::Sharer, &legacy.id));
    assert_eq!(loaded.peers[1].name, "My phone");
    // Only the name changes.
    assert_eq!(loaded.peers[0].public_key, legacy.public_key);
    assert_eq!(loaded.peers[0].private_key, legacy.private_key);
}

#[test]
fn unknown_field_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(&path, "bogus = 1\n").unwrap();
    let err = load_state(&path).unwrap_err();
    assert!(matches!(err, SettingsError::Corrupt(_)));
}

#[test]
fn details_and_advanced_start_collapsed() {
    let s = Settings::default();
    assert!(!s.details_expanded);
    assert!(!s.advanced_expanded);
}

#[test]
fn theme_defaults_to_system() {
    assert_eq!(Settings::default().theme, ThemeMode::System);
}

#[test]
fn old_settings_without_theme_load_as_system() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(
        &path,
        "peers = []\n[settings]\nrole = \"Joiner\"\nautostart = false\n",
    )
    .unwrap();
    let loaded = load_state(&path).unwrap();
    assert_eq!(loaded.settings.theme, ThemeMode::System);
}

#[test]
fn theme_round_trips_dark_and_light() {
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let state = LinuxStateFile {
            peers: vec![],
            settings: Settings {
                theme: mode,
                ..Settings::default()
            },
        };
        save_state(&path, &state).unwrap();
        assert_eq!(load_state(&path).unwrap().settings.theme, mode);
    }
}

#[test]
fn an_invalid_theme_is_rejected_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    fs::write(
        &path,
        "peers = []\n[settings]\nrole = \"Joiner\"\nautostart = false\ntheme = \"blue\"\n",
    )
    .unwrap();
    let err = load_state(&path).unwrap_err();
    assert!(matches!(err, SettingsError::Corrupt(_)));
}

#[test]
fn window_geometry_round_trips_and_old_files_have_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let window = WindowGeometry {
        width: 640,
        height: 720,
        x: Some(120),
        y: Some(80),
    };
    let state = LinuxStateFile {
        peers: vec![],
        settings: Settings {
            window,
            ..Settings::default()
        },
    };
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap().settings.window, window);

    fs::write(
        &path,
        "peers = []\n[settings]\nrole = \"Joiner\"\nautostart = false\n",
    )
    .unwrap();
    assert_eq!(
        load_state(&path).unwrap().settings.window,
        WindowGeometry::default()
    );
}

#[test]
fn window_geometry_is_sanitized_before_restore() {
    assert!(WindowGeometry::default().sanitized().is_none());
    assert!(WindowGeometry {
        width: MIN_WINDOW_SIDE - 1,
        height: 600,
        ..WindowGeometry::default()
    }
    .sanitized()
    .is_none());
    assert!(WindowGeometry {
        width: MAX_WINDOW_SIDE + 1,
        height: 600,
        ..WindowGeometry::default()
    }
    .sanitized()
    .is_none());
    assert!(WindowGeometry {
        width: 640,
        height: 480,
        ..WindowGeometry::default()
    }
    .sanitized()
    .is_some());
    let far = WindowGeometry {
        width: 640,
        height: 480,
        x: Some(i32::MAX),
        y: Some(4),
    }
    .sanitized()
    .unwrap();
    assert_eq!(far.x, None);
    assert_eq!(far.y, Some(4));
}

#[test]
fn view_expansion_round_trips_and_old_files_default_to_collapsed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let state = LinuxStateFile {
        peers: vec![],
        settings: Settings {
            details_expanded: true,
            advanced_expanded: true,
            ..Settings::default()
        },
    };
    save_state(&path, &state).unwrap();
    let loaded = load_state(&path).unwrap();
    assert!(loaded.settings.details_expanded);
    assert!(loaded.settings.advanced_expanded);

    fs::write(
        &path,
        "peers = []\n[settings]\nrole = \"Joiner\"\nautostart = false\n",
    )
    .unwrap();
    let old = load_state(&path).unwrap();
    assert!(!old.settings.details_expanded);
    assert!(!old.settings.advanced_expanded);
}
