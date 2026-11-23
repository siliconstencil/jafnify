//! The drop history of the laptop: one drop per outage, one back when the
//! link comes back. The root calls are scripts, the rx counter is a file and
//! the clock is injected through `tick_at`, so the whole path runs in a test.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::pair_record;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, KeyPair, RETRY_INTERVAL};
use jafnify_linux::app::{App, AppBuilder, UiEvent};
use jafnify_linux::helper::LiveConfig;
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fakes = [
            // The helper must really run: the App reads the live config
            // through it to adopt the tunnel.
            (
                "mock-pkexec.sh",
                "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in *jafnify-route*|*route-fallback*|*route-restore*|*resolvectl*|*jafn0*fwmark*) exit 0;; esac; done\nexec \"$@\"\n".to_string(),
            ),
            ("mock-wg.sh", "#!/bin/sh\nexit 1\n".to_string()),
            ("mock-wg-quick.sh", "#!/bin/sh\nexit 0\n".to_string()),
            ("mock-install.sh", "#!/bin/sh\nexit 0\n".to_string()),
            // No host `ip` may run in a test.
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

    /// The helper reports this live config, so the App uses the tunnel.
    fn helper(&self, live: &LiveConfig) -> PathBuf {
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
        helper
    }

    fn app(&self, helper: &Path) -> App {
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .ip_bin(p("mock-ip.sh"))
            .helper_bin(helper.to_path_buf())
            .connection_log_path(Some(self.dir.path().join("connection.log")))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(self.sessions_path())
            .polkit_rules_path(self.dir.path().join("rules"))
            .installed_conf_path(self.dir.path().join("jafn0.conf"))
            .build()
    }
}

/// An App that reaches Connected through the known-peer path.
///
/// The tx counter file is never written: the App can only read the rx
/// counter, so the session sampler cannot run at all. That is the field
/// condition of the missed outage: the link is lost while the session
/// counters cannot be read.
fn connected_app(lab: &Lab) -> App {
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
    let mut app = lab.app(&lab.helper(&live));
    app.start_known().unwrap();
    assert_eq!(
        app.state(),
        ConnectionState::Connecting,
        "log: {:?} error: {:?}",
        app.connection_log(),
        app.last_error()
    );
    let t0 = Instant::now();
    lab.set_rx(101);
    app.tick_at(t0).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    app
}

fn is_disconnect(ev: &Option<UiEvent>) -> bool {
    matches!(ev, Some(UiEvent::Disconnected { .. }))
}

/// The hotspot is gone: every send of the known track fails with "network
/// unreachable". The App notices on the next tick that sends.
fn lose_the_network(app: &mut App, start: Instant) -> Instant {
    app.fail_known_sends(Some(std::io::ErrorKind::NetworkUnreachable));
    let mut now = start + Duration::from_secs(1);
    for _ in 0..20 {
        let ev = app.tick_at(now).unwrap();
        if is_disconnect(&ev) {
            assert_eq!(app.state(), ConnectionState::Retry);
            return now;
        }
        now += Duration::from_secs(1);
    }
    panic!("the app did not notice the lost network");
}

#[test]
fn the_no_network_outage_is_one_drop_and_one_back() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);
    let records = app.drop_records();
    assert_eq!(records.len(), 1, "one drop for the outage: {records:?}");
    assert_eq!(records[0].reason, "no-network");
    assert_eq!(records[0].down_ms, None, "the link is still down");

    // Retry -> Searching -> Connecting is the same outage: no second drop.
    let searching_at = dropped_at + RETRY_INTERVAL;
    app.tick_at(searching_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_eq!(app.drop_records().len(), 1, "a second drop for one outage");

    // The network is back and the peer answers: the link returns.
    app.fail_known_sends(None);
    app.orchestrator_mut().known_peer_found();
    let back_at = dropped_at + Duration::from_secs(2640);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let records = app.drop_records();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].reason, "no-network");
    assert_eq!(
        records[0].down_ms,
        Some(2_640_000),
        "the down time is from the drop to the return"
    );
}

