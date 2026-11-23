//! App journal integration: cadence, STATS fields and the decision events.
//!
//! Everything is injected: the rx/tx counters are files, `ip`/`pkexec`/
//! `wg-quick` are fake scripts, the clock is `tick_at`, the carrier/operstate
//! live in a temp sysfs tree and the journal is a temp path. The known-peer
//! reset test uses loopback multicast, exactly like `peer_reset_test`; no host
//! interface, `ip`, `wg` or firewall is touched.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::{
    pair_record, Frame, KnownSocket, PairSecret, Proof, NONCE_ROTATE_INTERVAL,
};
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, DiscoveryConfig, KeyPair};
use jafnify_linux::app::{App, AppBuilder, UiEvent, RESET_NEUTRAL_GRACE, RX_STALL_TIMEOUT};
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};

const EPOCH_MS: i64 = 1_700_000_000_000;

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    }
}

struct Lab {
    dir: tempfile::TempDir,
    own: Option<KeyPair>,
    peer: Option<KeyPair>,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let lab = Self {
            dir,
            own: None,
            peer: None,
        };
        lab.script(
            "mock-pkexec.sh",
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n",
                lab.dir.path().join("calls.log").display()
            ),
        );
        lab.script("mock-wg.sh", "#!/bin/sh\nexit 1\n".to_string());
        lab.script("mock-wg-quick.sh", "#!/bin/sh\nexit 0\n".to_string());
        lab.script("mock-install.sh", "#!/bin/sh\nexit 0\n".to_string());
        // A read-only fake `ip`: route get has a source, anything else is a
        // harmless non-rule line. No host process is ever run.
        lab.script(
            "mock-ip.sh",
            "#!/bin/sh\necho '1.2.3.4 dev test0 src 10.0.0.5 uid 1000'\nexit 0\n".to_string(),
        );
        let sysfs = lab.dir.path().join("sysfs/test0");
        std::fs::create_dir_all(&sysfs).unwrap();
        std::fs::write(sysfs.join("carrier"), "1\n").unwrap();
        std::fs::write(sysfs.join("operstate"), "up\n").unwrap();
        lab
    }

    fn script(&self, name: &str, body: String) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn diag_path(&self) -> PathBuf {
        self.dir.path().join("state/jafnify/diag.log")
    }

    fn rx_path(&self) -> PathBuf {
        self.path("rx_bytes")
    }

    fn tx_path(&self) -> PathBuf {
        self.path("tx_bytes")
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(self.rx_path(), format!("{n}\n")).unwrap();
    }

    fn set_tx(&self, n: u64) {
        std::fs::write(self.tx_path(), format!("{n}\n")).unwrap();
    }

    fn diag_lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.diag_path())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn diag_with(&self, needle: &str) -> Vec<String> {
        self.diag_lines()
            .into_iter()
            .filter(|line| line.contains(needle))
            .collect()
    }

    fn stats_count(&self) -> usize {
        self.diag_with("kind=STATS").len()
    }

    fn builder(&self, link: bool) -> AppBuilder {
        let p = |name: &str| self.path(name);
        let mut builder = AppBuilder::new(self.path("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .ip_bin(p("mock-ip.sh"))
            .helper_bin(p("missing-helper"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(self.tx_path())
            .sessions_path(p("sessions.toml"))
            .session_epoch_ms(EPOCH_MS)
            .polkit_rules_path(p("rules"))
            .installed_conf_path(p("jafn0.conf"))
            .uplink_sysfs(self.path("sysfs"))
            .ufw_conf(p("missing-ufw.conf"))
            .diag_path(Some(self.diag_path()));
        if link {
            builder = builder
                .link_iface(Some("test0".to_string()))
                .link_state(Arc::new(|_| Some(true)));
        }
        builder
    }

    /// A Connected app on the code track, driven straight to the state the
    /// diagnostic hooks observe (the tunnel itself is not needed).
    fn connected_code_app(&self) -> App {
        let mut app = self.builder(true).build();
        app.start("1234").unwrap();
        app.accept_code("1234").unwrap();
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);
        app
    }

    fn store_pair(&mut self) {
        let own = KeyPair::generate();
        let peer = KeyPair::generate();
        let rec = pair_record(&own, &peer.public_key_b64(), Role::Joiner, None, 1).unwrap();
        save_state(
            &self.path("cfg/state.toml"),
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
        self.own = Some(own);
        self.peer = Some(peer);
    }

    /// A Connected app on the known track (a stored pair), with rx already at
    /// one so the shared peer key can be used without the tunnel.
    fn connected_known_app(&self) -> App {
        self.set_rx(1);
        self.set_tx(0);
        let mut app = self.builder(false).build();
        app.start_known().unwrap();
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);
        app
    }

    fn secret(&self) -> PairSecret {
        PairSecret::derive(
            self.peer.as_ref().unwrap(),
            &self.own.as_ref().unwrap().public_key_b64(),
        )
        .unwrap()
    }

    fn proof(&self, n_self: [u8; 16], n_echo: [u8; 16]) -> Proof {
        self.secret().proof_on(
            Role::Sharer,
            n_self,
            n_echo,
            &self.peer.as_ref().unwrap().public_key_bytes(),
            &self.own.as_ref().unwrap().public_key_bytes(),
            9797,
        )
    }
}

#[test]
fn stats_are_five_seconds_then_one_second_after_peer_rx_is_flat() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.set_tx(50);
    let mut app = lab.connected_code_app();
    let t0 = Instant::now();

    // Fresh peer data: stats every 5 s.
    lab.set_rx(100);
    app.tick_at(t0).unwrap();
    for (i, rx) in [101u64, 102, 103, 104, 105].into_iter().enumerate() {
        lab.set_rx(rx);
        app.tick_at(t0 + Duration::from_secs(i as u64 + 1)).unwrap();
    }
    assert_eq!(lab.stats_count(), 2, "{:?}", lab.diag_lines());

    let first_stats = lab.diag_with("kind=STATS").remove(0);
    for field in [
        "control_rx_packets=",
        "control_rx_bytes=",
        "control_tx_packets=",
        "control_tx_bytes=",
        "control_send_errors=",
        "control_recv_errors=",
        "control_last_errno=",
        "last_rx_ms=",
        "last_tx_ms=",
        "peer=-1",
        "local=127.0.0.1",
        "wg_handshake_ms=-1",
        "wg_rx_bytes=100",
        "wg_tx_bytes=50",
        "egress_iface=test0",
        "egress_src=-1",
        "carrier=1",
        "operstate=up",
        "state=connected",
    ] {
        assert!(
            first_stats.contains(field),
            "{field} missing: {first_stats}"
        );
    }

    // Peer rx is now flat. The 5 s window still governs until it is older
    // than 3 s; the next records then come one second apart.
    for seconds in [6u64, 7, 8] {
        app.tick_at(t0 + Duration::from_secs(seconds)).unwrap();
    }
    assert_eq!(lab.stats_count(), 2, "a flat rx wrote too early");
    app.tick_at(t0 + Duration::from_secs(9)).unwrap();
    assert_eq!(lab.stats_count(), 3, "{:?}", lab.diag_lines());
    app.tick_at(t0 + Duration::from_secs(10)).unwrap();
    assert_eq!(
        lab.stats_count(),
        4,
        "the flat cadence is not one second: {:?}",
        lab.diag_lines()
    );
}

#[test]
fn the_no_data_decision_is_an_event_with_counters_and_drop_timing_is_unchanged() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.set_rx(100);
    lab.set_tx(50);
    let mut app = lab.connected_code_app();
    let t0 = Instant::now();

    app.tick_at(t0).unwrap();
    let almost = t0 + RX_STALL_TIMEOUT - Duration::from_secs(1);
    assert_eq!(app.tick_at(almost).unwrap(), None);
    assert_eq!(
        app.state(),
        ConnectionState::Connected,
        "the drop moved earlier"
    );

    let event = app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap();
    assert!(
        matches!(event, Some(UiEvent::Disconnected { .. })),
        "no drop at the unchanged 35 s boundary: {event:?}"
    );
    assert_eq!(app.state(), ConnectionState::Retry);

    let no_data = lab.diag_with("event=no_data");
    assert_eq!(no_data.len(), 1, "{:?}", lab.diag_lines());
    let line = &no_data[0];
    assert!(line.contains("reason=no_data_from_peer_for_35_s"), "{line}");
    assert!(line.contains("wg_rx_bytes=100"), "{line}");
    assert!(line.contains("wg_tx_bytes=50"), "{line}");
    assert!(line.contains("control_rx_packets="), "{line}");

    assert!(
        lab.diag_lines()
            .iter()
            .any(|line| line.contains("event=state")
                && line.contains("from=Connected")
                && line.contains("to=Retry")),
        "the state transition is not an event: {:?}",
        lab.diag_lines()
    );
}

