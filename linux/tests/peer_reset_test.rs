//! Linux peer-session reset.
//! While Connected, a flat rx counter sends one known-peer probe; a valid
//! Proof carrying a peer nonce outside the current/previous set starts a 12 s
//! data grace window. Data inside it clears the hint; no data drops on the
//! reset-specific 2 s retry. Normal 30 s rotation is not a reset. Everything
//! is fake: the rx counter is a file, wg-quick/pkexec are scripts, and time is
//! injected through `tick_at`; only loopback multicast delivery is real.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::{pair_record, KnownSocket, PairSecret, Proof, NONCE_ROTATE_INTERVAL};
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, DiscoveryConfig, KeyPair};
use jafnify_linux::app::{App, AppBuilder, UiEvent};
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};
use jafnify_linux::view::ViewInput;

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
    own: KeyPair,
    peer: KeyPair,
}

impl Lab {
    /// A stored pair (Joiner app, Sharer peer) with a fake tunnel counter and
    /// fake root commands that never touch the host.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = |name: &str, body: String| {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        script(
            "mock-pkexec.sh",
            format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );
        script("mock-wg.sh", "#!/bin/sh\nexit 1\n".into());
        script("mock-wg-quick.sh", "#!/bin/sh\nexit 0\n".into());
        script("mock-install.sh", "#!/bin/sh\nexit 0\n".into());

        let own = KeyPair::generate();
        let peer = KeyPair::generate();
        let rec = pair_record(&own, &peer.public_key_b64(), Role::Joiner, None, 1).unwrap();
        save_state(
            &dir.path().join("cfg/state.toml"),
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
        Self { dir, own, peer }
    }

    fn rx_path(&self) -> PathBuf {
        self.dir.path().join("rx_bytes")
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(self.rx_path(), format!("{n}\n")).unwrap();
    }

    fn app(&self) -> App {
        self.set_rx(1);
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("mock-wg.sh"))
            .pkexec_bin(p("mock-pkexec.sh"))
            .wg_quick_bin(p("mock-wg-quick.sh"))
            .install_bin(p("mock-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.rx_path())
            .tx_bytes_path(p("tx_bytes"))
            .polkit_rules_path(p("rules"))
            .installed_conf_path(p("jafn0.conf"))
            .build()
    }

    /// The peer's view of the pair secret.
    fn secret(&self) -> PairSecret {
        PairSecret::derive(&self.peer, &self.own.public_key_b64()).unwrap()
    }

    /// A Sharer Proof the App must accept: it echoes the App's own nonce.
    fn proof(&self, n_self: [u8; 16], n_echo: [u8; 16]) -> Proof {
        self.secret().proof_on(
            Role::Sharer,
            n_self,
            n_echo,
            &self.peer.public_key_bytes(),
            &self.own.public_key_bytes(),
            9797,
        )
    }

    /// A Connected App on the known track (the tunnel itself is not needed).
    fn connected_app(&self) -> App {
        let mut app = self.app();
        app.start_known().unwrap();
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);
        app
    }
}

fn deliver(listener: &KnownSocket, proof: &Proof) {
    listener.send(&proof.encode()).unwrap();
    std::thread::sleep(Duration::from_millis(40));
}

/// Tick once so the App's track sends a Hello, returning that Hello's nonce.
fn capture_own_nonce(app: &mut App, listener: &KnownSocket, now: Instant) -> [u8; 16] {
    app.tick_at(now).unwrap();
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        if let Some((_, jafnify_core::known::Frame::Hello(h))) =
            listener.receive(Duration::from_millis(20)).unwrap()
        {
            if h.role == Role::Joiner {
                return h.nonce;
            }
        }
    }
    panic!("the App never sent a Hello");
}

/// Teach the App a prior peer nonce, then tick and return its own Hello nonce.
/// A reset needs an existing nonce to be outside of; the first valid Proof on
/// a fresh track is deliberately not a reset.
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

