//! The laptop hooks are run the way wg-quick runs them: `set -e`, then `eval`
//! of each PostUp / PostDown value (wireguard-tools, execute_hooks). Under
//! `set -e` the first failing simple command aborts the hook, so an optional
//! step must end in `|| true`, and only the kill switch rules may fail the
//! tunnel. The tools are stubs that
//! fail where the real ones do: no rule exists yet on the first start, and
//! systemd-resolved is not running.
//!
//! Unix only: these hooks run under /bin/sh with wg-quick's shell semantics.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use jafnify_core::config::{kill_switch_down, ConfigGenerator, InboundGuard, KillSwitch};

const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

fn stub(dir: &Path, name: &str, body: &str) {
    // Written under a temp name and renamed: never executed while still open for writing.
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, dir.join(name)).unwrap();
}

/// Runs one hook value as wg-quick does. `fail` makes any stubbed command whose
/// arguments contain it fail (empty: none). Returns whether the hook succeeded.
fn run_hook(hook: &str, fail: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    // Nothing is installed yet: deleting, flushing or removing fails.
    stub(
        dir.path(),
        "iptables",
        r#"case "$*" in *-D*|*-F*|*-X*) exit 1;; esac
case "$*" in *"$FAIL"*) [ -n "$FAIL" ] && exit 1;; esac
exit 0"#,
    );
    stub(
        dir.path(),
        "nft",
        r#"case "$*" in *delete*) exit 1;; esac
case "$*" in *"$FAIL"*) [ -n "$FAIL" ] && exit 1;; esac
exit 0"#,
    );
    stub(dir.path(), "ip", "exit 0");
    // systemd-resolved is not running: the client binary exists but fails.
    stub(dir.path(), "resolvectl", "exit 1");
    // Only the stubs are on PATH: a hook that names a real privileged tool
    // would fail here instead of touching the host.
    let path = dir.path().display().to_string();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(r#"set -e; (eval "$HOOK")"#)
        .env("HOOK", hook.replace("%i", "jafn0"))
        .env("FAIL", fail)
        .env("PATH", path);
    // Another test thread can briefly hold its fake script open for writing;
    // the shell then reports ETXTBSY before the stub is exec'd.
    let mut attempts = 0;
    loop {
        let out = cmd.output().unwrap();
        if out.status.success() {
            return true;
        }
        if attempts < 50 && String::from_utf8_lossy(&out.stderr).contains("Text file busy") {
            attempts += 1;
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        return false;
    }
}

fn joiner_up(guard: InboundGuard, resolved: bool, ks: Option<&KillSwitch>) -> String {
    ConfigGenerator::joiner_config_full(PRIV, PUB, "192.168.43.1:9797", guard, resolved, ks)
        .interface
        .post_up
        .unwrap()
}

fn kill_switch(iptables: bool) -> KillSwitch {
    KillSwitch {
        iptables,
        lan: "192.168.43.0/24".to_string(),
    }
}

#[test]
fn the_joiner_hooks_survive_a_host_without_systemd_resolved() {
    let up = joiner_up(InboundGuard::Iptables, true, None);
    assert!(run_hook(&up, ""), "PostUp failed: {up}");
}

#[test]
fn the_iptables_kill_switch_survives_its_first_start() {
    let ks = kill_switch(true);
    let up = joiner_up(InboundGuard::Iptables, false, Some(&ks));
    assert!(run_hook(&up, ""), "PostUp failed on the first start: {up}");
}

#[test]
fn the_nft_kill_switch_and_nft_guard_survive_their_first_start() {
    let ks = kill_switch(false);
    let up = joiner_up(InboundGuard::Nft, false, Some(&ks));
    assert!(run_hook(&up, ""), "PostUp failed on the first start: {up}");
}

#[test]
fn a_kill_switch_rule_that_cannot_be_installed_fails_the_tunnel() {
    // No guard here, so the only "-j DROP" is the kill switch's own.
    let ks = kill_switch(true);
    let up = joiner_up(InboundGuard::None, false, Some(&ks));
    assert!(
        !run_hook(&up, "-j DROP"),
        "a failed kill switch must fail closed"
    );
}

#[test]
fn removing_a_kill_switch_that_is_not_there_succeeds() {
    assert!(run_hook(&kill_switch_down(true), ""));
    assert!(run_hook(&kill_switch_down(false), ""));
}

#[test]
fn no_hook_has_a_hash_sign_because_wg_quick_strips_comments() {
    for guard in [
        InboundGuard::Iptables,
        InboundGuard::Nft,
        InboundGuard::None,
    ] {
        for resolved in [false, true] {
            let ks = kill_switch(guard != InboundGuard::Nft);
            let cfg = ConfigGenerator::joiner_config_full(
                PRIV,
                PUB,
                "192.168.43.1:9797",
                guard,
                resolved,
                Some(&ks),
            );
            for hook in [cfg.interface.post_up, cfg.interface.post_down]
                .into_iter()
                .flatten()
            {
                assert!(!hook.contains('#'), "{hook}");
            }
        }
    }
}
