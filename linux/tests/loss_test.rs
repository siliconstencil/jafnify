//! Dead-tunnel handling. While `jafn0` is up it routes 0.0.0.0/0 into the
//! tunnel; if the peer vanishes the PC stays offline until the peer returns.
//! The App must notice (no rx progress on the interface) and wait on the same
//! interface: no privileged teardown and no privileged bring-up on the
//! reconnect path. Everything is fake: pkexec/wg-quick are scripts, the rx
//! counter is a file, the clock is injected through `tick_at`.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::pair_record;
use jafnify_core::pairing::PairingCode;
use jafnify_core::pake::PakeLink;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, DiscoveryConfig, KeyPair, RETRY_INTERVAL};
use jafnify_linux::app::{
    App, AppBuilder, LinkStateFn, UiEvent, LINK_POLL_INTERVAL, RX_STALL_TIMEOUT,
};
use jafnify_linux::applog::format_local_millis;
use jafnify_linux::helper::LiveConfig;
use jafnify_linux::polkit;
use jafnify_linux::session::{SessionHistory, SessionRecord, MAX_SESSIONS};
use jafnify_linux::settings::{load_state, save_state, LinuxStateFile, Settings};
use jafnify_linux::ui_text::LINK_LOST;

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    /// `fail_up`: the fake pkexec exits 1 for `wg-quick up`.
    fn new(fail_up: bool) -> Self {
        Self::build(fail_up, None)
    }

    /// The fake pkexec refuses everything with `code`, like polkit when the
    /// password dialog is dismissed (126) or the user is not authorized (127).
    fn denying(code: i32) -> Self {
        Self::build(false, Some(code))
    }

    /// The fake pkexec refuses only the bring-up and succeeds for everything
    /// else, like a user who dismisses the first password dialog.
    fn denying_up(code: i32) -> Self {
        Self::with_guard(format!(
            "case \"$*\" in *\" up \"*) exit {code};; esac\nexit 0\n"
        ))
    }

    fn build(fail_up: bool, deny: Option<i32>) -> Self {
        let up_guard = match (fail_up, deny) {
            (_, Some(code)) => format!("exit {code}\n"),
            (true, None) => {
                "case \"$*\" in *\" up \"*) echo 'wg-quick: boom' >&2; exit 1;; esac\n".to_string()
            }
            (false, None) => String::new(),
        };
        Self::with_guard(up_guard)
    }

    fn with_guard(up_guard: String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let fakes = [
            (
                "mock-pkexec.sh",
                format!(
                    "#!/bin/sh\necho \"$@\" >> \"{}\"\n{}for a in \"$@\"; do case \"$a\" in *jafnify-route*|*route-fallback*|*route-restore*|*resolvectl*|*jafn0*fwmark*) exit 0;; esac; done\nexec \"$@\"\n",
                    log.display(),
                    up_guard
                ),
            ),
            ("mock-wg.sh", "#!/bin/sh\nexit 1\n".to_string()),
            ("mock-wg-quick.sh", "#!/bin/sh\nexit 0\n".to_string()),
            ("mock-install.sh", "#!/bin/sh\nexit 0\n".to_string()),
            // No host `ip` may run in a test; a failing one skips suppressions.
            ("mock-ip.sh", "#!/bin/sh\nexit 1\n".to_string()),
        ];
        for (name, body) in fakes {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn rx_path(&self) -> PathBuf {
        self.dir.path().join("rx_bytes")
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(self.rx_path(), format!("{n}\n")).unwrap();
    }

    fn tx_path(&self) -> PathBuf {
        self.dir.path().join("tx_bytes")
    }

    fn set_tx(&self, n: u64) {
        std::fs::write(self.tx_path(), format!("{n}\n")).unwrap();
    }

    fn state_path(&self) -> PathBuf {
        self.dir.path().join("cfg/state.toml")
    }

    fn sessions_path(&self) -> PathBuf {
        self.dir.path().join("sessions.toml")
    }

    /// Store a pair as an earlier code pairing would have, before the App loads.
    /// Returns the stored peer public key.
    fn store_pair(&self, own_role: Role) -> String {
        let peer = KeyPair::generate();
        let rec = pair_record(
            &KeyPair::generate(),
            &peer.public_key_b64(),
            own_role,
            None,
            1,
        )
        .unwrap();
        save_state(
            &self.state_path(),
            &LinuxStateFile {
                peers: vec![rec],
                settings: Settings {
                    role: own_role,
                    autostart: false,
                    kill_switch: false,
                    tray_only_when_closed: false,
                    ..Settings::default()
                },
            },
        )
        .unwrap();
        peer.public_key_b64()
    }

    /// Root commands the App issued so far (one line per pkexec call).
    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn app(&self) -> App {
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(self.sessions_path())
            .session_history_owner(true)
            .polkit_rules_path(self.dir.path().join("rules"))
            .installed_conf_path(self.dir.path().join("jafn0.conf"))
            .build()
    }

    /// App whose session clock starts at `epoch_ms` (deterministic tests).
    fn app_with_epoch(&self, epoch_ms: i64) -> App {
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(self.sessions_path())
            .session_history_owner(true)
            .session_epoch_ms(epoch_ms)
            .polkit_rules_path(self.dir.path().join("rules"))
            .installed_conf_path(self.dir.path().join("jafn0.conf"))
            .build()
    }

    /// Make the next root call remove the counter files, like wg-quick down.
    fn remove_counters_on_root(&self) {
        let p = self.dir.path().join("mock-pkexec.sh");
        let old = std::fs::read_to_string(&p).unwrap();
        let prefix = format!(
            "#!/bin/sh\nrm -f \"{}\" \"{}\"\n",
            self.rx_path().display(),
            self.tx_path().display()
        );
        std::fs::write(&p, old.replacen("#!/bin/sh\n", &prefix, 1)).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// App with a fake helper whose `inspect` reports this live config.
    fn app_with_helper(&self, live: &LiveConfig) -> App {
        self.helper_builder(live).build()
    }

    fn helper_builder(&self, live: &LiveConfig) -> AppBuilder {
        let live_path = self.dir.path().join("live.toml");
        std::fs::write(&live_path, live.to_toml().unwrap()).unwrap();
        let helper = self.dir.path().join("mock-helper.sh");
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\nif [ \"$1\" = inspect ]; then cat \"{}\"; exit 0; fi\nexit 0\n",
                live_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            // No host `ip` may run in a test: the host rule table decides the
            // suppress path otherwise, so the result would follow the live tunnel.
            .ip_bin(p("mock-ip.sh"))
            .helper_bin(helper)
            .connection_log_path(Some(self.dir.path().join("connection.log")))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(self.sessions_path())
            .session_history_owner(true)
            .polkit_rules_path(self.dir.path().join("rules"))
            .installed_conf_path(self.dir.path().join("jafn0.conf"))
    }

    /// App driven straight to Connected (the tunnel itself is not needed:
    /// only the loss watch is under test).
    fn connected_app(&self) -> App {
        let mut app = self.app();
        app.start("1234").unwrap();
        app.accept_code("1234").unwrap();
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);
        app
    }

    /// App with the uplink watch injected: a fixed interface and a closure
    /// standing in for carrier + address state. No host interface is read.
    fn app_with_link(&self, iface: Option<String>, state: LinkStateFn) -> App {
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .ip_bin(p("mock-ip.sh"))
            .helper_bin(self.dir.path().join("missing-helper"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(self.sessions_path())
            .session_history_owner(true)
            .polkit_rules_path(self.dir.path().join("rules"))
            .installed_conf_path(self.dir.path().join("jafn0.conf"))
            .link_iface(iface)
            .link_state(state)
            .build()
    }

    fn connected_app_with_link(&self, iface: Option<String>, state: LinkStateFn) -> App {
        let mut app = self.app_with_link(iface, state);
        app.start("1234").unwrap();
        app.accept_code("1234").unwrap();
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);
        app
    }
}

/// A shared uplink state and the closure the app observes it through.
fn shared_uplink(up: bool) -> (Arc<Mutex<bool>>, LinkStateFn) {
    let flag = Arc::new(Mutex::new(up));
    let seen = flag.clone();
    let state: LinkStateFn = Arc::new(move |_| Some(*seen.lock().unwrap()));
    (flag, state)
}

fn set_uplink(flag: &Arc<Mutex<bool>>, up: bool) {
    *flag.lock().unwrap() = up;
}

fn is_disconnect(ev: &Option<UiEvent>) -> bool {
    matches!(ev, Some(UiEvent::Disconnected { .. }))
}

#[test]
fn stalled_rx_keeps_the_tunnel_up_then_retries() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();

    assert_eq!(app.tick_at(t0).unwrap(), None);
    let almost = t0 + RX_STALL_TIMEOUT - Duration::from_secs(1);
    assert_eq!(app.tick_at(almost).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!lab.calls().contains("down"), "torn down too early");

    let ev = app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap();
    assert!(is_disconnect(&ev), "got {ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(app
        .status_text_at(t0 + RX_STALL_TIMEOUT)
        .starts_with("Connection lost - retrying in "));
    // Kill switch off: the grace counts from the last peer data, and a peer
    // loss is declared only after RX_STALL_TIMEOUT of silence, so the
    // password-free route fallback runs now. Nothing else may ask for root.
    assert!(
        !lab.calls().is_empty() && only_route_fallback(&lab.calls()),
        "peer loss must run the route fallback and nothing else: {:?}",
        lab.calls()
    );
}

/// Every root call is the password-free route fallback (route binary verb or
/// its fixed script); never a tunnel up/down, wg-quick or inspect.
fn only_route_fallback(calls: &str) -> bool {
    calls.lines().all(|line| {
        (line.contains("route-fallback") || line.contains("jafnify_killswitch"))
            && !line.contains("wg-quick")
            && !line.ends_with(" up")
            && !line.ends_with(" down")
    })
}

#[test]
fn a_known_peer_loss_and_retry_make_zero_root_calls() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let peer_key = lab.store_pair(Role::Joiner);
    lab.set_rx(100);
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();

    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    // The loss watch starts on the first Connected tick.
    app.tick_at(t0).unwrap();
    let before_loss = lab.calls();

    assert!(is_disconnect(&app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap()));
    assert_eq!(app.state(), ConnectionState::Retry);
    app.tick_at(t0 + RX_STALL_TIMEOUT + RETRY_INTERVAL).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    let after = lab.calls();
    let added = after.strip_prefix(before_loss.as_str()).unwrap_or(&after);
    assert!(
        only_route_fallback(added),
        "reconnect asked for root beyond the route fallback: {added:?}"
    );
}

#[test]
fn rx_progress_resets_the_stall_window() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();

    // A keepalive arrives late in the window.
    let t1 = t0 + RX_STALL_TIMEOUT - Duration::from_secs(1);
    lab.set_rx(148);
    assert_eq!(app.tick_at(t1).unwrap(), None);

    // Past the first window, but only 29 s after the last progress.
    let t2 = t0 + RX_STALL_TIMEOUT + Duration::from_secs(5);
    assert_eq!(app.tick_at(t2).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);

    let ev = app.tick_at(t1 + RX_STALL_TIMEOUT).unwrap();
    assert!(is_disconnect(&ev), "got {ev:?}");
}