fn deliver(listener: &KnownSocket, proof: &Proof) {
    listener.send(&proof.encode()).unwrap();
    std::thread::sleep(Duration::from_millis(40));
}

/// Tick once so the App sends a Hello, returning that Hello's nonce.
fn capture_own_nonce(app: &mut App, listener: &KnownSocket, now: Instant) -> [u8; 16] {
    app.tick_at(now).unwrap();
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        if let Some((_, Frame::Hello(hello))) = listener.receive(Duration::from_millis(20)).unwrap()
        {
            if hello.role == Role::Joiner {
                return hello.nonce;
            }
        }
    }
    panic!("the App never sent a Hello");
}

/// Teach the App a prior peer nonce, then tick and return its own Hello nonce.
fn capture_own_nonce_with_prior_peer(
    app: &mut App,
    listener: &KnownSocket,
    lab: &Lab,
    prior: [u8; 16],
    now: Instant,
) -> [u8; 16] {
    listener
        .send(&lab.secret().hello(Role::Sharer, prior).encode())
        .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    capture_own_nonce(app, listener, now)
}

/// Settle the track on one normal rotation so the next unknown nonce is
/// unambiguous reset evidence.
fn settle_with_normal_rotation(
    app: &mut App,
    listener: &KnownSocket,
    lab: &Lab,
    echo: [u8; 16],
    at: Instant,
) {
    let normal = [50u8; 16];
    listener
        .send(&lab.secret().hello(Role::Sharer, normal).encode())
        .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    app.tick_at(at).unwrap();
    listener.send(&lab.proof(normal, echo).encode()).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    app.tick_at(at + Duration::from_millis(10)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.peer_reset_pending());
}

