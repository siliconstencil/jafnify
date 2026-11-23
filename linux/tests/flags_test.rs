//! `flags.toml` is a separate store from `state.toml`: a 0.1.1 build never
//! reads it, unknown keys survive a write, and a missing or unreadable file
//! only ever means the core defaults.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jafnify_core::flags::{find, platform, Bucket, FeatureFlag, EXPERIMENTAL_NOTE, FLAGS};
use jafnify_linux::flags::{
    sections, sections_from, writable, writable_in, FlagRefusal, FlagStore, Placement, FILE_NAME,
};

fn flag_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join(FILE_NAME)
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_missing_file_gives_every_core_default_and_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    let store = FlagStore::load(&path);
    for flag in FLAGS {
        assert_eq!(store.get(flag.id), flag.default, "{}", flag.id);
    }
    assert!(!path.exists(), "load must not create flags.toml");
}

#[test]
fn all_flags_at_default_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    FlagStore::default().save(&path).unwrap();
    let loaded = FlagStore::load(&path);
    for flag in FLAGS {
        assert_eq!(loaded.get(flag.id), flag.default, "{}", flag.id);
    }
}

#[test]
fn one_flag_on_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    let mut store = FlagStore::load(&path);
    store.set("usage_statistics", true);
    store.save(&path).unwrap();

    let loaded = FlagStore::load(&path);
    assert!(loaded.get("usage_statistics"));
    for flag in FLAGS.iter().filter(|flag| flag.id != "usage_statistics") {
        assert_eq!(loaded.get(flag.id), flag.default, "{}", flag.id);
    }
}

#[test]
fn a_missing_key_reads_the_core_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    fs::write(&path, "start_after_restart = true\n").unwrap();

    let store = FlagStore::load(&path);
    assert!(store.get("start_after_restart"));
    for flag in FLAGS.iter().filter(|flag| flag.id != "start_after_restart") {
        assert_eq!(store.get(flag.id), flag.default, "{}", flag.id);
    }
}

#[test]
fn an_unknown_key_is_not_a_declared_row_and_does_not_affect_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    fs::write(&path, "flag_from_a_newer_version = true\n").unwrap();

    let store = FlagStore::load(&path);
    assert!(find("flag_from_a_newer_version").is_none());
    for flag in FLAGS {
        assert_eq!(store.get(flag.id), flag.default, "{}", flag.id);
    }
}

#[test]
fn an_unknown_key_survives_a_load_and_save_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    fs::write(&path, "flag_from_a_newer_version = true\n").unwrap();

    let mut store = FlagStore::load(&path);
    store.set("unicast_search", true);
    store.save(&path).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("flag_from_a_newer_version = true"),
        "unknown key dropped: {text}"
    );
    assert!(
        text.contains("unicast_search = true"),
        "known key missing: {text}"
    );
}

#[test]
fn a_0_1_1_config_directory_without_flags_toml_loads_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let state = "peers = []\n\n[settings]\nrole = \"Joiner\"\nautostart = false\n";
    let state_path = dir.path().join("state.toml");
    fs::write(&state_path, state).unwrap();

    let store = FlagStore::load(&flag_path(&dir));
    for flag in FLAGS {
        assert_eq!(store.get(flag.id), flag.default, "{}", flag.id);
    }
    assert!(!flag_path(&dir).exists());
    assert_eq!(fs::read_to_string(&state_path).unwrap(), state);
}

#[test]
fn an_unreadable_file_gives_defaults_and_never_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    fs::write(&path, "not [valid toml ==").unwrap();

    let store = FlagStore::load(&path);
    for flag in FLAGS {
        assert_eq!(store.get(flag.id), flag.default, "{}", flag.id);
    }
}

#[test]
fn resave_tightens_an_existing_0644_file_and_leaves_its_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    fs::write(&path, "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

    FlagStore::default().save(&path).unwrap();
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(dir.path()), 0o755);
}