#[test]
fn missing_rx_counter_counts_as_a_stall() {
    // The interface is gone (someone ran wg-quick down): sysfs file absent.
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    let t0 = Instant::now();
    assert_eq!(app.tick_at(t0).unwrap(), None);
    let ev = app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap();
    assert!(is_disconnect(&ev), "got {ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
}

#[test]
fn retry_goes_back_to_searching_after_ten_seconds() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(1);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    let lost = t0 + RX_STALL_TIMEOUT;
    assert!(is_disconnect(&app.tick_at(lost).unwrap()));

    app.tick_at(lost + RETRY_INTERVAL - Duration::from_millis(100))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Retry);
    app.tick_at(lost + RETRY_INTERVAL).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
}

#[test]
fn failed_bring_up_goes_to_retry_instead_of_erroring_out() {
    let _g = multicast_lock();
    let lab = Lab::new(true);
    let mut app = lab.app();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();

    // A peer that knows the code answers the PAKE Hello; our wg-quick up fails.
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    };
    let code = PairingCode::parse("1234").unwrap();
    let mut peer = PakeLink::open(
        &cfg,
        Role::Sharer,
        &code,
        &KeyPair::generate(),
        jafnify_core::LISTEN_PORT,
    )
    .unwrap();

    let end = Instant::now() + Duration::from_secs(5);
    let mut ev = None;
    while ev.is_none() && Instant::now() < end {
        peer.poll(Instant::now()).unwrap();
        ev = app
            .tick()
            .expect("a failed wg-quick up must not bubble out of tick");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(is_disconnect(&ev), "got {ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    let calls = lab.calls();
    assert!(
        calls.contains("-m 600") && calls.contains(" up "),
        "no up in the one combined call: {calls:?}"
    );
    assert!(
        calls.contains("ip link del dev jafn0"),
        "no cleanup down after failed up: {calls:?}"
    );
    assert!(
        !calls.contains("/etc/wireguard") && !calls.contains("/sys/class/net"),
        "test teardown used a host path: {calls:?}"
    );
    assert!(app.status_text().contains("boom"), "{}", app.status_text());
}

// ---- polkit denial: stop and ask, never loop ------------------------------

/// Tick until an event arrives (or 5 s pass).
fn tick_until_event(app: &mut App, ghost: Option<&mut PakeLink>) -> Option<UiEvent> {
    let end = Instant::now() + Duration::from_secs(5);
    let mut ev = None;
    let mut ghost = ghost;
    while ev.is_none() && Instant::now() < end {
        if let Some(g) = ghost.as_deref_mut() {
            g.poll(Instant::now()).unwrap();
        }
        ev = app.tick().expect("tick must not fail");
        std::thread::sleep(Duration::from_millis(20));
    }
    ev
}

/// A code pairing whose peer has announced, so the App tries to bring up the
/// tunnel (the first pkexec call is the config install).
fn code_app_with_peer_announced(lab: &Lab) -> (App, PakeLink) {
    let mut app = lab.app();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    };
    let code = PairingCode::parse("1234").unwrap();
    let mut peer = PakeLink::open(
        &cfg,
        Role::Sharer,
        &code,
        &KeyPair::generate(),
        jafnify_core::LISTEN_PORT,
    )
    .unwrap();
    peer.poll(Instant::now()).unwrap();
    (app, peer)
}

