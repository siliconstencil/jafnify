use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jafnify_linux::autostart::{enable, write_unit_file, AutostartError};

fn stub(dir: &Path, body: &str) -> (PathBuf, PathBuf) {
    let log = dir.join("calls.log");
    let bin = dir.join("systemctl");
    fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

#[test]
fn unit_file_content_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.service");
    write_unit_file(&path, "/usr/bin/jafnify tray").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("[Unit]"));
    assert!(text.contains("After=graphical-session.target"));
    assert!(text.contains("PartOf=graphical-session.target"));
    assert!(text.contains("[Service]"));
    assert!(text.contains("ExecStart=/usr/bin/jafnify tray\n"));
    assert!(text.contains("ExecStopPost=/usr/bin/jafnify fallback\n"));
    assert!(text.contains("[Install]"));
    assert!(text.contains("WantedBy=graphical-session.target"));
    assert!(!text.contains("default.target"));
}

#[test]
fn the_masked_message_says_what_to_run() {
    assert_eq!(
        AutostartError::Masked.to_string(),
        "Autostart is blocked on this system (the jafnify user service is masked). To allow it, run: systemctl --user unmask jafnify.service"
    );
}

#[test]
fn a_unit_symlinked_to_dev_null_is_masked_and_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.service");
    std::os::unix::fs::symlink("/dev/null", &path).unwrap();
    let (bin, log) = stub(dir.path(), "exit 99\n");
    let err = enable(&path, "/usr/local/bin/jafnify tray", &bin).unwrap_err();
    assert!(matches!(err, AutostartError::Masked));
    assert_eq!(
        std::fs::read_link(&path).unwrap(),
        PathBuf::from("/dev/null")
    );
    assert!(!log.exists());
}

#[test]
fn is_enabled_masked_is_reported_and_enable_is_not_called() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.service");
    let (bin, log) = stub(
        dir.path(),
        "if [ \"$2\" = \"is-enabled\" ]; then echo masked; exit 1; fi\nexit 0\n",
    );
    let err = enable(&path, "/usr/local/bin/jafnify tray", &bin).unwrap_err();
    assert!(matches!(err, AutostartError::Masked));
    assert!(!path.exists());
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("--user is-enabled jafnify.service"));
    assert!(!calls.contains("--user enable"));
}

#[test]
fn enable_writes_the_tray_unit_and_does_not_start_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.service");
    let (bin, log) = stub(
        dir.path(),
        "if [ \"$2\" = \"is-enabled\" ]; then echo disabled; exit 1; fi\nexit 0\n",
    );
    enable(&path, "/usr/local/bin/jafnify tray", &bin).unwrap();
    let unit = fs::read_to_string(&path).unwrap();
    assert!(unit.contains("ExecStart=/usr/local/bin/jafnify tray\n"));
    assert!(unit.contains("ExecStopPost=/usr/local/bin/jafnify fallback\n"));
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("--user enable jafnify.service"));
    assert!(!calls.contains("--now"));
    assert!(!calls.contains(" start"));
}

#[test]
fn a_masked_word_on_stderr_does_not_mask_the_unit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.service");
    let (bin, _log) = stub(
        dir.path(),
        "if [ \"$2\" = \"is-enabled\" ]; then echo masked >&2; echo disabled; exit 1; fi\nexit 0\n",
    );
    enable(&path, "/usr/local/bin/jafnify tray", &bin).unwrap();
    assert!(path.exists());
}

#[test]
fn write_to_bad_dir_errors() {
    let err = write_unit_file(
        &PathBuf::from("/nonexistent-parent-dir/jafnify.service"),
        "/usr/bin/jafnify tray",
    )
    .unwrap_err();
    assert!(matches!(err, AutostartError::Io(_)));
}
