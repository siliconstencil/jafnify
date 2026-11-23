//! While Connected, jafn0's own wg-quick rule must be there. If it is missing
//! (another process removed it, or a previous fallback was left behind), the
//! app hands routing back to the tunnel. Everything here is a fake tool.

use std::path::{Path, PathBuf};

use jafnify_linux::app::{restore_needed, tunnel_rule_installed};

/// Serialises the tests in this file. Each one writes fake tool scripts and then
/// runs them; a fork from a parallel test can still hold a just-written script
/// open for writing, and exec then fails with ETXTBSY, which the rule check
/// reads as "no decision". Holding one lock per test closes that window.
fn exec_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn wg(dir: &Path, out: &str) -> PathBuf {
    script(dir, "wg", &format!("#!/bin/sh\nprintf '%s\\n' '{out}'\n"))
}

fn ip(dir: &Path, name: &str, rules: &str) -> PathBuf {
    script(dir, name, &format!("#!/bin/sh\nprintf '%s\\n' '{rules}'\n"))
}

#[test]
fn the_rule_check_needs_both_the_mark_and_jafn0s_rule() {
    let _exec = exec_lock();
    let dir = tempfile::tempdir().unwrap();
    let wg = wg(dir.path(), "0xca6c");
    let yes = ip(
        dir.path(),
        "ip-yes",
        "32765:\\tnot from all fwmark 0xca6c/0xffff lookup 51820",
    );
    let other = ip(
        dir.path(),
        "ip-other",
        "250:\\tnot from all fwmark 0xc000/0xffff lookup 51820",
    );
    let none = ip(dir.path(), "ip-none", "32766:\\tfrom all lookup main");
    assert_eq!(tunnel_rule_installed(&wg, &yes), Some(true));
    assert_eq!(tunnel_rule_installed(&wg, &other), Some(false));
    assert_eq!(tunnel_rule_installed(&wg, &none), Some(false));
}

#[test]
fn an_off_or_failed_mark_is_no_decision() {
    let _exec = exec_lock();
    let dir = tempfile::tempdir().unwrap();
    let rules = ip(
        dir.path(),
        "ip",
        "32765:\\tnot from all fwmark 0xca6c/0xffff lookup 51820",
    );
    assert_eq!(tunnel_rule_installed(&wg(dir.path(), "off"), &rules), None);
    assert_eq!(
        tunnel_rule_installed(&dir.path().join("missing-wg"), &rules),
        None
    );
    assert_eq!(
        tunnel_rule_installed(&wg(dir.path(), "0xca6c"), &dir.path().join("missing-ip")),
        None
    );
}

#[test]
fn a_connected_app_restores_only_a_known_missing_rule() {
    let _exec = exec_lock();
    assert!(restore_needed(true, false, Some(false)));
    assert!(
        !restore_needed(true, true, Some(false)),
        "a restore is pending"
    );
    assert!(
        !restore_needed(true, false, Some(true)),
        "the rule is there"
    );
    assert!(!restore_needed(true, false, None), "the state is unknown");
    assert!(!restore_needed(false, false, Some(false)), "not connected");
}

#[test]
fn an_adopted_tunnel_records_routing_that_was_already_down() {
    // F4: the previous process fell back before it exited, so the adopted
    // tunnel starts with its rule already gone. That is a known state, not a
    // WARN "missing while connected".
    use jafnify_linux::app::adopted_routing_down;
    assert!(adopted_routing_down(Some(false)));
    assert!(!adopted_routing_down(Some(true)));
    assert!(!adopted_routing_down(None), "the state is unknown");
}

/// Routing follows liveness: one pure decision, tested here without root.
#[test]
fn routing_follows_tunnel_liveness() {
    let _exec = exec_lock();
    use jafnify_core::timing::{LIVENESS_DEAD_AFTER, MIN_ROUTE_TOGGLE};
    use jafnify_linux::app::{route_decision, RouteAction, RouteInputs};
    use std::time::Duration;
    let base = RouteInputs {
        kill_switch: false,
        tunnel_present: true,
        silent: Duration::ZERO,
        dead_after: LIVENESS_DEAD_AFTER,
        link_back_unanswered: false,
        routing_down: false,
        since_toggle: MIN_ROUTE_TOGGLE,
        retry_ok: true,
        answered_since_toggle: true,
    };
    let with = |f: &dyn Fn(&mut RouteInputs)| {
        let mut i = base;
        f(&mut i);
        route_decision(&i)
    };
    // Alive and routed through the tunnel: nothing to do.
    assert_eq!(route_decision(&base), RouteAction::None);
    // Silent past the threshold: the system's own routing wins.
    assert_eq!(
        with(&|i| i.silent = LIVENESS_DEAD_AFTER),
        RouteAction::Fallback
    );
    // A short gap: keep the tunnel.
    assert_eq!(
        with(&|i| i.silent = LIVENESS_DEAD_AFTER - Duration::from_secs(1)),
        RouteAction::None
    );
    // The uplink came back and the peer did not answer: another network.
    assert_eq!(
        with(&|i| i.link_back_unanswered = true),
        RouteAction::Fallback
    );
    // Fallen back and the peer answers again: hand the routing back.
    assert_eq!(with(&|i| i.routing_down = true), RouteAction::Restore);
    // Fallen back, no peer data since that change (a fresh counter at start
    // reads as silent 0): never hand back on that alone.
    assert_eq!(
        with(&|i| {
            i.routing_down = true;
            i.answered_since_toggle = false;
        }),
        RouteAction::None
    );
    // Fallen back and still silent, or on another network: stay.
    assert_eq!(
        with(&|i| {
            i.routing_down = true;
            i.silent = LIVENESS_DEAD_AFTER;
        }),
        RouteAction::None
    );
    assert_eq!(
        with(&|i| {
            i.routing_down = true;
            i.link_back_unanswered = true;
        }),
        RouteAction::None
    );
    // Toggle floor and retry gate.
    assert_eq!(
        with(&|i| {
            i.silent = LIVENESS_DEAD_AFTER;
            i.since_toggle = Duration::from_secs(1);
        }),
        RouteAction::None
    );
    assert_eq!(
        with(&|i| {
            i.silent = LIVENESS_DEAD_AFTER;
            i.retry_ok = false;
        }),
        RouteAction::None
    );
    // Kill switch on, or no tunnel: never touch routing.
    assert_eq!(
        with(&|i| {
            i.silent = LIVENESS_DEAD_AFTER;
            i.kill_switch = true;
        }),
        RouteAction::None
    );
    assert_eq!(
        with(&|i| {
            i.silent = LIVENESS_DEAD_AFTER;
            i.tunnel_present = false;
        }),
        RouteAction::None
    );
}

/// A reconnect after a fallback waits RETURN_CONNECT_TIMEOUT, also after the
/// routing was handed back on the peer's Proof (the restore clears
/// `routing_down`, but a young WireGuard session needs its ~15 s no-reply
/// re-handshake, which races the plain timeout). The code track keeps
/// CONNECT_TIMEOUT.
#[test]
fn the_return_after_a_fallback_keeps_the_long_connect_timeout() {
    let _exec = exec_lock();
    use jafnify_core::timing::RETURN_CONNECT_TIMEOUT;
    use jafnify_linux::app::{known_connect_timeout, CONNECT_TIMEOUT};
    assert_eq!(known_connect_timeout(false), CONNECT_TIMEOUT);
    assert_eq!(known_connect_timeout(true), RETURN_CONNECT_TIMEOUT);
}
