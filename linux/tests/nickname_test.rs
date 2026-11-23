//! The local device name: kept in its own file, never in `state.toml` (which a
//! 0.1.1 build reads with `deny_unknown_fields`), never in the text the peer
//! reads, the session, the export or the measurements.

use jafnify_linux::local_name;
use jafnify_linux::settings::Settings;

/// The `Settings` keys a 0.1.1 build accepts. A new key here would make a
/// downgraded build set `state.toml` aside and start unpaired.
const SETTINGS_KEYS_0_1_1: &[&str] = &[
    "role",
    "autostart",
    "kill_switch",
    "tray_only_when_closed",
    "details_expanded",
    "advanced_expanded",
    "theme",
    "window",
];

#[test]
fn state_toml_settings_keep_the_0_1_1_keys_only() {
    let text = toml::to_string(&Settings::default()).unwrap();
    let table: toml::Table = text.parse().unwrap();
    for key in table.keys() {
        assert!(
            SETTINGS_KEYS_0_1_1.contains(&key.as_str()),
            "Settings writes `{key}`, which a 0.1.1 build rejects"
        );
    }
}

#[test]
fn the_name_round_trips_through_its_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(local_name::FILE_NAME);
    assert_eq!(local_name::load(&path), "");
    assert_eq!(local_name::save(&path, "  Desk\u{7}  ").unwrap(), "Desk");
    assert_eq!(local_name::load(&path), "Desk");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn an_empty_name_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(local_name::FILE_NAME);
    local_name::save(&path, "Desk").unwrap();
    assert_eq!(local_name::save(&path, "   ").unwrap(), "");
    assert!(!path.exists());
    assert_eq!(local_name::load(&path), "");
}

#[test]
fn an_unreadable_file_is_an_empty_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(local_name::FILE_NAME);
    std::fs::write(&path, "not = [toml").unwrap();
    assert_eq!(local_name::load(&path), "");
}

#[test]
fn the_peer_facing_and_exported_text_carry_no_name() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in [
        "export.rs",
        "session.rs",
        "phone_stats.rs",
        "flags.rs",
        "details.rs",
        "settings.rs",
        "app.rs",
    ] {
        let text = std::fs::read_to_string(src.join(name)).unwrap();
        assert!(
            !text.contains("nickname") && !text.contains("local_name"),
            "{name} mentions the local name"
        );
    }
}