#[test]
fn a_stale_temp_file_does_not_keep_its_old_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    let tmp = dir.path().join(format!(".{FILE_NAME}.tmp"));
    fs::write(&tmp, "stale").unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();

    FlagStore::default().save(&path).unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn save_is_atomic_and_0600_in_a_0700_dir_it_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg").join(FILE_NAME);
    FlagStore::default().save(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains("tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

#[test]
fn effective_lists_every_declared_flag_with_the_stored_or_core_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = flag_path(&dir);
    let mut store = FlagStore::load(&path);
    store.set("usage_statistics", true);

    let effective = store.effective();
    assert_eq!(effective.len(), FLAGS.len());
    for flag in FLAGS {
        let expected = if flag.id == "usage_statistics" {
            true
        } else {
            flag.default
        };
        assert_eq!(effective.get(flag.id), Some(&expected), "{}", flag.id);
    }
}

fn synthetic(id: &'static str, bucket: Bucket, platforms: u8, wired: bool) -> FeatureFlag {
    FeatureFlag {
        id,
        bucket,
        default: false,
        platforms,
        label: "Synthetic row",
        help: "Synthetic help.",
        wired,
    }
}

#[test]
fn set_settings_may_write_only_declared_wired_rows_for_linux() {
    assert_eq!(
        writable("no_such_flag", platform::LINUX),
        Err(FlagRefusal::Unknown)
    );
    assert_eq!(
        writable("unicast_search", platform::LINUX),
        Err(FlagRefusal::NotWired),
        "every real row that is still unwired"
    );
    assert_eq!(
        writable("notify_connection_change", platform::LINUX).map(|flag| flag.id),
        Ok("notify_connection_change"),
        "the desktop notification row is wired"
    );

    let android_only = synthetic("android_only", Bucket::Settings, platform::ANDROID, true);
    assert_eq!(
        writable_in(&[android_only], "android_only", platform::LINUX),
        Err(FlagRefusal::NotSupported)
    );
    let linux_row = synthetic("linux_row", Bucket::Settings, platform::LINUX, true);
    assert_eq!(
        writable_in(&[linux_row], "linux_row", platform::LINUX).map(|flag| flag.id),
        Ok("linux_row")
    );
    let unwired = synthetic("unwired", Bucket::Settings, platform::LINUX, false);
    assert_eq!(
        writable_in(&[unwired], "unwired", platform::LINUX),
        Err(FlagRefusal::NotWired)
    );
}

#[test]
fn only_the_wired_linux_rows_are_rendered_today() {
    let defaults = sections(&BTreeMap::new());
    assert_eq!(defaults.len(), 2);
    assert_eq!(defaults[0].bucket, Bucket::Settings);
    let ids: Vec<&str> = defaults[0].rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, ["notify_connection_change"]);
    assert!(defaults[0].rows.iter().all(|row| !row.value));
    assert_eq!(defaults[1].bucket, Bucket::Experimental);
    let ids: Vec<&str> = defaults[1].rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, ["best_spot", "good_time_notice", "good_hours"]);
    assert!(defaults[1].rows.iter().all(|row| !row.value));

    let all_on: BTreeMap<String, bool> = FLAGS.iter().map(|f| (f.id.to_string(), true)).collect();
    let sections = sections(&all_on);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].bucket, Bucket::Settings);
    let ids: Vec<&str> = sections[0].rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, ["notify_connection_change"]);
    assert_eq!(sections[1].bucket, Bucket::Experimental);
    let ids: Vec<&str> = sections[1].rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, ["best_spot", "good_time_notice", "good_hours"]);
    assert!(sections[1].rows.iter().all(|row| row.value));
    assert!(
        sections[1]
            .rows
            .iter()
            .all(|row| row.id == "best_spot"
                || row.id == "good_time_notice"
                || row.id == "good_hours"),
        "an unwired row reached the window"
    );
}

#[test]
fn a_bucket_with_no_rendered_row_is_not_shown() {
    let table = [synthetic(
        "android_only",
        Bucket::Settings,
        platform::ANDROID,
        true,
    )];
    assert!(sections_from(&table, &BTreeMap::new()).is_empty());
}

#[test]
fn wired_rows_are_grouped_in_bucket_order_and_start_collapsed() {
    let table = [
        synthetic("exp", Bucket::Experimental, platform::LINUX, true),
        synthetic("set", Bucket::Settings, platform::LINUX, true),
        synthetic("adv", Bucket::Advanced, platform::LINUX, true),
        synthetic("unwired", Bucket::Advanced, platform::LINUX, false),
        synthetic("android_only", Bucket::Settings, platform::ANDROID, true),
    ];
    let values = BTreeMap::from([("set".to_string(), true), ("adv".to_string(), false)]);

    let out = sections_from(&table, &values);
    let buckets: Vec<Bucket> = out.iter().map(|section| section.bucket).collect();
    assert_eq!(
        buckets,
        [Bucket::Settings, Bucket::Advanced, Bucket::Experimental,]
    );
    assert_eq!(out[0].title, "Settings");
    assert!(!out[0].collapsed_by_default);
    assert_eq!(out[0].placement, Placement::MainList);
    assert_eq!(out[1].title, "Advanced");
    assert!(out[1].collapsed_by_default);
    assert_eq!(out[1].placement, Placement::AdvancedBody);
    assert_eq!(out[2].title, "Experimental");
    assert!(out[2].collapsed_by_default);
    assert_eq!(out[2].placement, Placement::OwnHeader);
    assert_eq!(out[2].note, Some(EXPERIMENTAL_NOTE));
    assert_eq!(out[0].note, None);

    let settings_ids: Vec<&str> = out[0].rows.iter().map(|row| row.id).collect();
    assert_eq!(settings_ids, ["set"]);
    assert!(out[0].rows[0].value);
    let advanced_ids: Vec<&str> = out[1].rows.iter().map(|row| row.id).collect();
    assert_eq!(advanced_ids, ["adv"], "the unwired row is dropped");
    assert!(!out[1].rows[0].value);
    assert_eq!(out[2].rows[0].id, "exp");
    assert!(!out[2].rows[0].value, "a missing value reads the default");
}