/// Settle the track on one normal peer rotation at `at` (callers pass at least
/// 25 s after the prior nonce). A fresh track opened mid-rotation can see the
/// peer's first replacement early; before that normal rotation the replacement
/// is ambiguous and intentionally treated as normal, with the ordinary
/// 35 s/10 s path still in force. Reset evidence is only meaningful after this.
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

fn reason_of(ev: &UiEvent) -> &str {
    match ev {
        UiEvent::Disconnected { reason } => reason,
        other => panic!("expected Disconnected, got {other:?}"),
    }
}

#[test]
fn a_peer_session_reset_without_data_drops_at_the_neutral_deadline_and_retries_at_two() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, [41u8; 16], t0);

    // Before the first observed normal rotation an early replacement is
    // ambiguous, so settle on one >= 25 s rotation before injecting a restart.
    let settle_at = t0 + NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
    settle_with_normal_rotation(&mut app, &listener, &lab, echo, settle_at);
    // Fresh data just before the reset keeps the ordinary 35 s stall out of
    // this test: the neutral window must be what closes the outage.
    lab.set_rx(200);
    app.tick_at(settle_at + Duration::from_millis(20)).unwrap();

    // A valid Proof from a nonce this session never saw: reset evidence, but
    // not a drop yet.
    let unknown = [42u8; 16];
    deliver(&listener, &lab.proof(unknown, echo));
    let hinted_at = settle_at + Duration::from_secs(1);
    assert_eq!(app.tick_at(hinted_at).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(app.peer_reset_pending(), "no reset hint from a new nonce");

    // Eleven seconds later the connection is still up: a real install gap
    // of an app update (ten seconds on a phone) must fit inside the window.
    assert_eq!(
        app.tick_at(hinted_at + Duration::from_secs(11)).unwrap(),
        None
    );
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(app.peer_reset_pending());

    // At 12 s the neutral window closes with no data.
    let ev = app.tick_at(hinted_at + Duration::from_secs(12)).unwrap();
    assert_eq!(
        reason_of(ev.as_ref().expect("no drop at the neutral deadline")),
        "peer session reset; no data for 12 s"
    );
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(!app.peer_reset_pending());
    assert_eq!(
        app.status_text_at(hinted_at + Duration::from_secs(12)),
        "Connection lost - retrying in 2 s - peer session reset; no data for 12 s"
    );
    assert_eq!(
        app.retry_secs_left_at(hinted_at + Duration::from_secs(12)),
        Some(2),
        "the retry countdown must use the 2 s reset interval"
    );

    // The retry override is 2 s, not the ordinary 10 s.
    app.tick_at(hinted_at + Duration::from_secs(13)).unwrap();
    assert_eq!(app.state(), ConnectionState::Retry);
    assert_eq!(
        app.retry_secs_left_at(hinted_at + Duration::from_secs(13)),
        Some(1)
    );
    app.tick_at(hinted_at + Duration::from_secs(14)).unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
}

#[test]
fn view_input_carries_the_short_reset_retry() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app();
    let now = Instant::now();
    app.orchestrator_mut().on_disconnect_at_with_retry(
        "peer session reset; no data for 12 s".to_string(),
        now,
        Duration::from_secs(2),
    );

    assert_eq!(app.retry_secs_left_at(now), Some(2));
    let input = ViewInput::from_app(&app);
    assert!(
        input.retry_secs_left.is_some_and(|secs| secs <= 2),
        "view countdown is {:?}, not the 2 s reset retry",
        input.retry_secs_left
    );
    assert_eq!(
        input.retry_reason.as_deref(),
        Some("peer session reset; no data for 12 s")
    );
}