/// An absence of the phone that lasts longer than the edge is not a fault:
/// it is an away event, it is not counted as a drop and the return is quiet.
#[test]
fn a_long_absence_is_an_away_and_not_a_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);

    // The laptop keeps ticking while the phone is away. Past the edge the
    // outage becomes an away event of its own.
    app.tick_at(dropped_at + Duration::from_secs(121)).unwrap();
    let events = app.drop_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].kind(),
        jafnify_core::drop_history::EventKind::Away
    );
    assert!(
        app.drop_records().is_empty(),
        "an absence is not a drop: {:?}",
        app.drop_records()
    );

    // The phone comes back on its own: the absence closes with its length.
    app.fail_known_sends(None);
    app.orchestrator_mut().known_peer_found();
    let back_at = dropped_at + Duration::from_secs(2640);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let events = app.drop_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].kind(),
        jafnify_core::drop_history::EventKind::Away
    );
    assert_eq!(
        events[0].down_ms(),
        Some(2_640_000),
        "the length of the absence is the length of the outage"
    );
    assert!(
        app.drop_records().is_empty(),
        "an absence is never counted as a drop"
    );
    let log = app.connection_log();
    assert!(
        log.iter().any(|line| line.contains("Phone away")),
        "the log says the phone was away: {log:?}"
    );
}

/// A loss that is shorter than the edge stays a real drop, with the hint of
/// the hotspot of the phone.
#[test]
fn a_short_absence_stays_a_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);
    // The laptop ticks while the phone is away, but the phone comes back
    // before the edge.
    app.tick_at(dropped_at + Duration::from_secs(30)).unwrap();

    app.fail_known_sends(None);
    app.orchestrator_mut().known_peer_found();
    let back_at = dropped_at + Duration::from_secs(45);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let records = app.drop_records();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].reason, "no-network");
    assert_eq!(records[0].down_ms, Some(45_000));
    assert_eq!(
        records[0].hint(),
        "likely the phone's hotspot went away",
        "the short loss keeps the hint of the hotspot"
    );
}

/// The phone stopped sharing: the laptop records a stop on the phone, not a
/// drop.
#[test]
fn a_bye_is_a_stop_on_the_phone_and_not_a_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    let t0 = Instant::now();
    app.peer_said_bye(t0);

    let events = app.drop_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].kind(),
        jafnify_core::drop_history::EventKind::Stopped
    );
    assert!(
        app.drop_records().is_empty(),
        "a stop on the phone is not a drop"
    );
}

#[test]
fn the_link_back_on_any_path_closes_the_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);
    assert_eq!(app.drop_records().len(), 1, "one drop for the outage");

    // The link comes back on a path that does not go through the known
    // track: the app is Connected without `known_connected` running.
    app.fail_known_sends(None);
    app.orchestrator_mut().confirm_connected();
    let back_at = dropped_at + Duration::from_secs(60);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let records = app.drop_records();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].down_ms,
        Some(60_000),
        "the drop closes on every path back"
    );
}

#[test]
fn the_no_data_stall_is_one_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    // The peer goes quiet: no data for the stall timeout.
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    let ev = app
        .tick_at(t0 + jafnify_linux::app::RX_STALL_TIMEOUT)
        .unwrap();
    assert!(is_disconnect(&ev), "{ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);

    let records = app.drop_records();
    assert_eq!(
        records.len(),
        1,
        "session: {:?} log: {:?}",
        app.current_session(),
        app.connection_log()
    );
    assert_eq!(records[0].reason, "no-data");
}

#[test]
fn a_stop_records_no_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connected_app(&lab);
    app.set_drop_history_enabled(true);

    app.stop().unwrap();
    assert!(app.drop_records().is_empty());
    assert!(
        !lab.dir.path().join("cfg/drops.tsv").exists(),
        "the user stop wrote a drop"
    );
}

/// The connection log gets its "reconnected at" line when the link comes
/// back after a logged drop. The line carries the time and no endpoint.
#[test]
fn a_logged_drop_is_closed_with_a_reconnected_line() {
    let _g = multicast_lock();
    let lab = Lab::new();
    // The session tracker needs both counters: the drop line of the
    // connection log comes from it.
    lab.set_tx(0);
    let mut app = connected_app(&lab);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);
    let log = app.connection_log();
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("disconnected at "))
            .count(),
        1,
        "{log:?}"
    );

    // The link comes back on a path that does not go through
    // `known_connected`: the log still gets its line.
    app.fail_known_sends(None);
    app.orchestrator_mut().confirm_connected();
    let back_at = dropped_at + Duration::from_secs(60);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let log = app.connection_log();
    let reconnects: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("reconnected at "))
        .collect();
    assert_eq!(reconnects.len(), 1, "{log:?}");
    let line = reconnects[0];
    assert!(
        !line.contains("(peer"),
        "the line carries an endpoint: {line}"
    );
    assert!(
        !line.contains("127.0.0.1"),
        "the line carries an address: {line}"
    );
    // The same local time style as the line's own timestamp.
    let stamp = line.split(" reconnected at ").next().unwrap();
    assert!(line.ends_with(stamp), "{line}");
}

