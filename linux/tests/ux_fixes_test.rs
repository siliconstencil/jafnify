//! Two things seen in a real-device test.
//!
//!   - Cancel while Searching raised a polkit prompt for `wg-quick down jafn0`
//!     although no tunnel had ever been brought up.
//!   - A stale "Last error: Root privileges required - wg-quick:
//!     /etc/wireguard/jafn0.conf does not exist" showed during a fresh search.
//!
//! Everything is fake: pkexec is a script that records its calls.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use jafnify_core::known::pair_record;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, KeyPair};
use jafnify_linux::app::{App, AppBuilder};
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    /// `pkexec_fails`: every root command exits 1 with the message of the real
    /// bug report on stderr.
    fn new(pkexec_fails: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let fail = if pkexec_fails {
            "echo 'wg-quick: /etc/wireguard/jafn0.conf does not exist' >&2\nexit 1\n"
        } else {
            "exit 0\n"
        };
        let p = dir.path().join("mock-pkexec.sh");
        std::fs::write(
            &p,
            format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n{fail}", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn rx_path(&self) -> PathBuf {
        self.dir.path().join("rx_bytes")
    }

    /// The tunnel interface exists (sysfs shows its counter).
    fn tunnel_is_up(&self) {
        std::fs::write(self.rx_path(), "0\n").unwrap();
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn app(&self) -> App {
        AppBuilder::new(self.dir.path().join("cfg"))
            .pkexec_bin(self.dir.path().join("mock-pkexec.sh"))
            .wg_quick_bin(PathBuf::from("/usr/bin/wg-quick"))
            .wg_bin(PathBuf::from("/nonexistent/wg"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .build()
    }

    fn store_pair(&self) {
        let rec = pair_record(
            &KeyPair::generate(),
            &KeyPair::generate().public_key_b64(),
            Role::Joiner,
            None,
            1,
        )
        .unwrap();
        save_state(
            &self.dir.path().join("cfg/state.toml"),
            &LinuxStateFile {
                peers: vec![rec],
                settings: Settings {
                    role: Role::Joiner,
                    autostart: false,
                    kill_switch: false,
                    tray_only_when_closed: false,
                    ..Settings::default()
                },
            },
        )
        .unwrap();
    }
}

#[test]
fn cancelling_a_search_runs_no_root_command() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let mut app = lab.app();
    app.start("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    app.stop().unwrap();
    assert_eq!(app.state(), ConnectionState::Idle);
    assert_eq!(
        lab.calls(),
        "",
        "a polkit prompt for a tunnel that never existed"
    );
    assert!(app.last_error().is_none());
}

#[test]
fn cancelling_a_known_search_runs_no_root_command_either() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    lab.store_pair();
    let mut app = lab.app();
    app.start_known().unwrap();
    app.stop().unwrap();
    assert_eq!(lab.calls(), "");
}

#[test]
fn stopping_with_a_tunnel_up_still_takes_it_down() {
    // Disconnect, and `jafnify stop` from another process, must still work:
    // the interface is there, so the teardown runs.
    let lab = Lab::new(false);
    lab.tunnel_is_up();
    let mut app = lab.app();
    app.stop().unwrap();
    // The teardown also removes the installed config (it holds the private key).
    let calls = lab.calls();
    assert!(calls.contains("ip link del dev jafn0"), "{calls}");
    assert!(calls.contains("jafn0 is still present"), "{calls}");
}

#[test]
fn a_new_search_starts_without_the_last_error() {
    let _g = multicast_lock();
    let lab = Lab::new(true);
    lab.tunnel_is_up();
    let mut app = lab.app();
    // A teardown that fails leaves the message of the bug report behind.
    app.stop().unwrap();
    assert!(
        app.last_error()
            .is_some_and(|e| e.contains("does not exist")),
        "{:?}",
        app.last_error()
    );

    app.start("1234").unwrap();
    assert_eq!(
        app.last_error(),
        None,
        "stale error shown during a fresh search"
    );
}

#[test]
fn a_connected_app_does_not_report_a_privileged_status_failure() {
    // Seen in the first real-phone test: while connected, the card showed
    // "Last error: status unavailable (wg show requires privileges)". The
    // card's counters come from sysfs, so a missing privileged status is not
    // an error.
    let _g = multicast_lock();
    let lab = Lab::new(false);
    lab.tunnel_is_up();
    let mut app = lab.app();
    app.orchestrator_mut().confirm_connected();
    app.tick().unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert_eq!(app.last_error(), None, "privileged status is not an error");
}

#[test]
fn a_successful_connection_clears_the_stale_error() {
    let _g = multicast_lock();
    let lab = Lab::new(true);
    lab.tunnel_is_up();
    let mut app = lab.app();
    app.stop().unwrap();
    assert!(
        app.last_error().is_some(),
        "the failed teardown left a message"
    );

    app.orchestrator_mut().confirm_connected();
    app.tick().unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert_eq!(
        app.last_error(),
        None,
        "a connected tunnel must not show a stale teardown error"
    );
}

#[test]
fn a_new_known_search_clears_it_too() {
    let _g = multicast_lock();
    let lab = Lab::new(true);
    lab.tunnel_is_up();
    lab.store_pair();
    let mut app = lab.app();
    app.stop().unwrap();
    assert!(app.last_error().is_some());
    app.start_known().unwrap();
    assert_eq!(app.last_error(), None);
}