fn denial_stops_instead_of_retrying(code: i32) {
    let _g = multicast_lock();
    let lab = Lab::denying(code);
    let (mut app, mut ghost) = code_app_with_peer_announced(&lab);

    let ev = tick_until_event(&mut app, Some(&mut ghost));
    assert_eq!(ev, Some(UiEvent::PermissionDenied), "exit {code}: {ev:?}");
    assert_eq!(app.state(), ConnectionState::Idle, "must not go to Retry");
    assert!(app.denied());
    // One prompt was refused; nothing may prompt again (no `down`, no `up`).
    assert_eq!(lab.calls().lines().count(), 1, "{:?}", lab.calls());
    assert!(!lab.calls().contains("down"), "{:?}", lab.calls());
    // The config with the private key must not be left behind.
    assert!(
        !lab.dir.path().join("cfg/jafn0.conf").exists(),
        "private-key config left in the config dir"
    );

    // A long time later still nothing happens: no 10 s prompt loop.
    let later = Instant::now() + Duration::from_secs(60);
    assert_eq!(app.tick_at(later).unwrap(), None);
    assert_eq!(app.tick_at(later + RETRY_INTERVAL * 2).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Idle);
    assert_eq!(lab.calls().lines().count(), 1, "re-prompted on its own");
    assert!(
        app.connection_log()
            .iter()
            .any(|line| line.ends_with("privileged up: denied (password prompt)")),
        "{:?}",
        app.connection_log()
    );
}

#[test]
fn a_dismissed_polkit_dialog_stops_and_does_not_retry() {
    denial_stops_instead_of_retrying(126);
}

#[test]
fn a_not_authorized_polkit_answer_stops_and_does_not_retry() {
    denial_stops_instead_of_retrying(127);
}

#[test]
fn try_again_searches_afresh_and_clears_the_denial() {
    let _g = multicast_lock();
    let lab = Lab::denying(126);
    let (mut app, mut ghost) = code_app_with_peer_announced(&lab);
    assert_eq!(
        tick_until_event(&mut app, Some(&mut ghost)),
        Some(UiEvent::PermissionDenied)
    );

    app.try_again().expect("the same code is searched again");
    assert!(!app.denied());
    assert_eq!(app.state(), ConnectionState::Searching);
}

#[test]
fn try_again_without_a_denial_is_a_no_op() {
    let lab = Lab::new(false);
    let mut app = lab.app();
    app.try_again().unwrap();
    assert_eq!(app.state(), ConnectionState::Idle);
}

#[test]
fn a_known_sharer_refused_by_polkit_stops_and_try_again_restarts_it() {
    let _g = multicast_lock();
    let lab = Lab::denying(126);
    lab.store_pair(Role::Sharer);
    let mut app = lab.app();
    app.start_known().unwrap();

    assert_eq!(
        tick_until_event(&mut app, None),
        Some(UiEvent::PermissionDenied)
    );
    assert_eq!(app.state(), ConnectionState::Idle);
    assert!(app.denied());
    assert_eq!(lab.calls().lines().count(), 1, "{:?}", lab.calls());

    app.try_again().unwrap();
    assert!(!app.denied());
    assert_eq!(app.state(), ConnectionState::Searching);
    assert!(app.is_paired(), "a denial must not forget the pair");
}

#[test]
fn stopping_or_forgetting_clears_the_denial() {
    let _g = multicast_lock();
    let lab = Lab::denying(126);
    let (mut app, mut ghost) = code_app_with_peer_announced(&lab);
    assert_eq!(
        tick_until_event(&mut app, Some(&mut ghost)),
        Some(UiEvent::PermissionDenied)
    );
    app.stop().unwrap();
    assert!(!app.denied());
}

#[test]
fn forgetting_the_device_clears_the_denial() {
    let _g = multicast_lock();
    let lab = Lab::denying_up(126);
    lab.store_pair(Role::Sharer);
    let mut app = lab.app();
    app.start_known().unwrap();
    assert_eq!(
        tick_until_event(&mut app, None),
        Some(UiEvent::PermissionDenied)
    );
    app.forget().unwrap();
    assert!(!app.denied());
    assert!(!app.is_paired());
}

#[test]
fn any_other_bring_up_failure_still_retries() {
    let _g = multicast_lock();
    let lab = Lab::new(true); // wg-quick up exits 1
    let (mut app, mut ghost) = code_app_with_peer_announced(&lab);
    let ev = tick_until_event(&mut app, Some(&mut ghost));
    assert!(is_disconnect(&ev), "{ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(!app.denied());
}

// ---- connected clock, counters, last connected ----------------------------

#[test]
fn the_connected_clock_starts_at_connected_and_stops_on_loss() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(1);
    let t0 = Instant::now();
    assert_eq!(app.connected_secs_at(t0), None, "not seen by a tick yet");

    app.tick_at(t0).unwrap();
    assert_eq!(app.connected_secs_at(t0), Some(0));
    assert_eq!(
        app.connected_secs_at(t0 + Duration::from_secs(90)),
        Some(90)
    );

    // The tunnel dies: the clock stops with it.
    assert!(is_disconnect(&app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap()));
    assert_eq!(app.connected_secs_at(t0 + RX_STALL_TIMEOUT), None);
}

#[test]
fn traffic_is_up_from_tx_and_down_from_rx_and_none_when_unreadable() {
    let lab = Lab::new(false);
    let app = lab.app();
    assert_eq!(app.traffic(), None);
    lab.set_rx(100);
    assert_eq!(app.traffic(), None, "tx still unreadable");
    lab.set_tx(50);
    assert_eq!(app.traffic(), Some((50, 100)));
}

#[test]
fn connecting_stamps_last_connected_on_the_stored_pair_only() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    lab.store_pair(Role::Joiner);
    let mut app = lab.app();
    assert_eq!(app.last_connected(), Some(1), "as stored by the pairing");
    app.start_known().unwrap();
    app.orchestrator_mut().known_peer_found();
    app.orchestrator_mut().confirm_connected();
    lab.set_rx(1);
    app.tick_at(Instant::now()).unwrap();

    let stored = load_state(&lab.state_path()).unwrap();
    let when = stored.peers[0].last_connected.unwrap();
    assert!(when > 1_700_000_000, "not a current time: {when}");
    assert_eq!(app.last_connected(), Some(when));
    assert!(
        app.paired_name().unwrap().contains(" - "),
        "{:?}",
        app.paired_name()
    );
}

#[test]
fn connecting_without_a_stored_pair_writes_no_state_file() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(1);
    app.tick_at(Instant::now()).unwrap();
    assert!(!lab.state_path().exists());
    assert_eq!(app.last_connected(), None);
    assert_eq!(app.paired_name(), None);
}

// ---- session tracking ------------------------------------------------------

