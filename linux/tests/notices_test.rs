//! The third-party notices script is offline, POSIX sh, and shipped by every
//! packaging path (tarball, .deb, APK asset).

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(repo().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn the_notices_script_is_valid_posix_shell_and_runs_offline() {
    let script = repo().join("tools/release/notices.sh");
    let out = std::process::Command::new("sh")
        .args(["-n", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = read("tools/release/notices.sh");
    assert!(text.contains("cargo"), "{text}");
    assert!(text.contains("metadata"), "{text}");
    assert!(text.contains("--offline"), "the notices must not fetch");
    assert!(text.contains("--filter-platform"), "one platform per graph");
    assert!(text.contains("normal"), "dev/build deps must not ship");
    // The two non-permissive crates are allowed with a reason, in the script.
    assert!(text.contains("epaint_default_fonts 0.37.0"), "{text}");
    assert!(text.contains("option-ext 0.2.0"), "{text}");
}

#[test]
fn every_packaging_path_ships_the_notices() {
    let linux = read("tools/release/linux.sh");
    assert!(linux.contains("tools/release/notices.sh"), "{linux}");
    assert!(linux.contains("THIRD_PARTY_NOTICES.txt"), "{linux}");
    assert!(
        linux.contains("usr/share/doc/jafnify/THIRD_PARTY_NOTICES.txt"),
        "the .deb must install the notices: {linux}"
    );

    let android = read("tools/release/android.sh");
    assert!(android.contains("tools/release/notices.sh"), "{android}");
    assert!(
        android.contains("src/main/assets/THIRD_PARTY_NOTICES.txt"),
        "the APK needs the notices asset: {android}"
    );
}

#[test]
fn the_android_about_row_opens_the_asset_in_a_plain_view() {
    let about = read("app/app/src/main/java/com/siliconstencil/jafnify/AboutActivity.java");
    assert!(about.contains("bNotices"), "{about}");
    assert!(about.contains("Native.thirdPartyNoticesTitle()"), "{about}");
    assert!(about.contains("NoticesActivity.class"), "{about}");
    let notices = read("app/app/src/main/java/com/siliconstencil/jafnify/NoticesActivity.java");
    assert!(notices.contains("THIRD_PARTY_NOTICES.txt"), "{notices}");
    assert!(notices.contains("ScrollView") || notices.contains("tNotices"));
}