#[test]
fn data_inside_the_grace_window_clears_the_reset_hint() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, [43u8; 16], t0);
    let settle_at = t0 + NONCE_ROTATE_INTERVAL - Duration::from_secs(5);
    settle_with_normal_rotation(&mut app, &listener, &lab, echo, settle_at);

    deliver(&listener, &lab.proof([44u8; 16], echo));
    let hinted_at = settle_at + Duration::from_secs(1);
    app.tick_at(hinted_at).unwrap();
    assert!(app.peer_reset_pending());

    // Data at 3 s is inside the neutral window: keep the connection.
    lab.set_rx(200);
    app.tick_at(hinted_at + Duration::from_secs(3)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.peer_reset_pending(), "data did not clear the hint");

    // Past the whole window there is still no drop: data cleared it.
    assert_eq!(
        app.tick_at(hinted_at + Duration::from_secs(16)).unwrap(),
        None
    );
    assert_eq!(app.state(), ConnectionState::Connected);
}

#[test]
fn normal_nonce_rotation_while_idle_sets_no_hint_and_does_not_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, [45u8; 16], t0);

    let mut now = t0;
    // Idle, but still well inside the ordinary 35 s stall window.
    for _ in 0..29 {
        now += Duration::from_secs(1);
        assert_eq!(app.tick_at(now).unwrap(), None);
    }

    // The peer rotates at 30 s; its Hello and Proof use the new nonce.
    let rotated = [44u8; 16];
    listener
        .send(&lab.secret().hello(Role::Sharer, rotated).encode())
        .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    now += Duration::from_secs(1);
    assert_eq!(app.tick_at(now).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(
        !app.peer_reset_pending(),
        "rotation was mistaken for a reset"
    );

    deliver(&listener, &lab.proof(rotated, echo));
    now += Duration::from_secs(1);
    assert_eq!(app.tick_at(now).unwrap(), None);
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(
        !app.peer_reset_pending(),
        "a Proof for the rotated nonce was mistaken for a reset"
    );
}

#[test]
fn two_seconds_of_silence_do_not_drop() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();

    app.tick_at(t0 + Duration::from_secs(2)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.peer_reset_pending());
}

#[test]
fn an_unknown_nonce_without_a_valid_proof_sets_no_hint() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce(&mut app, &listener, t0);

    // A valid Hello announcing a new nonce is a normal rotation, not reset
    // evidence: the reset needs a valid Proof that echoes our nonce.
    listener
        .send(&lab.secret().hello(Role::Sharer, [45u8; 16]).encode())
        .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.peer_reset_pending());

    // A forged Proof (bad MAC) with another unknown nonce is no evidence.
    let mut forged = lab.proof([46u8; 16], echo);
    forged.mac = [5u8; 32];
    listener.send(&forged.encode()).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    app.tick_at(t0 + Duration::from_secs(2)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.peer_reset_pending(), "a forged Proof set the hint");
}

#[test]
fn a_thirty_second_rotation_with_healthy_data_does_not_shorten_the_session() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let mut app = lab.connected_app();
    lab.set_rx(100);
    let t0 = Instant::now();
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, [49u8; 16], t0);

    let mut now = t0;
    let mut rx = 100;
    for second in 1..=45 {
        rx += 10;
        lab.set_rx(rx);
        now += Duration::from_secs(1);
        assert_eq!(app.tick_at(now).unwrap(), None);
        if second == 30 {
            // The peer's normal 30 s rotation, with data still flowing.
            let rotated = [47u8; 16];
            listener
                .send(&lab.secret().hello(Role::Sharer, rotated).encode())
                .unwrap();
            std::thread::sleep(Duration::from_millis(40));
            app.tick_at(now).unwrap();
            deliver(&listener, &lab.proof(rotated, echo));
            app.tick_at(now).unwrap();
        }
        assert_eq!(app.state(), ConnectionState::Connected);
        assert!(!app.peer_reset_pending());
    }
    // The ordinary 35 s stall window never fired either.
    assert_eq!(app.state(), ConnectionState::Connected);
}

