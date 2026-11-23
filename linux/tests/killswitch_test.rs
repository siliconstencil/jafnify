//! The laptop kill switch as the Linux app and CLI see it. Off by default;
//! the setting survives restarts; the CLI turns it on and off.

use jafnify_linux::app::AppBuilder;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

fn stub_pkexec(dir: &Path) -> PathBuf {
    let p = dir.join("pkexec");
    std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn the_kill_switch_is_off_by_default_and_an_old_state_file_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cfg")).unwrap();
    std::fs::write(
        dir.path().join("cfg/state.toml"),
        "[settings]\nrole = \"joiner\"\nautostart = false\n",
    )
    .unwrap();
    let app = AppBuilder::new(dir.path().join("cfg"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    assert!(!app.kill_switch());
}

#[test]
fn the_kill_switch_setting_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    app.set_kill_switch(true).unwrap();
    drop(app);
    let app = AppBuilder::new(dir.path().join("cfg"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    assert!(app.kill_switch());
}

#[test]
fn the_cli_turns_the_kill_switch_on_and_off() {
    let xdg = tempfile::tempdir().unwrap();
    jafnify_core::terms::record_acceptance(&xdg.path().join("jafnify"), 1).unwrap();
    let bin = tempfile::tempdir().unwrap();
    stub_pkexec(bin.path());
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |arg: &str| {
        Process::new(env!("CARGO_BIN_EXE_jafnify"))
            .args(["kill-switch", arg])
            .env("XDG_CONFIG_HOME", xdg.path())
            .env("HOME", xdg.path())
            .env("PATH", &path)
            .env("JAFNIFY_TEST_NO_ROOT", "1")
            .output()
            .unwrap()
    };
    let on = run("on");
    assert!(
        on.status.success(),
        "{}",
        String::from_utf8_lossy(&on.stderr)
    );
    assert!(String::from_utf8_lossy(&on.stdout).contains("Kill switch on"));
    let app = AppBuilder::new(xdg.path().join("jafnify"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    assert!(app.kill_switch());
    let off = run("off");
    assert!(
        off.status.success(),
        "{}",
        String::from_utf8_lossy(&off.stderr)
    );
    let app = AppBuilder::new(xdg.path().join("jafnify"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    assert!(!app.kill_switch());
}
