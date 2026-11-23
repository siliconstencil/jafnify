//! Linux frontends and the use-at-your-own-risk terms.
//! Sharing or joining starts only
//! after the terms are accepted: the CLI asks (or takes --accept-terms), the GUI
//! shows the dialog.

use jafnify_core::config::{ConfigGenerator, InboundGuard};
use jafnify_core::state::Role;
use jafnify_linux::app::build_config;
use jafnify_linux::terms::{cli_gate, Gate};

#[test]
fn an_accepted_user_starts_without_being_asked_again() {
    assert_eq!(cli_gate(true, false, false), Gate::Proceed);
    assert_eq!(cli_gate(true, false, true), Gate::Proceed);
}

#[test]
fn the_accept_flag_records_the_acceptance_and_starts() {
    assert_eq!(cli_gate(false, true, false), Gate::Record);
    assert_eq!(cli_gate(false, true, true), Gate::Record);
}

#[test]
fn an_interactive_run_asks_first() {
    assert_eq!(cli_gate(false, false, true), Gate::Ask);
}

#[test]
fn a_non_interactive_run_without_the_flag_is_refused() {
    assert_eq!(cli_gate(false, false, false), Gate::Refuse);
}

#[test]
fn the_cli_help_ends_with_the_terms_notice() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_jafnify"))
        .arg("--help")
        .env("JAFNIFY_TEST_NO_ROOT", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Use at your own risk"), "{text}");
    assert!(text.contains("--accept-terms"), "{text}");
}

#[test]
fn a_non_interactive_start_is_refused_until_the_terms_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_jafnify"))
        .env("XDG_CONFIG_HOME", dir.path())
        .env("HOME", dir.path())
        .env("JAFNIFY_TEST_NO_ROOT", "1")
        .args(["start", "--code", "4321"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--accept-terms"), "{err}");
    // Refused means nothing was stored.
    assert!(!dir.path().join("jafnify").join("terms.toml").exists());
}

#[test]
fn resolved_routing_is_chosen_only_when_resolvectl_exists_and_resolved_runs() {
    use jafnify_linux::app::resolved_present;
    assert!(resolved_present(&|name| name == "resolvectl", true));
    assert!(!resolved_present(&|_| false, true));
    // A client binary without a running systemd-resolved is not enough.
    assert!(!resolved_present(&|name| name == "resolvectl", false));
}

#[test]
fn an_empty_resolved_directory_is_not_a_running_resolver() {
    use jafnify_linux::app::resolved_running;
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !resolved_running(dir.path()),
        "the directory alone is not enough"
    );
    std::fs::write(dir.path().join("io.systemd.Resolve"), b"").unwrap();
    assert!(
        resolved_running(dir.path()),
        "the varlink socket is the proof"
    );
}

#[test]
fn the_joiner_config_carries_the_chosen_guard() {
    let kp = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let pk = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
    let with = build_config(
        Role::Joiner,
        kp,
        pk,
        None,
        InboundGuard::Iptables,
        false,
        None,
    );
    assert!(with.interface.post_up.unwrap().starts_with("iptables "));
    let without = build_config(Role::Joiner, kp, pk, None, InboundGuard::None, false, None);
    let up = without.interface.post_up.unwrap();
    assert!(!up.contains("iptables") && !up.contains("nft"), "{up}");
    // The sharer never has a guard.
    let sharer = build_config(
        Role::Sharer,
        kp,
        pk,
        None,
        InboundGuard::Iptables,
        true,
        None,
    );
    assert_eq!(sharer, ConfigGenerator::sharer_config(kp, pk));
}
