//! A fake script written by another test thread must not make exec flaky:
//! Linux returns ETXTBSY while any thread still holds the file open for write,
//! even briefly after the writer closed its own handle.

use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[test]
fn output_retries_a_script_that_is_open_for_writing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("hello.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf hello\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let hold = OpenOptions::new().write(true).open(&script).unwrap();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        drop(hold);
    });

    let out = jafnify_linux::exec::output(Command::new(&script)).unwrap();
    writer.join().unwrap();

    assert!(out.status.success());
    assert_eq!(out.stdout, b"hello");
}

#[test]
fn spawn_retries_a_script_that_is_open_for_writing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("spawned.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf spawned\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let hold = OpenOptions::new().write(true).open(&script).unwrap();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        drop(hold);
    });

    let mut cmd = Command::new(&script);
    cmd.stdout(Stdio::piped());
    let out = jafnify_linux::exec::spawn(cmd)
        .unwrap()
        .wait_with_output()
        .unwrap();
    writer.join().unwrap();

    assert!(out.status.success());
    assert_eq!(out.stdout, b"spawned");
}