#[test]
fn a_drop_records_one_reconnect_without_ending_the_session() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(200);
    lab.set_tx(80);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    let id = app.current_session().id;

    let drop_at = t0 + Duration::from_secs(1) + RX_STALL_TIMEOUT;
    let ev = app.tick_at(drop_at).unwrap();
    assert!(is_disconnect(&ev), "{ev:?}");
    let dropped = app.current_session();
    assert_eq!(dropped.id, id);
    assert_eq!(dropped.drops, 1);
    assert_eq!(dropped.reconnects.len(), 1);
    assert_eq!(dropped.reconnects[0].connected_at, 0);
    assert!(app.session_history().is_empty(), "a drop ended the session");

    // The peer returns and new data arrives.
    app.orchestrator_mut().confirm_connected();
    lab.set_rx(300);
    lab.set_tx(120);
    let reconnect_at = drop_at + Duration::from_secs(5);
    assert_eq!(app.tick_at(reconnect_at).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);

    let live = app.current_session();
    assert_eq!(live.id, id);
    assert_eq!(live.drops, 1);
    assert_eq!(live.reconnects.len(), 1);
    assert!(live.reconnects[0].connected_at > 0);
    assert!(live.reconnects[0].duration_ms > 0);
    assert!(
        app.session_history().is_empty(),
        "a reconnect ended the session"
    );
}

#[test]
fn connection_log_pins_the_drop_event_time_and_state_line() {
    const EPOCH_MS: i64 = 1_700_000_000_000;
    let lab = Lab::new(false);
    let mut app = lab.app_with_epoch(EPOCH_MS);
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().confirm_connected();

    let base = app.session_epoch();
    lab.set_rx(100);
    lab.set_tx(50);
    app.tick_at(base + Duration::from_secs(1)).unwrap();
    // The last peer activity before the loss anchors the drop event time.
    lab.set_rx(200);
    lab.set_tx(80);
    app.tick_at(base + Duration::from_secs(2)).unwrap();

    let drop_at = base + Duration::from_secs(2) + RX_STALL_TIMEOUT;
    assert!(is_disconnect(&app.tick_at(drop_at).unwrap()));
    let log = app.connection_log();
    let event = format_local_millis(EPOCH_MS + 2_000);
    let expected = format!(
        "{} disconnected at {}: no data from peer for 35 s (detected after 35 s)",
        format_local_millis(EPOCH_MS + 2_000 + RX_STALL_TIMEOUT.as_millis() as i64),
        event
    );
    let drop_line = log
        .iter()
        .find(|line| line.contains("disconnected at "))
        .unwrap_or_else(|| panic!("no drop line in {log:?}"));
    assert_eq!(drop_line, &expected);
    assert!(
        log.iter()
            .any(|line| line.ends_with("state Connected -> Retry")),
        "{log:?}"
    );
}

fn stored_record(id: &str) -> SessionRecord {
    SessionRecord {
        id: id.to_string(),
        role: "joiner".to_string(),
        started_at: 1_700_000_000_000,
        ended_at: 1_700_000_001_000,
        end_reason: "stopped".to_string(),
        ..Default::default()
    }
}

#[test]
fn stop_and_forget_persist_completed_sessions_newest_first() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(150);
    lab.set_tx(70);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    app.stop().unwrap();

    let log = app.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("session ended (stopped)")),
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|line| line.ends_with("session started (joiner)"))
            .count(),
        1,
        "stop must not open a new session in the visible log: {log:?}"
    );
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged down: ok (password prompt)")),
        "teardown did not use the down template: {log:?}"
    );

    let history = app.session_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].end_reason, "stopped");
    assert_eq!(history[0].up_bytes, 20);
    assert_eq!(history[0].down_bytes, 50);
    let first_id = history[0].id.clone();
    let on_disk = SessionHistory::load(&lab.sessions_path()).recent(MAX_SESSIONS);
    assert_eq!(on_disk.len(), 1, "stop did not write exactly one record");
    assert_eq!(on_disk[0].id, first_id);

    // A second session ends with forget; it must move the first down.
    app.start("1234").unwrap();
    // The new run starts below the earlier one: the log keeps earlier runs.
    let log = app.connection_log();
    assert!(
        log.last()
            .is_some_and(|line| line.ends_with("session started (joiner)")),
        "the new run did not start with its own line: {log:?}"
    );
    assert!(
        log.iter()
            .any(|line| line.ends_with("session ended (stopped)")),
        "the earlier run's lines were dropped: {log:?}"
    );
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().confirm_connected();
    lab.set_rx(200);
    lab.set_tx(50);
    app.tick_at(Instant::now()).unwrap();
    lab.set_rx(260);
    lab.set_tx(80);
    app.tick_at(Instant::now() + Duration::from_secs(1))
        .unwrap();
    app.forget().unwrap();

    let history = app.session_history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].end_reason, "stopped");
    assert_ne!(history[0].id, first_id);
    assert_eq!(history[1].id, first_id);
    let on_disk = SessionHistory::load(&lab.sessions_path()).recent(MAX_SESSIONS);
    assert_eq!(on_disk.len(), 2);
    assert_eq!(on_disk[0].id, history[0].id);
    assert_eq!(on_disk[1].id, first_id);
}

#[test]
fn a_session_save_error_becomes_last_error_instead_of_a_panic() {
    let lab = Lab::new(false);
    // A directory at the target path makes the atomic rename fail.
    std::fs::create_dir_all(lab.sessions_path()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(120);
    lab.set_tx(60);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();

    app.stop().unwrap();
    assert!(
        app.last_error()
            .is_some_and(|e| e.contains("could not save the session")),
        "{:?}",
        app.last_error()
    );
}

#[test]
fn build_loads_session_history_from_the_injected_path() {
    let lab = Lab::new(false);
    let mut history = SessionHistory::default();
    history.push(stored_record("loaded-1"));
    history.save(&lab.sessions_path()).unwrap();

    let app = lab.app();
    let loaded = app.session_history();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "loaded-1");
}

#[test]
fn the_history_cap_stays_at_fifty_through_the_app() {
    let lab = Lab::new(false);
    let mut history = SessionHistory::default();
    for i in 0..MAX_SESSIONS {
        history.push(stored_record(&format!("old-{i}")));
    }
    history.save(&lab.sessions_path()).unwrap();

    let mut app = lab.connected_app();
    assert_eq!(app.session_history().len(), MAX_SESSIONS);
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(120);
    lab.set_tx(60);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    app.stop().unwrap();

    let history = app.session_history();
    assert_eq!(history.len(), MAX_SESSIONS);
    assert_eq!(history[0].end_reason, "stopped");
    assert!(
        history.iter().all(|r| r.id != "old-0"),
        "the oldest record survived the cap"
    );
}

#[test]
fn a_drop_records_with_the_last_counters_when_they_disappear() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(200);
    lab.set_tx(80);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();

    // The interface/counters disappear with the peer.
    std::fs::remove_file(lab.rx_path()).unwrap();
    std::fs::remove_file(lab.tx_path()).unwrap();
    let drop_at = t0 + Duration::from_secs(1) + RX_STALL_TIMEOUT;
    assert!(is_disconnect(&app.tick_at(drop_at).unwrap()));
    assert_eq!(app.state(), ConnectionState::Retry);

    let dropped = app.current_session();
    assert_eq!(dropped.drops, 1, "{dropped:?}");
    assert_eq!(dropped.reconnects.len(), 1, "{dropped:?}");
    let connected_after_drop = dropped.connected_ms;

    // Retry time must not be counted as Connected time.
    app.tick_at(drop_at + Duration::from_secs(2)).unwrap();
    assert_eq!(app.state(), ConnectionState::Retry);
    let still_retry = app.current_session();
    assert_eq!(still_retry.drops, 1);
    assert_eq!(still_retry.connected_ms, connected_after_drop);
}