fn signal(
    lab: &Lab,
    kind: jafnify_core::known::SignalKind,
    n_self: [u8; 16],
    echo: [u8; 16],
) -> Vec<u8> {
    lab.secret()
        .signal(
            kind,
            Role::Sharer,
            n_self,
            echo,
            &lab.peer.public_key_bytes(),
            &lab.own.public_key_bytes(),
        )
        .encode()
        .to_vec()
}

fn send(listener: &KnownSocket, frame: &[u8]) {
    listener.send(frame).unwrap();
    std::thread::sleep(Duration::from_millis(40));
}

/// The peer (phone) stops and starts sharing again within seconds; data
/// never stalls long enough for the 35 s path. Its Fresh must restart this
/// side's connected clock and count one drop.
#[test]
fn a_restarted_peer_restarts_the_connected_clock_and_counts_a_drop() {
    use jafnify_core::known::SignalKind;
    let _g = multicast_lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let lab = Lab::new();
    let mut app = lab.connected_app();
    let t0 = Instant::now();
    let first = [21u8; 16];
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, first, t0);
    // The first session's own Fresh: not a restart.
    send(&listener, &signal(&lab, SignalKind::Fresh, first, echo));
    app.tick_at(t0 + Duration::from_millis(10)).unwrap();
    let mut rx = 1;
    for s in 1..=20 {
        rx += 1000;
        lab.set_rx(rx);
        app.tick_at(t0 + Duration::from_secs(s)).unwrap();
    }
    assert_eq!(app.current_session().drops, 0);
    assert!(app.connected_secs_at(t0 + Duration::from_secs(20)).unwrap() >= 19);

    // The peer restarts: a new nonce, its Hello and its Fresh.
    let second = [22u8; 16];
    send(
        &listener,
        &lab.secret().hello(Role::Sharer, second).encode(),
    );
    send(&listener, &signal(&lab, SignalKind::Fresh, second, echo));
    let at = t0 + Duration::from_secs(21);
    lab.set_rx(rx + 1000);
    app.tick_at(at).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected, "data still flows");
    assert_eq!(app.connected_secs_at(at), Some(0), "the clock starts again");
    assert_eq!(app.current_session().drops, 1);

    // A repeated copy of the same Fresh does not count again.
    send(&listener, &signal(&lab, SignalKind::Fresh, second, echo));
    app.tick_at(at + Duration::from_secs(1)).unwrap();
    assert_eq!(app.current_session().drops, 1);
}

/// The peer stops sharing: its Bye drops this side at once, on the short
/// retry, instead of after 35 s without data.
#[test]
fn a_bye_from_the_peer_drops_the_connection_at_once() {
    use jafnify_core::known::SignalKind;
    let _g = multicast_lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let lab = Lab::new();
    let mut app = lab.connected_app();
    let t0 = Instant::now();
    let peer_nonce = [31u8; 16];
    let echo = capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, peer_nonce, t0);
    send(&listener, &signal(&lab, SignalKind::Bye, peer_nonce, echo));
    let ev = app.tick_at(t0 + Duration::from_millis(10)).unwrap();
    assert_eq!(
        reason_of(ev.as_ref().expect("an event")),
        "peer disconnected"
    );
    assert_eq!(app.state(), ConnectionState::Retry);
}

/// A Bye that does not echo this side's nonce (a replay or a forgery) is
/// ignored.
#[test]
fn a_bye_with_a_stale_echo_is_ignored() {
    use jafnify_core::known::SignalKind;
    let _g = multicast_lock();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let lab = Lab::new();
    let mut app = lab.connected_app();
    let t0 = Instant::now();
    let peer_nonce = [41u8; 16];
    capture_own_nonce_with_prior_peer(&mut app, &listener, &lab, peer_nonce, t0);
    send(
        &listener,
        &signal(&lab, SignalKind::Bye, peer_nonce, [0xEE; 16]),
    );
    app.tick_at(t0 + Duration::from_millis(10)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
}
