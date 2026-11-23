use clap::Parser;
use jafnify_linux::cli::{Cli, Command};

#[test]
fn cli_parses_phone_stats() {
    let cli = Cli::try_parse_from(["jafnify", "phone-stats"]).unwrap();
    assert!(matches!(
        cli,
        Cli {
            command: Command::PhoneStats,
            ..
        }
    ));
}

#[test]
fn cli_parses_start_with_code() {
    let cli = Cli::try_parse_from(["jafnify", "start", "--code", "1234"]).unwrap();
    assert!(matches!(cli, Cli { command: Command::Start { code: Some(c) }, .. } if c == "1234"));
}

#[test]
fn cli_takes_the_accept_terms_flag_before_or_after_the_command() {
    let before = Cli::try_parse_from(["jafnify", "--accept-terms", "start"]).unwrap();
    assert!(before.accept_terms);
    let after =
        Cli::try_parse_from(["jafnify", "pair", "--code", "4321", "--accept-terms"]).unwrap();
    assert!(after.accept_terms);
    assert!(
        !Cli::try_parse_from(["jafnify", "start"])
            .unwrap()
            .accept_terms
    );
}

#[test]
fn cli_parses_join_without_code() {
    let cli = Cli::try_parse_from(["jafnify", "join"]).unwrap();
    assert!(matches!(
        cli,
        Cli {
            command: Command::Join { code: None },
            ..
        }
    ));
}

#[test]
fn cli_parses_stop_status_peers() {
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "stop"]).unwrap(),
        Cli {
            command: Command::Stop,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "status"]).unwrap(),
        Cli {
            command: Command::Status,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "peers"]).unwrap(),
        Cli {
            command: Command::Peers,
            ..
        }
    ));
}

#[test]
fn cli_parses_the_hours_command() {
    let cli = Cli::try_parse_from(["jafnify", "hours"]).unwrap();
    assert!(matches!(
        cli,
        Cli {
            command: Command::Hours,
            ..
        }
    ));
}

#[test]
fn cli_parses_pair_with_code_flag_and_forget() {
    let cli = Cli::try_parse_from(["jafnify", "pair", "--code", "4321"]).unwrap();
    assert!(matches!(cli, Cli { command: Command::Pair { code }, .. } if code == "4321"));
    assert!(Cli::try_parse_from(["jafnify", "pair"]).is_err());
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "forget"]).unwrap(),
        Cli {
            command: Command::Forget,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "uninstall"]).unwrap(),
        Cli {
            command: Command::Uninstall,
            ..
        }
    ));
}

// The real binary, with its state under a temp XDG_CONFIG_HOME. These runs
// never reach the network: they stop before any socket or root command. The
// temp PATH's `pkexec` stub and the guard env make a missed assumption fail
// loudly instead of prompting on the host.
fn jafnify(xdg: &std::path::Path, args: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let bin = xdg.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pkexec = bin.join("pkexec");
    std::fs::write(&pkexec, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&pkexec, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    std::process::Command::new(env!("CARGO_BIN_EXE_jafnify"))
        .args(args)
        .env("XDG_CONFIG_HOME", xdg)
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("PATH", path)
        .env("JAFNIFY_TEST_NO_ROOT", "1")
        .env_remove("JAFNIFY_CODE")
        .output()
        .unwrap()
}

#[test]
fn join_and_start_without_code_and_without_pairing_say_use_pair() {
    let xdg = tempfile::tempdir().unwrap();
    // Accepted already: the pairing check is what is under test.
    jafnify_core::terms::record_acceptance(&xdg.path().join("jafnify"), 1).unwrap();
    for cmd in ["join", "start"] {
        let out = jafnify(xdg.path(), &[cmd]);
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("no paired device - use jafnify pair"),
            "{cmd}: {err}"
        );
    }
}

#[test]
fn forget_removes_the_stored_pair() {
    use jafnify_core::keyexchange::KeyPair;
    use jafnify_core::known::pair_record;
    use jafnify_core::state::Role;
    use jafnify_linux::settings::{load_state, save_state, LinuxStateFile};

    let xdg = tempfile::tempdir().unwrap();
    let path = xdg.path().join("jafnify/state.toml");
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let rec = pair_record(&own, &peer.public_key_b64(), Role::Joiner, None, 1).unwrap();
    save_state(
        &path,
        &LinuxStateFile {
            peers: vec![rec],
            ..Default::default()
        },
    )
    .unwrap();

    let out = jafnify(xdg.path(), &["forget"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(load_state(&path).unwrap().peers.is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&own.private_key_b64()));

    // Nothing paired any more.
    jafnify_core::terms::record_acceptance(&xdg.path().join("jafnify"), 1).unwrap();
    let out = jafnify(xdg.path(), &["join"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no paired device"));
}

#[test]
fn cli_parses_firewall_rule_and_allow() {
    use jafnify_linux::cli::FirewallCommand;
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "firewall", "rule"]).unwrap(),
        Cli {
            command: Command::Firewall {
                action: FirewallCommand::Rule
            },
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "firewall", "allow"]).unwrap(),
        Cli {
            command: Command::Firewall {
                action: FirewallCommand::Allow { yes: false }
            },
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "firewall", "allow", "--yes"]).unwrap(),
        Cli {
            command: Command::Firewall {
                action: FirewallCommand::Allow { yes: true }
            },
            ..
        }
    ));
    assert!(Cli::try_parse_from(["jafnify", "firewall"]).is_err());
}

#[test]
fn cli_parses_probe() {
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "probe", "--code", "4321"]).unwrap(),
        Cli { command: Command::Probe { code, seconds: 30 }, .. } if code == "4321"
    ));
    assert!(matches!(
        Cli::try_parse_from(["jafnify", "probe", "--code", "4321", "--seconds", "5"]).unwrap(),
        Cli {
            command: Command::Probe { seconds: 5, .. },
            ..
        }
    ));
    // The key exchange needs a code.
    assert!(Cli::try_parse_from(["jafnify", "probe"]).is_err());
}

#[test]
fn probe_with_a_malformed_code_stops_before_any_socket() {
    let xdg = tempfile::tempdir().unwrap();
    let out = jafnify(xdg.path(), &["probe", "--code", "12ab"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Invalid code"));
}

#[test]
fn firewall_allow_without_consent_changes_nothing() {
    // No tty answer (stdin closed): the prompt reads nothing, which is a no.
    // The state dir is empty and `ufw` is not asked for anything.
    let xdg = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_jafnify"))
        .args(["firewall", "allow"])
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("XDG_STATE_HOME", xdg.path().join("state"))
        .env("PATH", "/nonexistent")
        .env("JAFNIFY_TEST_NO_ROOT", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Either nothing to scope the rule to (no `ip` on this PATH) or a declined
    // prompt: never a success.
    assert!(!out.status.success(), "{all}");
    assert!(!all.contains("Rule added"), "{all}");
}

#[test]
fn cli_rejects_unknown_subcommand() {
    assert!(Cli::try_parse_from(["jafnify", "frobnicate"]).is_err());
}

#[test]
fn status_into_a_closed_pipe_does_not_panic() {
    // Live: `jafnify status | head -2` panicked with
    // "failed printing to stdout: Broken pipe".
    let xdg = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_jafnify"))
        .arg("status")
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("XDG_STATE_HOME", xdg.path().join("state"))
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("PATH", "/nonexistent")
        .env("JAFNIFY_TEST_NO_ROOT", "1")
        .stdin(std::process::Stdio::null())
        .stdout(writer)
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
}