#[test]
fn stop_samples_final_counters_before_teardown() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(200);
    lab.set_tx(80);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();

    // Traffic arrives after the last tick, then teardown removes the counters.
    lab.set_rx(900);
    lab.set_tx(400);
    lab.remove_counters_on_root();
    app.stop().unwrap();

    let history = app.session_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].down_bytes, 800, "{:?}", history[0]);
    assert_eq!(history[0].up_bytes, 350, "{:?}", history[0]);
}

#[test]
fn stop_on_an_idle_app_writes_no_history_record() {
    let lab = Lab::new(false);
    let mut app = lab.app();
    app.stop().unwrap();
    assert!(app.session_history().is_empty());
    assert!(
        !lab.sessions_path().exists(),
        "an empty session wrote history"
    );
}

#[test]
fn a_dropped_session_with_no_activity_is_still_kept() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(0);
    lab.set_tx(0);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    app.orchestrator_mut()
        .on_disconnect_at("link down".to_string(), t0);
    app.tick_at(t0).unwrap();
    assert_eq!(app.current_session().drops, 1);
    assert_eq!(app.current_session().connected_ms, 0);

    app.stop().unwrap();
    let history = app.session_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].drops, 1);
}

#[test]
fn sampling_the_same_instant_twice_is_idempotent() {
    let lab = Lab::new(false);
    let mut app = lab.connected_app();
    lab.set_rx(100);
    lab.set_tx(50);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    lab.set_rx(200);
    lab.set_tx(80);
    let t1 = t0 + Duration::from_secs(1);
    app.tick_at(t1).unwrap();

    let first = app.current_session();
    app.tick_at(t1).unwrap();
    let second = app.current_session();
    assert_eq!(second.up_bytes, first.up_bytes);
    assert_eq!(second.down_bytes, first.down_bytes);
    assert_eq!(second.connected_ms, first.connected_ms);
    assert_eq!(second.drops, first.drops);
    assert_eq!(second.reconnects, first.reconnects);
}

#[test]
fn session_epoch_ms_sets_the_session_clock_base() {
    const EPOCH_MS: i64 = 1_700_000_000_000;
    let lab = Lab::new(false);
    let mut app = lab.app_with_epoch(EPOCH_MS);
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().confirm_connected();
    assert_eq!(app.current_session().started_at, EPOCH_MS);

    lab.set_rx(100);
    lab.set_tx(50);
    let base = app.session_epoch();
    app.tick_at(base + Duration::from_secs(1)).unwrap();
    lab.set_rx(200);
    lab.set_tx(80);
    app.tick_at(base + Duration::from_secs(2)).unwrap();

    lab.set_rx(300);
    lab.set_tx(120);
    app.stop().unwrap();
    let history = app.session_history();
    assert_eq!(history.len(), 1);
    let record = &history[0];
    assert_eq!(record.started_at, EPOCH_MS);
    assert!(record.ended_at >= record.started_at);
    assert!(record.ended_at < EPOCH_MS + 60_000);
    assert_eq!(record.up_bytes, 70);
    assert_eq!(record.down_bytes, 200);
}

#[test]
fn a_failed_forget_does_not_end_the_session() {
    let _g = multicast_lock();
    let lab = Lab::denying(126);
    let mut app = lab.connected_app();
    let id = app.current_session().id;
    lab.set_rx(100);
    lab.set_tx(50);
    app.tick_at(Instant::now()).unwrap();

    assert!(app.forget().is_err(), "a refused forget must fail");
    assert!(app.session_history().is_empty());
    assert_eq!(app.current_session().id, id);
}

#[test]
fn a_paired_idle_forget_still_runs_helper_forget_without_a_tunnel_or_files() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    lab.store_pair(Role::Joiner);
    // None of the old cleanup reasons exist: no tunnel, no config, no rule.
    assert!(!lab.dir.path().join("jafn0.conf").exists());
    assert!(!lab.dir.path().join("rules").exists());
    assert!(!lab.dir.path().join("cfg/jafn0.iface").exists());
    let live = LiveConfig {
        peer_public_key: KeyPair::generate().public_key_b64(),
        endpoint: None,
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2/32".to_string(),
        kill_switch: false,
    };
    let mut app = lab.app_with_helper(&live);
    assert_eq!(app.peers().len(), 1);

    app.forget().unwrap();

    let calls = lab.calls();
    assert!(
        calls.contains("forget"),
        "a stored pair must still run the privileged forget: {calls:?}"
    );
    assert!(app.peers().is_empty(), "the pair was not cleared");
}

#[test]
fn a_repaired_suppress_rule_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("ip-count");
    let ip = dir.path().join("fake-ip.sh");
    std::fs::write(
        &ip,
        format!(
            "#!/bin/sh\n\
             n=$(cat \"{count}\" 2>/dev/null || echo 0)\n\
             n=$((n+1))\n\
             echo \"$n\" > \"{count}\"\n\
             if [ \"$n\" -ge 2 ]; then\n\
               echo '100: from all lookup main suppress_prefixlength 0'\n\
             fi\n\
             exit 0\n",
            count = counter.display()
        ),
    )
    .unwrap();
    let helper = dir.path().join("fake-helper.sh");
    std::fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();
    let pkexec = dir.path().join("fake-pkexec.sh");
    std::fs::write(&pkexec, "#!/bin/sh\nexit 0\n").unwrap();
    let wg = dir.path().join("fake-wg.sh");
    // No fwmark known: the jafn0 rule check stays out of the fake ip count
    // this test drives (it is about the shared suppress rule only).
    std::fs::write(&wg, "#!/bin/sh\necho off\nexit 0\n").unwrap();
    let wg_quick = dir.path().join("fake-wg-quick.sh");
    std::fs::write(&wg_quick, "#!/bin/sh\nexit 0\n").unwrap();
    let install = dir.path().join("fake-install.sh");
    std::fs::write(&install, "#!/bin/sh\nexit 0\n").unwrap();
    for path in [&ip, &helper, &pkexec, &wg, &wg_quick, &install] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rx = dir.path().join("rx_bytes");
    let tx = dir.path().join("tx_bytes");
    std::fs::write(&rx, "100\n").unwrap();
    std::fs::write(&tx, "50\n").unwrap();

    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .ip_bin(ip)
        .pkexec_bin(pkexec)
        .helper_bin(helper)
        .wg_bin(wg)
        .wg_quick_bin(wg_quick)
        .install_bin(install)
        .rx_bytes_path(rx)
        .tx_bytes_path(tx)
        .polkit_rules_path(dir.path().join("rules"))
        .installed_conf_path(dir.path().join("jafn0.conf"))
        .build();
    app.orchestrator_mut().confirm_connected();

    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    assert!(
        counter.exists(),
        "the unprivileged rule check did not run: {:?}",
        app.last_error()
    );
    assert!(
        !app.connection_log()
            .iter()
            .any(|line| line.ends_with("suppress rule restored")),
        "reported before the next check saw the rule"
    );
    // Let the non-blocking helper finish before the next poll.
    std::thread::sleep(Duration::from_millis(50));
    app.tick_at(t0 + Duration::from_secs(2)).unwrap();
    let log = app.connection_log();
    assert_eq!(
        log.iter()
            .filter(|line| line.ends_with("suppress rule restored"))
            .count(),
        1,
        "{log:?}"
    );
    app.tick_at(t0 + Duration::from_secs(4)).unwrap();
    assert_eq!(
        app.connection_log()
            .iter()
            .filter(|line| line.ends_with("suppress rule restored"))
            .count(),
        1,
        "the restore line repeated on every poll"
    );
}