/// The peer reconnect that goes through `known_connected` writes the
/// endpoint-free "reconnected at" line into the connection log. This is the
/// real path: the peer answers again and the app connects through the known
/// track.
#[test]
fn the_peer_reconnect_writes_one_reconnected_line_without_the_endpoint() {
    let _g = multicast_lock();
    let lab = Lab::new();
    // The session tracker needs both counters: the drop line of the
    // connection log comes from it.
    lab.set_tx(0);
    let mut app = connected_app(&lab);

    let t0 = Instant::now();
    let dropped_at = lose_the_network(&mut app, t0);
    let log = app.connection_log();
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("disconnected at "))
            .count(),
        1,
        "{log:?}"
    );

    // The retry search runs again and the peer answers: the app comes back
    // through `known_connected`.
    let searching_at = dropped_at + RETRY_INTERVAL;
    app.tick_at(searching_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    app.fail_known_sends(None);
    app.orchestrator_mut().known_peer_found();
    let back_at = dropped_at + Duration::from_secs(60);
    lab.set_rx(200);
    app.tick_at(back_at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    let log = app.connection_log();
    let reconnects: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("reconnected at "))
        .collect();
    assert_eq!(reconnects.len(), 1, "{log:?}");
    let line = reconnects[0];
    assert!(
        !line.contains("(peer"),
        "the line carries an endpoint: {line}"
    );
    assert!(
        !line.contains("127.0.0.1"),
        "the line carries an address: {line}"
    );
    // The same local time style as the line's own timestamp.
    let stamp = line.split(" reconnected at ").next().unwrap();
    assert!(line.ends_with(stamp), "{line}");
}

/// A quick stop and reconnect within seconds after an update of the phone
/// app is the same session: its id, its
/// start, its bytes and its drop count carry on, the history stores it once
/// and the stopped gap is not a drop.
#[test]
fn a_quick_stop_and_connect_continues_the_session() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.set_tx(10);
    let mut app = connected_app(&lab);
    let t0 = Instant::now();
    lab.set_rx(5_101);
    lab.set_tx(2_010);
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    let before = app.current_session();
    assert!(
        before.down_bytes > 0,
        "the first session moved data: {before:?}"
    );

    app.stop().unwrap();
    app.start_known().unwrap();

    let after = app.current_session();
    assert_eq!(after.id, before.id, "a quick Connect keeps the session");
    assert_eq!(after.started_at, before.started_at);
    assert_eq!(after.down_bytes, before.down_bytes);
    assert_eq!(after.drops, 0, "a stop is not a drop: {after:?}");
    assert!(
        app.session_history().iter().all(|r| r.id != before.id),
        "the resumed session is not stored as ended"
    );
    assert!(
        app.connection_log()
            .iter()
            .any(|line| line.contains("session resumed")),
        "the log says the session was continued"
    );

    // The new tunnel counts from zero: the next samples add to the session
    // and the gap is no connected time.
    lab.set_rx(0);
    lab.set_tx(0);
    let t1 = Instant::now();
    app.tick_at(t1).unwrap();
    lab.set_rx(1_000);
    lab.set_tx(500);
    app.tick_at(t1 + Duration::from_secs(1)).unwrap();
    let later = app.current_session();
    assert_eq!(later.id, before.id);
    assert_eq!(later.drops, 0, "{later:?}");
    assert!(
        later.down_bytes >= before.down_bytes + 1_000,
        "bytes keep adding up: {later:?}"
    );

    // When it really ends, the history holds it once.
    app.stop().unwrap();
    let stored: Vec<_> = app
        .session_history()
        .into_iter()
        .filter(|r| r.id == before.id)
        .collect();
    assert_eq!(stored.len(), 1, "one record for one session");
}
