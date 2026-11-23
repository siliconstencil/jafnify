use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use jafnify_linux::children::Children;

#[test]
fn finished_windows_are_reaped_and_running_windows_are_kept() {
    let mut children = Children::default();
    children.track(Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap());
    children.track(
        Command::new("sh")
            .arg("-c")
            .arg("sleep 0.4")
            .spawn()
            .unwrap(),
    );
    assert_eq!(children.len(), 2);

    sleep(Duration::from_millis(200));
    assert_eq!(children.reap(), 1);
    assert_eq!(children.len(), 1);

    sleep(Duration::from_millis(400));
    assert_eq!(children.reap(), 1);
    assert_eq!(children.len(), 0);
}