#[test]
fn privileged_source_is_sampled_before_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules");
    let state_path = dir.path().join("cfg/state.toml");
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let rec = pair_record(&own, &peer.public_key_b64(), Role::Sharer, None, 1).unwrap();
    save_state(
        &state_path,
        &LinuxStateFile {
            peers: vec![rec],
            settings: Settings {
                role: Role::Sharer,
                ..Settings::default()
            },
        },
    )
    .unwrap();

    let script = |name: &str, body: String| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let valid_rule = dir.path().join("valid.rules");
    std::fs::write(
        &valid_rule,
        polkit::rule_text(&polkit::current_user().expect("login user")).unwrap(),
    )
    .unwrap();
    let pkexec = script(
        "fake-pkexec.sh",
        format!(
            "#!/bin/sh\n\
             case \"$2\" in\n\
               up) cp \"{valid}\" \"{rules}\";;\n\
               forget) rm -f \"{rules}\";;\n\
             esac\n\
             exit 0\n",
            valid = valid_rule.display(),
            rules = rules.display()
        ),
    );
    let helper = script("fake-helper.sh", "#!/bin/sh\nexit 0\n".to_string());
    let wg = script("fake-wg.sh", "#!/bin/sh\nexit 1\n".to_string());
    let wg_quick = script("fake-wg-quick.sh", "#!/bin/sh\nexit 0\n".to_string());
    let install = script("fake-install.sh", "#!/bin/sh\nexit 0\n".to_string());
    // pkcheck is authorized only while the fake pkexec's up has installed the
    // rule; the sample is taken before each root command.
    let pkcheck = script(
        "fake-pkcheck.sh",
        format!(
            "#!/bin/sh\n\
             echo \"$@\" >> \"{log}\"\n\
             [ -f \"{rules}\" ] && exit 0\n\
             exit 1\n",
            log = dir.path().join("pkcheck.log").display(),
            rules = rules.display()
        ),
    );

    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .wg_bin(wg)
        .pkexec_bin(pkexec)
        .pkcheck_bin(pkcheck)
        .helper_bin(helper)
        .wg_quick_bin(wg_quick)
        .install_bin(install)
        .iface_pin(Ipv4Addr::LOCALHOST)
        .rx_bytes_path(dir.path().join("rx"))
        .tx_bytes_path(dir.path().join("tx"))
        .polkit_rules_path(rules.clone())
        .installed_conf_path(dir.path().join("jafn0.conf"))
        .build();
    app.start_known().unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while !rules.exists() && Instant::now() < end {
        app.tick().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rules.exists(), "the fake up did not install the rule");
    let pkcheck_calls = std::fs::read_to_string(dir.path().join("pkcheck.log")).unwrap();
    assert!(
        pkcheck_calls.contains("--action-id org.jafnify.tunnel"),
        "{pkcheck_calls:?}"
    );
    assert!(pkcheck_calls.contains("--process"), "{pkcheck_calls:?}");
    assert!(
        !pkcheck_calls.contains("--allow-user-interaction"),
        "the hint check must not open a dialog: {pkcheck_calls:?}"
    );

    let log = app.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged up: ok (password prompt)")),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|line| line.ends_with("privileged up: ok (polkit rule)")),
        "the rule installed by the command leaked into the up line: {log:?}"
    );

    app.forget().unwrap();
    assert!(!rules.exists(), "the fake forget did not remove the rule");
    let log = app.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged down: ok (polkit rule)")),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|line| line.ends_with("privileged down: ok (password prompt)")),
        "the rule removed by forget leaked into the down line: {log:?}"
    );
}

// ---- immediate uplink link-loss indication ---------------------------------

/// Count the connection-log lines that end with `text`.
fn log_count(app: &App, text: &str) -> usize {
    app.connection_log()
        .iter()
        .filter(|line| line.ends_with(text))
        .count()
}

#[test]
fn a_lost_uplink_is_shown_immediately_and_a_blip_rides_through() {
    let lab = Lab::new(false);
    let (up, state) = shared_uplink(true);
    let mut app = lab.connected_app_with_link(Some("wlo1".into()), state);
    lab.set_rx(100);
    lab.set_tx(50);

    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    assert!(!app.link_lost());
    assert_eq!(log_count(&app, "link down (wlo1)"), 0);
    let calls_before = lab.calls();

    // The Wi-Fi drops: the state machine stays Connected and says so at once.
    set_uplink(&up, false);
    let t1 = t0 + Duration::from_secs(1);
    app.tick_at(t1).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(app.link_lost());
    assert_eq!(app.current_session().link_downs, 1);
    assert_eq!(app.status_text(), LINK_LOST);
    assert_eq!(log_count(&app, "link down (wlo1)"), 1);
    assert_eq!(lab.calls(), calls_before, "link loss asked for root");
    assert!(
        !lab.calls().contains("down"),
        "link loss tore the tunnel down"
    );

    // Still down: no duplicate line, no second counter.
    app.tick_at(t1 + Duration::from_secs(1)).unwrap();
    assert_eq!(app.current_session().link_downs, 1);
    assert_eq!(log_count(&app, "link down (wlo1)"), 1);

    // Back after 2 s: the flag clears, the tunnel never dropped.
    set_uplink(&up, true);
    let t2 = t1 + Duration::from_secs(2);
    app.tick_at(t2).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.link_lost());
    assert_eq!(app.current_session().link_ups, 1);
    assert_eq!(app.current_session().drops, 0);
    assert_eq!(app.status_text(), "Connected");
    assert_eq!(log_count(&app, "link up (wlo1)"), 1);
    assert_eq!(lab.calls(), calls_before, "link restore asked for root");

    // Still up: no duplicate restore line.
    app.tick_at(t2 + Duration::from_secs(1)).unwrap();
    assert_eq!(app.current_session().link_ups, 1);
    assert_eq!(log_count(&app, "link up (wlo1)"), 1);
}

#[test]
fn the_ordinary_flat_rx_stall_still_drops_and_retries() {
    let lab = Lab::new(false);
    let (_up, state) = shared_uplink(true);
    let mut app = lab.connected_app_with_link(Some("wlo1".into()), state);
    lab.set_rx(100);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();

    let almost = t0 + RX_STALL_TIMEOUT - Duration::from_secs(1);
    assert_eq!(app.tick_at(almost).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.link_lost());

    let ev = app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap();
    assert!(is_disconnect(&ev), "got {ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(app
        .status_text_at(t0 + RX_STALL_TIMEOUT)
        .starts_with("Connection lost - retrying in "));
    assert!(
        !app.link_lost(),
        "a non-Connected state clears the link flag"
    );
    assert_eq!(app.current_session().link_downs, 0);
}