#[test]
fn a_non_boolean_key_from_a_newer_build_does_not_reset_the_other_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flags.toml");
    std::fs::write(
        &path,
        "usage_statistics = true\nfuture_level = 3\nfuture_name = \"x\"\nkeep_screen_on = \"yes\"\n",
    )
    .unwrap();
    let mut store = FlagStore::load(&path);
    assert!(store.get("usage_statistics"));
    // A wrong-typed value for a declared row reads as the core default.
    assert!(!store.get("keep_screen_on"));
    store.set("unicast_search", true);
    store.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("future_level = 3"), "{text}");
    assert!(text.contains("future_name = \"x\""), "{text}");
    let again = FlagStore::load(&path);
    assert!(again.get("usage_statistics"));
    assert!(again.get("unicast_search"));
}

#[test]
fn a_flags_file_from_a_newer_build_is_read_by_the_old_parser_and_the_other_flags_survive_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flags.toml");
    let mut store = FlagStore::load(&path);
    store.set("drop_history", true);
    store.set("phone_heat_warning", true);
    store.set("good_time_notice", true);
    store.set("usage_statistics", true);
    store.set("best_spot", true);
    store.set("device_panel", true);
    store.set("keep_screen_on", true);
    store.set("good_hours", true);
    store.save(&path).unwrap();

    // The file is a flat table of booleans, so a 0.2.2 build reads it as is.
    let text = fs::read_to_string(&path).unwrap();
    let table: toml::Table = text.parse().expect("0.2.2 can parse this file");
    assert_eq!(table["drop_history"].as_bool(), Some(true));
    assert_eq!(table["phone_heat_warning"].as_bool(), Some(true));
    assert_eq!(table["good_time_notice"].as_bool(), Some(true));
    assert_eq!(table["usage_statistics"].as_bool(), Some(true));
    assert_eq!(table["best_spot"].as_bool(), Some(true));
    assert_eq!(table["device_panel"].as_bool(), Some(true));
    assert_eq!(table["keep_screen_on"].as_bool(), Some(true));
    assert_eq!(table["good_hours"].as_bool(), Some(true));

    // The new good-hours file sits next to the flags: a build that does not
    // know it starts with it there and leaves it alone.
    let hours_path = dir.path().join("good_hours.tsv");
    fs::write(
        &hours_path,
        "JFG1 1 hour=1700000000000 n=12 rtt=42 g5=100 sig=3\n",
    )
    .unwrap();

    // The old build does not know the new rows: it drops them and keeps the
    // rest.
    let mut older = table.clone();
    older.remove("drop_history");
    older.remove("phone_heat_warning");
    older.remove("good_time_notice");
    older.remove("best_spot");
    older.remove("device_panel");
    older.remove("keep_screen_on");
    older.remove("good_hours");
    fs::write(&path, toml::to_string(&older).unwrap()).unwrap();

    let mut store = FlagStore::load(&path);
    assert!(store.get("usage_statistics"));
    // A standard row is always on now, even when the key is missing: an
    // update never turns off a row that used to be a switch.
    assert!(store.get("drop_history"));
    assert!(store.get("phone_heat_warning"));
    assert!(!store.get("good_time_notice"));
    assert!(!store.get("best_spot"));
    assert!(!store.get("device_panel"));
    assert!(!store.get("keep_screen_on"));
    assert!(!store.get("good_hours"));
    store.set("best_spot", true);
    store.save(&path).unwrap();

    let again = FlagStore::load(&path);
    assert!(again.get("usage_statistics"));
    assert!(again.get("best_spot"));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("usage_statistics = true"), "{text}");
    assert!(text.contains("best_spot = true"), "{text}");
    assert!(
        hours_path.exists(),
        "the old build must leave the good-hours file alone"
    );
}
