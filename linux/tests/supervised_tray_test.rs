//! The window starts the tray as a supervised user unit, so a crash with the
//! kill switch off falls back to normal routing (a live gap found earlier
//! on a clean VM: kill -9 left the route pointing into a dead jafn0).

use std::path::Path;

use jafnify_linux::control::{supervised_tray_args, tray_unit_name, unit_is_running};

#[test]
fn the_tray_unit_runs_fallback_after_any_exit_and_restarts_on_failure() {
    let daemon = Path::new("/usr/bin/jafnify");
    let args = supervised_tray_args(daemon, &[]);
    let unit = format!("--unit={}", tray_unit_name());
    assert!(args.contains(&"--user".to_string()), "{args:?}");
    assert!(args.contains(&"--collect".to_string()), "{args:?}");
    assert!(args.contains(&unit), "{args:?}");
    assert!(
        args.contains(&"ExecStopPost=/usr/bin/jafnify fallback".to_string()),
        "{args:?}"
    );
    assert!(args.contains(&"Restart=on-failure".to_string()), "{args:?}");
    let tail: Vec<&str> = args.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(tail, vec!["tray", "/usr/bin/jafnify", "--"]);
}

#[test]
fn desktop_variables_are_passed_before_the_command() {
    let env = vec![("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string())];
    let args = supervised_tray_args(Path::new("/usr/bin/jafnify"), &env);
    let set = args
        .iter()
        .position(|a| a == "--setenv=WAYLAND_DISPLAY=wayland-0");
    let sep = args.iter().position(|a| a == "--");
    assert!(set.is_some() && set < sep, "{args:?}");
}

#[test]
fn the_unit_name_follows_the_flavor() {
    assert_eq!(
        tray_unit_name(),
        format!("{}-tray", jafnify_linux::flavor::NAME)
    );
}

#[test]
fn a_restarting_unit_counts_as_running() {
    assert!(unit_is_running("active\n"));
    assert!(unit_is_running("activating"));
    assert!(!unit_is_running("inactive"));
    assert!(!unit_is_running("failed"));
    assert!(!unit_is_running(""));
}