#[test]
fn a_loopback_pin_never_runs_ip_for_the_uplink_watch() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ip-invoked");
    let ip = dir.path().join("fake-ip.sh");
    std::fs::write(
        &ip,
        format!(
            "#!/bin/sh\necho invoked >> \"{}\"\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ip, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .ip_bin(ip)
        .iface_pin(Ipv4Addr::LOCALHOST)
        .rx_bytes_path(dir.path().join("missing-rx"))
        .tx_bytes_path(dir.path().join("missing-tx"))
        .polkit_rules_path(dir.path().join("rules"))
        .installed_conf_path(dir.path().join("jafn0.conf"))
        .build();
    app.orchestrator_mut().confirm_connected();
    assert_eq!(app.state(), ConnectionState::Connected);

    app.tick_at(Instant::now()).unwrap();
    assert!(
        !marker.exists(),
        "the uplink watch ran ip for a loopback peer"
    );
    assert!(!app.link_lost());
    assert_eq!(app.current_session().link_downs, 0);
}

#[test]
fn the_uplink_is_sampled_at_most_once_per_interval() {
    let lab = Lab::new(false);
    let up = Arc::new(Mutex::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let up_seen = up.clone();
    let calls_seen = calls.clone();
    let state: LinkStateFn = Arc::new(move |_| {
        calls_seen.fetch_add(1, Ordering::SeqCst);
        Some(*up_seen.lock().unwrap())
    });
    let mut app = lab.connected_app_with_link(Some("wlo1".into()), state);
    lab.set_rx(100);

    // The first Connected tick samples immediately.
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 100 ms later is inside the interval: no second sample.
    app.tick_at(t0 + Duration::from_millis(100)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!app.link_lost());

    // 500 ms after the first sample: sample again.
    app.tick_at(t0 + LINK_POLL_INTERVAL).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!app.link_lost());

    // Down is not reported until the next scheduled sample...
    *up.lock().unwrap() = false;
    let within = t0 + LINK_POLL_INTERVAL + Duration::from_millis(100);
    app.tick_at(within).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!app.link_lost());

    // ...and that sample reports it immediately.
    let next = t0 + LINK_POLL_INTERVAL + LINK_POLL_INTERVAL;
    app.tick_at(next).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(app.link_lost());
    assert_eq!(app.current_session().link_downs, 1);
}

/// The App's next Joiner Hello nonce on the known track.
fn app_hello_nonce(
    app: &mut App,
    listener: &jafnify_core::known::KnownSocket,
    now: Instant,
) -> [u8; 16] {
    // Drop older frames first: only the Hello of this tick counts.
    while listener
        .receive(Duration::from_millis(5))
        .unwrap()
        .is_some()
    {}
    app.tick_at(now).unwrap();
    let end = Instant::now() + Duration::from_millis(300);
    let mut last = None;
    while Instant::now() < end {
        if let Some((_, jafnify_core::known::Frame::Hello(h))) =
            listener.receive(Duration::from_millis(20)).unwrap()
        {
            if h.role == Role::Joiner {
                last = Some(h.nonce);
            }
        }
    }
    last.expect("the App never sent a Hello")
}

/// A live case: the phone stopped sharing (Bye) and started again. The
/// laptop found it but reused jafn0, whose WireGuard session keys belonged
/// to the phone's old gateway, and waited in Connecting with no data. After
/// a Bye the next bring-up must set jafn0 up again, not reuse it.
#[test]
fn after_the_peers_bye_the_reconnect_sets_the_tunnel_up_again() {
    use jafnify_core::known::{KnownSocket, PairSecret, SignalKind};
    use jafnify_linux::app::RETRY_AFTER_RESET;
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let rec = pair_record(&own, &peer.public_key_b64(), Role::Joiner, None, 1).unwrap();
    save_state(
        &lab.state_path(),
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
    let peer_key = peer.public_key_b64();
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    lab.set_rx(100);
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let listener = KnownSocket::bind(&DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    })
    .unwrap();
    let secret = PairSecret::derive(&peer, &own.public_key_b64()).unwrap();
    let send = |frame: &[u8]| {
        listener.send(frame).unwrap();
        std::thread::sleep(Duration::from_millis(40));
    };

    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    // The phone stops: its Bye drops the laptop at once.
    let phone_old = [61u8; 16];
    send(&secret.hello(Role::Sharer, phone_old).encode());
    // Connected tracks Hello every 5 s.
    let ta = t0 + Duration::from_millis(5_100);
    lab.set_rx(102);
    let echo = app_hello_nonce(&mut app, &listener, ta);
    let bye = secret.signal(
        SignalKind::Bye,
        Role::Sharer,
        phone_old,
        echo,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
    );
    send(&bye.encode());
    assert!(is_disconnect(
        &app.tick_at(ta + Duration::from_millis(20)).unwrap()
    ));
    assert_eq!(app.state(), ConnectionState::Retry);

    // The phone shares again; the laptop searches and finds its Proof.
    let t1 = ta + Duration::from_millis(20) + RETRY_AFTER_RESET;
    app.tick_at(t1).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    let before = lab.calls();
    let phone_new = [62u8; 16];
    send(&secret.hello(Role::Sharer, phone_new).encode());
    let echo = app_hello_nonce(&mut app, &listener, t1 + Duration::from_secs(1));
    let proof = secret.proof_on(
        Role::Sharer,
        phone_new,
        echo,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
        9797,
    );
    send(&proof.encode());
    app.tick_at(t1 + Duration::from_secs(2)).unwrap();
    assert_eq!(
        app.state(),
        ConnectionState::Connecting,
        "calls: {}",
        lab.calls()
    );
    let after = lab.calls();
    let added = after.strip_prefix(before.as_str()).unwrap_or(&after);
    assert!(
        added.lines().any(|l| l.contains(" up")),
        "the tunnel was not set up again after the peer's Bye: {added:?}"
    );
}

/// A live case: the old tray said Bye on SIGTERM, the phone restarted its
/// gateway, and the new tray adopted jafn0 with the old session keys. The
/// phone's Fresh must set the adopted tunnel up again at once instead of
/// waiting ~15 s for WireGuard's own re-handshake.
#[test]
fn a_fresh_peer_while_an_adopted_tunnel_waits_sets_it_up_again() {
    use jafnify_core::known::{KnownSocket, PairSecret, SignalKind};
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    let rec = pair_record(&own, &peer.public_key_b64(), Role::Joiner, None, 1).unwrap();
    save_state(
        &lab.state_path(),
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
    let peer_key = peer.public_key_b64();
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    lab.set_rx(100);
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let listener = KnownSocket::bind(&DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    })
    .unwrap();
    let secret = PairSecret::derive(&peer, &own.public_key_b64()).unwrap();
    let send = |frame: &[u8]| {
        listener.send(frame).unwrap();
        std::thread::sleep(Duration::from_millis(40));
    };

    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting, "adopted");
    let t0 = Instant::now();
    let before = lab.calls();
    let phone_new = [71u8; 16];
    send(&secret.hello(Role::Sharer, phone_new).encode());
    let echo = app_hello_nonce(&mut app, &listener, t0);
    let fresh = secret.signal(
        SignalKind::Fresh,
        Role::Sharer,
        phone_new,
        echo,
        &peer.public_key_bytes(),
        &own.public_key_bytes(),
    );
    send(&fresh.encode());
    app.tick_at(t0 + Duration::from_millis(100)).unwrap();
    let after = lab.calls();
    let added = after.strip_prefix(before.as_str()).unwrap_or(&after);
    assert!(
        added.lines().any(|l| l.contains(" up")),
        "the adopted tunnel was not set up again: {added:?}"
    );
}