#[test]
fn a_peer_session_reset_is_an_event_on_the_known_reset_path() {
    let _g = multicast_lock();
    let mut lab = Lab::new();
    lab.store_pair();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_known_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, [41u8; 16], t0);

    let settle_at = t0 + NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
    settle_with_normal_rotation(&mut app, &listener, &lab, echo, settle_at);

    // A valid Proof from a nonce this session never saw is reset evidence.
    deliver(&listener, &lab.proof([42u8; 16], echo));
    let hinted_at = settle_at + Duration::from_secs(1);
    app.tick_at(hinted_at).unwrap();
    assert!(app.peer_reset_pending(), "no reset hint from a new nonce");
    assert!(
        lab.diag_with("event=peer_reset")
            .iter()
            .any(|line| line.contains("reason=proof_nonce")),
        "the reset hint is not an event: {:?}",
        lab.diag_lines()
    );

    // No data inside the grace window: the reset-specific drop is an event too.
    let event = app.tick_at(hinted_at + RESET_NEUTRAL_GRACE).unwrap();
    assert!(
        matches!(event, Some(UiEvent::Disconnected { .. })),
        "{event:?}"
    );
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(
        lab.diag_with("event=peer_reset")
            .iter()
            .any(|line| line.contains("reason=no_data_after_grace")),
        "the reset drop is not an event: {:?}",
        lab.diag_lines()
    );
}