/// The hotspot vanishes for good: the connected session drops once, then the
/// search waits quietly for the network instead of cycling Searching -> Retry
/// every 10 s. A long outage must not push the drop out of the bounded
/// connection log, and the return is one line.
#[test]
fn a_long_outage_with_no_network_waits_quietly_and_keeps_the_drop_in_the_log() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let peer_key = lab.store_pair(Role::Joiner);
    lab.set_rx(100);
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    app.tick_at(t0).unwrap();

    // The network goes: the next connected Hello (5 s) fails and the session drops.
    app.fail_known_sends(Some(std::io::ErrorKind::NetworkUnreachable));
    let t1 = t0 + Duration::from_millis(5_100);
    let ev = app.tick_at(t1).unwrap();
    assert!(is_disconnect(&ev), "got {ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    let log = app.connection_log();
    let drop_line = log
        .iter()
        .find(|l| l.contains("Connected -> Retry"))
        .unwrap_or_else(|| panic!("the drop is not logged: {log:#?}"))
        .clone();

    // 50 minutes with no network, ticking once a second.
    let mut t = t1;
    for _ in 0..3_000 {
        t += Duration::from_secs(1);
        app.tick_at(t).unwrap();
    }
    assert_eq!(
        app.state(),
        ConnectionState::Searching,
        "waits, no Retry cycle"
    );
    assert!(app.no_network(), "the window says why it waits");
    let log = app.connection_log();
    assert!(
        log.contains(&drop_line),
        "the drop was pushed out: {log:#?}"
    );
    assert!(
        log.len() < 20,
        "the outage flooded the log: {} lines",
        log.len()
    );
    assert_eq!(
        log.iter().filter(|l| l.contains("no network")).count(),
        1,
        "{log:#?}"
    );

    // The network returns: one line says so, and the search goes on.
    app.fail_known_sends(None);
    t += Duration::from_secs(1);
    app.tick_at(t).unwrap();
    let log = app.connection_log();
    assert!(
        log.last().is_some_and(|l| l.contains("network back after")),
        "{log:#?}"
    );
    assert_eq!(app.state(), ConnectionState::Searching);
    assert!(!app.no_network());
}

/// The network comes back but the old known track cannot send (pinned to an
/// address the computer no longer has). The wait opens the track again on the
/// current addresses, silently, and the search goes on without a user action.
#[test]
fn a_track_that_cannot_send_after_the_outage_is_opened_again() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let peer_key = lab.store_pair(Role::Joiner);
    lab.set_rx(100);
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    // The network goes: the session drops, the retry finds the search.
    app.fail_known_sends(Some(std::io::ErrorKind::NetworkUnreachable));
    let mut t = t0 + Duration::from_millis(5_100);
    assert!(is_disconnect(&app.tick_at(t).unwrap()));
    for _ in 0..30 {
        t += Duration::from_secs(1);
        app.tick_at(t).unwrap();
    }
    assert_eq!(app.state(), ConnectionState::Searching);

    // Back, but the current track stays broken; a new one would work.
    app.fail_known_sends(None);
    app.fail_current_known_track(Some(std::io::ErrorKind::AddrNotAvailable));
    for _ in 0..12 {
        t += Duration::from_secs(1);
        app.tick_at(t).unwrap();
    }
    let log = app.connection_log();
    assert!(
        log.last().is_some_and(|l| l.contains("network back after")),
        "the track was never opened again: {log:#?}"
    );
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_eq!(
        log.iter().filter(|l| l.contains("no network")).count(),
        1,
        "{log:#?}"
    );
}

/// The connection log survives a restart of the app: the earlier run (its
/// start, its end) is still above the new run's first line.
#[test]
fn the_connection_log_shows_the_earlier_run_after_a_restart() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let peer_key = lab.store_pair(Role::Joiner);
    lab.set_rx(100);
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    {
        let mut app = lab.app_with_helper(&live);
        app.start_known().unwrap();
        let t0 = Instant::now();
        lab.set_rx(101);
        app.tick_at(t0).unwrap();
        assert_eq!(app.state(), ConnectionState::Connected);
        app.stop().unwrap();
    }
    let mut app = lab.app_with_helper(&live);
    let earlier = app.connection_log();
    assert!(
        earlier
            .iter()
            .any(|l| l.ends_with("session started (joiner)")),
        "{earlier:#?}"
    );
    assert!(
        earlier
            .iter()
            .any(|l| l.ends_with("session ended (stopped)")),
        "{earlier:#?}"
    );
    app.start_known().unwrap();
    let log = app.connection_log();
    assert_eq!(
        &log[..earlier.len()],
        &earlier[..],
        "the earlier run stays on top"
    );
    assert!(
        log.last().unwrap().ends_with("session started (joiner)"),
        "{log:#?}"
    );
}

/// The laptop walks out of hotspot range and back. While the joiner waits for
/// the network it asks NetworkManager to scan every 10 s on the disconnected
/// Wi-Fi device only (NetworkManager alone backs off to one scan per 120 s
/// and missed the hotspot for minutes). Once the network is back it stops.
#[test]
fn a_joiner_with_no_network_asks_networkmanager_to_scan_until_it_returns() {
    let _g = multicast_lock();
    let lab = Lab::new(false);
    let peer_key = lab.store_pair(Role::Joiner);
    lab.set_rx(100);
    std::fs::write(
        lab.dir.path().join("cfg/tunnel.toml"),
        format!("peer_public_key = \"{peer_key}\"\n"),
    )
    .unwrap();
    let live = LiveConfig {
        peer_public_key: peer_key,
        endpoint: Some("127.0.0.1:9797".to_string()),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    };
    let calls = lab.dir.path().join("nmcli.calls");
    let nmcli = lab.dir.path().join("mock-nmcli.sh");
    std::fs::write(
        &nmcli,
        format!(
            "#!/bin/sh\necho \"$*\" >> \"{}\"\n\
             if [ \"$1\" = -t ]; then\n\
             printf 'wlo1:wifi:disconnected\\nwlx0:wifi:unavailable\\n\
             p2p-dev-wlo1:wifi-p2p:disconnected\\njafn0:wireguard:connected (externally)\\n'\n\
             fi\nexit 0\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&nmcli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut app = lab.helper_builder(&live).nmcli_bin(nmcli).build();
    app.start_known().unwrap();
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    let rescans = || {
        std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("device wifi rescan"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert!(rescans().is_empty(), "no scan while connected");

    app.fail_known_sends(Some(std::io::ErrorKind::NetworkUnreachable));
    let mut t = t0 + Duration::from_millis(5_100);
    assert!(is_disconnect(&app.tick_at(t).unwrap()));
    for _ in 0..60 {
        t += Duration::from_secs(1);
        app.tick_at(t).unwrap();
        assert!(app.wait_wifi_scan_idle(Duration::from_secs(5)));
    }
    assert!(app.no_network());
    let during = rescans();
    assert!(
        (5..=7).contains(&during.len()),
        "about one request per 10 s: {during:#?}"
    );
    assert!(
        during.iter().all(|l| l == "device wifi rescan ifname wlo1"),
        "only the disconnected Wi-Fi device: {during:#?}"
    );

    app.fail_known_sends(None);
    for _ in 0..60 {
        t += Duration::from_secs(1);
        app.tick_at(t).unwrap();
        assert!(app.wait_wifi_scan_idle(Duration::from_secs(5)));
    }
    assert!(!app.no_network());
    assert_eq!(
        rescans().len(),
        during.len(),
        "no scan once the network is back"
    );
}