#[test]
fn every_record_has_one_kind_token_and_probes_use_source_and_frame() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.builder(true).build();
    app.start("1234").unwrap();
    let t0 = Instant::now();
    app.tick_at(t0).unwrap(); // 0x01 discovery announce
    app.accept_code("1234").unwrap();
    app.tick_at(t0 + Duration::from_secs(1)).unwrap(); // 0x02 handshake announce

    let lines = lab.diag_lines();
    assert!(!lines.is_empty());
    for line in &lines {
        let kinds: Vec<&str> = line
            .split_whitespace()
            .filter(|token| token.starts_with("kind="))
            .collect();
        assert_eq!(kinds.len(), 1, "record has {kinds:?}: {line}");
        assert!(
            matches!(kinds[0], "kind=STATS" | "kind=EVENT"),
            "reserved kind reused as payload: {line}"
        );
    }

    let probes = lab.diag_with("event=probe");
    assert!(
        probes
            .iter()
            .any(|line| line.contains("source=discovery") && line.contains("frame=0x01")),
        "{probes:?}"
    );
    assert!(
        probes
            .iter()
            .any(|line| line.contains("source=pake") && line.contains("frame=0x05")),
        "{probes:?}"
    );
    assert!(
        !probes
            .iter()
            .any(|line| { line.contains("kind=discovery") || line.contains("kind=handshake") }),
        "a probe reused the kind discriminator: {probes:?}"
    );
}

#[test]
fn one_interface_change_is_one_link_change_and_a_disconnect_adds_none() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.set_rx(100);
    lab.set_tx(50);
    let mut app = lab.connected_code_app();
    let t0 = Instant::now();

    app.tick_at(t0).unwrap();
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    let links = lab.diag_with("event=link_change");
    assert_eq!(links.len(), 1, "repeated samples: {:?}", lab.diag_lines());
    assert!(links[0].contains("reason=interface"), "{links:?}");
    assert!(links[0].contains("iface=test0"), "{links:?}");

    // The 35 s no-data drop clears the uplink watch; it must not add a
    // `link_change` for the missing interface.
    app.tick_at(t0 + RX_STALL_TIMEOUT).unwrap();
    assert_eq!(app.state(), ConnectionState::Retry);
    let links = lab.diag_with("event=link_change");
    assert_eq!(
        links.len(),
        1,
        "a disconnect added a link_change: {links:?}"
    );
    assert!(
        !links.iter().any(|line| line.contains("iface=-1")),
        "the cleared watch invented an interface: {links:?}"
    );
}

#[test]
fn start_and_stop_emit_session_events_beside_the_connection_log_lines() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.builder(false).build();

    app.start("1234").unwrap();
    let starts = lab.diag_with("event=session_start");
    assert_eq!(starts.len(), 1, "{:?}", lab.diag_lines());
    assert!(starts[0].contains("role=joiner"), "{}", starts[0]);
    assert!(
        app.connection_log()
            .iter()
            .any(|line| line.ends_with("session started (joiner)")),
        "{:?}",
        app.connection_log()
    );

    app.stop().unwrap();
    let stops = lab.diag_with("event=session_stop");
    assert_eq!(stops.len(), 1, "{:?}", lab.diag_lines());
    assert!(stops[0].contains("reason=stopped"), "{}", stops[0]);
    assert!(
        app.connection_log()
            .iter()
            .any(|line| line.ends_with("session ended (stopped)")),
        "{:?}",
        app.connection_log()
    );
    // Stop reopens an idle session, exactly like the connection-log line.
    assert_eq!(
        lab.diag_with("event=session_start").len(),
        2,
        "{:?}",
        lab.diag_lines()
    );
}
