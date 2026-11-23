//! Known-peer reconnect on Linux: a code pairing is persisted after the first
//! rx > 0, and from then on the pair reconnects with NO code through the
//! 0x03 Hello/Proof track,
//! raising the tunnel with the stored key only after an accepted Proof.
//! Loopback multicast, fake pkexec/wg-quick (they log; the fake pkexec also
//! keeps the config `install` would have copied to /etc/wireguard), the rx
//! counter is a file, time is injected through `tick_at`.
//! Keys are generated per test; assertions never print key material.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{pair_record, Frame, KnownSession, KnownSocket, KnownTrack, Proof};
use jafnify_core::pairing::PairingCode;
use jafnify_core::pake::PakeLink;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, Discovery, DiscoveryConfig, Handshake, RETRY_INTERVAL};
use jafnify_linux::app::{App, AppBuilder, AppError, UiEvent};
use jafnify_linux::settings::{load_state, save_state, LinuxStateFile, Settings};

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
}

impl Lab {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn script(&self, name: &str, body: String) -> std::path::PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// One App: own state dir `<name>-cfg`, fake pkexec logging to `<name>.log`
    /// and keeping the installed config as `<name>.conf`, rx counter file
    /// `<name>.rx` (preset to 1: the peer's data has already arrived).
    fn app(&self, name: &str, role: Role) -> App {
        self.set_rx(name, 1);
        self.set_tx(name, 0);
        let d = self.dir.path();
        let pkexec = self.script(
            &format!("{name}-pkexec.sh"),
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{log}\"\n\
                 case \"$1\" in *install.sh) cp \"$4\" \"{conf}\";; esac\n\
                 case \"$3\" in *\"-m 600\"*) cp \"$5\" \"{conf}\"; echo \"wg-quick.sh up jafn0\" >> \"{log}\";; \
                   *install*) cp \"$5\" \"{conf}\"; echo \"$(basename \"$6\") up $8\" >> \"{log}\";; esac\n\
                 exit 0\n",
                log = d.join(format!("{name}.log")).display(),
                conf = d.join(format!("{name}.conf")).display(),
            ),
        );
        let wg = self.script("wg.sh", "#!/bin/sh\nexit 1\n".into());
        let wgq = self.script("wg-quick.sh", "#!/bin/sh\nexit 0\n".into());
        let install = self.script("install.sh", "#!/bin/sh\nexit 0\n".into());
        let mut app = AppBuilder::new(d.join(format!("{name}-cfg")))
            .wg_bin(wg)
            .pkexec_bin(pkexec)
            .wg_quick_bin(wgq)
            .install_bin(install)
            .iface_pin(Ipv4Addr::LOCALHOST)
            .auto_accept_delay(Duration::from_millis(50))
            .rx_bytes_path(d.join(format!("{name}.rx")))
            .tx_bytes_path(d.join(format!("{name}.tx")))
            .polkit_rules_path(d.join("rules"))
            .installed_conf_path(d.join("jafn0.conf"))
            .build();
        app.set_role(role);
        app
    }

    fn set_rx(&self, name: &str, n: u64) {
        std::fs::write(self.dir.path().join(format!("{name}.rx")), format!("{n}\n")).unwrap();
    }

    fn set_tx(&self, name: &str, n: u64) {
        std::fs::write(self.dir.path().join(format!("{name}.tx")), format!("{n}\n")).unwrap();
    }

    fn count(&self, name: &str, needle: &str) -> usize {
        std::fs::read_to_string(self.dir.path().join(format!("{name}.log")))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(needle))
            .count()
    }

    fn ups(&self, name: &str) -> usize {
        self.count(name, "wg-quick.sh up jafn0")
    }

    fn downs(&self, name: &str) -> usize {
        self.count(name, "ip link del dev jafn0")
    }

    /// The wg-quick config the App asked root to install (holds a private key).
    fn installed_conf(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(format!("{name}.conf"))).unwrap_or_default()
    }

    /// Forget the previous run's log and installed config, and the tunnel
    /// marker: this simulates a process restart with no tunnel up.
    fn fresh_run(&self, name: &str) {
        for ext in ["log", "conf"] {
            let _ = std::fs::remove_file(self.dir.path().join(format!("{name}.{ext}")));
        }
        let _ = std::fs::remove_file(self.dir.path().join(format!("{name}-cfg/tunnel.toml")));
    }

    fn state_path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(format!("{name}-cfg/state.toml"))
    }

    fn state(&self, name: &str) -> LinuxStateFile {
        load_state(&self.state_path(name)).unwrap()
    }

    /// Pre-fill a pair as an earlier code pairing would have stored it.
    fn store_pair(&self, name: &str, own: &KeyPair, peer_pk: &str, own_role: Role) {
        let rec = pair_record(own, peer_pk, own_role, None, 1).unwrap();
        save_state(
            &self.state_path(name),
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
    }
}

/// Fake clock advanced in steps; every advance sleeps a little real time so
/// loopback multicast gets delivered.
struct Clock(Instant);

impl Clock {
    fn advance(&mut self, step: Duration) -> Instant {
        self.0 += step;
        std::thread::sleep(Duration::from_millis(3));
        self.0
    }
}

/// Tick every app (real clock) until `done`.
fn run_until(apps: &mut [&mut App], what: &str, done: impl Fn(&[&mut App]) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !done(&*apps) {
        assert!(
            Instant::now() < end,
            "timeout: {what}: states {:?}",
            apps.iter().map(|a| a.state()).collect::<Vec<_>>()
        );
        for a in apps.iter_mut() {
            a.tick().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Pair two Apps with the code 1234 through real discovery + handshake.
fn pair_with_code(a: &mut App, b: &mut App) {
    a.start("1234").unwrap();
    b.start("1234").unwrap();
    run_until(&mut [a, b], "code pairing", |x| {
        x[0].state() == ConnectionState::Connected && x[1].state() == ConnectionState::Connected
    });
}

/// A "device" that knows the code and announces a fresh key; the App under
/// test brings its tunnel up toward that key.
fn pair_with_ghost(lab: &Lab, name: &str, app: &mut App) -> KeyPair {
    let code = PairingCode::parse("1234").unwrap();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    let ghost = KeyPair::generate();
    let ghost_role = match app.role() {
        Role::Joiner => Role::Sharer,
        Role::Sharer => Role::Joiner,
    };
    let own_port = match ghost_role {
        Role::Sharer => jafnify_core::LISTEN_PORT,
        Role::Joiner => 0,
    };
    let mut pake = PakeLink::open(&cfg(), ghost_role, &code, &ghost, own_port).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while lab.ups(name) == 0 {
        assert!(Instant::now() < end, "tunnel never raised");
        pake.poll(Instant::now()).unwrap();
        app.tick().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    ghost
}

/// `a` lost its peer (rx flat). Drive its fake clock until it is Searching
/// again, returning the events on the way.
fn drive_to_searching(a: &mut App, clock: &mut Clock) -> Vec<UiEvent> {
    let mut events = Vec::new();
    for _ in 0..120 {
        if let Some(ev) = a.tick_at(clock.advance(Duration::from_secs(1))).unwrap() {
            events.push(ev);
        }
        if a.state() == ConnectionState::Searching {
            return events;
        }
    }
    panic!("never reached Searching, state {:?}", a.state());
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ---- persistence -----------------------------------------------------------

#[test]
fn code_pairing_is_persisted_only_after_the_first_rx() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app("a", Role::Joiner);
    lab.set_rx("a", 0);
    let ghost = pair_with_ghost(&lab, "a", &mut app);

    // Tunnel configured, no traffic yet: nothing is remembered.
    for _ in 0..20 {
        app.tick().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert!(lab.state("a").peers.is_empty(), "persisted before any rx");

    lab.set_rx("a", 5);
    run_until(&mut [&mut app], "connected", |x| {
        x[0].state() == ConnectionState::Connected
    });

    let peers = lab.state("a").peers;
    assert_eq!(peers.len(), 1);
    let p = &peers[0];
    assert_eq!(p.public_key, ghost.public_key_b64());
    assert_eq!(p.role, Role::Sharer, "role is the peer's");
    assert_eq!(p.last_endpoint.as_deref(), Some("127.0.0.1:9797"));
    // The stored private key is the one the tunnel used, and the id matches it.
    let own = KeyPair::from_private_b64(p.private_key.as_deref().expect("no private key")).unwrap();
    assert!(lab
        .installed_conf("a")
        .contains(p.private_key.as_deref().unwrap()));
    assert!(
        lab.installed_conf("a").contains("\nDNS = 10.233.0.1\n"),
        "code-track joiner conf has no DNS line"
    );
    let rec = pair_record(&own, &ghost.public_key_b64(), Role::Joiner, None, 0).unwrap();
    assert_eq!(p.id, rec.id);

    // The state file holds a private key.
    assert_eq!(mode(&lab.state_path("a")), 0o600);
    assert_eq!(mode(lab.state_path("a").parent().unwrap()), 0o700);
    assert_eq!(app.peers().len(), 1);
}

#[test]
fn a_connect_that_never_gets_data_persists_nothing() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app("a", Role::Joiner);
    lab.set_rx("a", 0);
    pair_with_ghost(&lab, "a", &mut app);

    let mut clock = Clock(Instant::now());
    let mut lost = false;
    for _ in 0..60 {
        if let Some(UiEvent::Disconnected { .. }) =
            app.tick_at(clock.advance(Duration::from_secs(1))).unwrap()
        {
            lost = true;
            break;
        }
    }
    assert!(lost, "Connecting never gave up");
    assert!(lab.state("a").peers.is_empty());
}

#[test]
fn session_stats_track_up_down_rates_endpoint_from_the_app() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app("a", Role::Joiner);
    lab.set_rx("a", 0);
    lab.set_tx("a", 0);
    pair_with_ghost(&lab, "a", &mut app);

    let mut clock = Clock(Instant::now());
    // The Lab raised the tunnel and recorded the peer endpoint; confirm the
    // state without inventing traffic so the session starts from 0/0.
    app.orchestrator_mut().confirm_connected();
    app.tick_at(clock.advance(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);

    // The tracker's first sample is only the counter baseline, so establish a
    // 0/0 sample before the measured traffic.
    lab.set_rx("a", 0);
    app.tick_at(clock.advance(Duration::from_millis(100)))
        .unwrap();

    lab.set_rx("a", 100);
    lab.set_tx("a", 50);
    app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
    lab.set_rx("a", 200);
    lab.set_tx("a", 80);
    app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
    lab.set_rx("a", 300);
    lab.set_tx("a", 120);
    app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();

    let stats = app.current_session();
    assert_eq!(stats.up_bytes, 120, "{stats:?}");
    assert_eq!(stats.down_bytes, 300, "{stats:?}");
    assert_eq!(stats.peer_endpoint, "127.0.0.1:9797");
    assert!(stats.connected_ms > 0, "{stats:?}");
    assert!(stats.avg_up_bps > 0, "{stats:?}");
    assert!(stats.avg_down_bps > 0, "{stats:?}");
    assert!(stats.peak_up_bps >= stats.avg_up_bps, "{stats:?}");
    assert!(stats.peak_down_bps >= stats.avg_down_bps, "{stats:?}");
}

#[test]
fn a_new_pairing_replaces_the_stored_peer() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let old = KeyPair::generate();
    lab.store_pair(
        "a",
        &KeyPair::generate(),
        &old.public_key_b64(),
        Role::Joiner,
    );
    let mut app = lab.app("a", Role::Joiner);
    let ghost = pair_with_ghost(&lab, "a", &mut app);
    run_until(&mut [&mut app], "connected", |x| {
        x[0].state() == ConnectionState::Connected
    });

    let peers = lab.state("a").peers;
    assert_eq!(peers.len(), 1, "v1 keeps one known peer");
    assert_eq!(peers[0].public_key, ghost.public_key_b64());
}

#[test]
fn pair_code_with_a_stored_peer_still_runs_the_code_track() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.store_pair(
        "a",
        &KeyPair::generate(),
        &KeyPair::generate().public_key_b64(),
        Role::Joiner,
    );
    let mut app = lab.app("a", Role::Joiner);
    app.start("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    assert!(app.orchestrator_mut().own_token().is_some());
}

// ---- known track -----------------------------------------------------------

#[test]
fn known_start_needs_a_paired_device() {
    let lab = Lab::new();
    let mut app = lab.app("a", Role::Joiner);
    let err = app.start_known().unwrap_err();
    assert!(matches!(err, AppError::NoPairedDevice));
    assert_eq!(err.to_string(), "no paired device - use jafnify pair");
    assert_eq!(app.state(), ConnectionState::Idle);

    // A record without a private key is not a pairing.
    let legacy = jafnify_core::state::Peer {
        id: "old".into(),
        name: "old".into(),
        public_key: "k".into(),
        role: Role::Sharer,
        last_connected: None,
        private_key: None,
        last_endpoint: None,
    };
    save_state(
        &lab.state_path("b"),
        &LinuxStateFile {
            peers: vec![legacy],
            settings: Settings::default(),
        },
    )
    .unwrap();
    let mut app = lab.app("b", Role::Joiner);
    assert!(matches!(app.start_known(), Err(AppError::NoPairedDevice)));
}

#[test]
fn two_paired_apps_reconnect_without_a_code_using_the_persisted_keys() {
    let _g = multicast_lock();
    let lab = Lab::new();
    {
        let mut a = lab.app("a", Role::Joiner);
        let mut b = lab.app("b", Role::Sharer);
        pair_with_code(&mut a, &mut b);
    } // both dropped: the process "restarted"
    let (sa, sb) = (lab.state("a"), lab.state("b"));
    let (pa, pb) = (&sa.peers[0], &sb.peers[0]);
    assert_eq!(sa.peers.len(), 1);
    assert_eq!(sb.peers.len(), 1);
    assert_eq!(pa.id, pb.id, "pairId differs between the sides");
    let a_priv = pa.private_key.clone().unwrap();
    let b_priv = pb.private_key.clone().unwrap();
    assert_eq!(
        pa.public_key,
        KeyPair::from_private_b64(&b_priv).unwrap().public_key_b64()
    );
    assert_eq!(
        pb.public_key,
        KeyPair::from_private_b64(&a_priv).unwrap().public_key_b64()
    );

    lab.fresh_run("a");
    lab.fresh_run("b");
    // No 0x01 / 0x02 may appear any more.
    let disc = Discovery::bind(&cfg()).unwrap();
    let hs = Handshake::bind(&cfg()).unwrap();
    let mut a = lab.app("a", Role::Joiner);
    let mut b = lab.app("b", Role::Sharer);
    a.start_known().unwrap();
    b.start_known().unwrap();
    run_until(&mut [&mut a, &mut b], "known reconnect", |x| {
        x[0].state() == ConnectionState::Connected && x[1].state() == ConnectionState::Connected
    });

    assert_eq!((lab.ups("a"), lab.ups("b")), (1, 1));
    let ca = lab.installed_conf("a");
    assert!(ca.contains(&a_priv), "joiner did not use the persisted key");
    assert!(
        ca.contains(&pa.public_key),
        "joiner did not use the stored peer key"
    );
    assert!(
        ca.contains("Endpoint = 127.0.0.1:9797"),
        "endpoint is the Proof source"
    );
    assert!(
        ca.contains("\nDNS = 10.233.0.1\n"),
        "known-track joiner conf has no DNS line"
    );
    let cb = lab.installed_conf("b");
    assert!(!cb.contains("DNS"), "the sharer conf must not set DNS");
    assert!(cb.contains(&b_priv), "sharer did not use the persisted key");
    assert!(
        !cb.contains("Endpoint"),
        "sharer learns its endpoint from WireGuard"
    );
    assert!(
        disc.try_receive(Duration::from_millis(50))
            .unwrap()
            .is_none(),
        "0x01 on the known track"
    );
    assert!(
        hs.try_receive(Duration::from_millis(50)).unwrap().is_none(),
        "0x02 on the known track"
    );
    // Reconnecting keeps the pair: only its last_connected stamp moves on
    // (in whole seconds, so it may or may not change within this test).
    let (ra, rb) = (lab.state("a"), lab.state("b"));
    assert_eq!(ra.peers.len(), 1);
    assert_eq!(rb.peers.len(), 1);
    assert_eq!(ra.peers[0].id, pa.id);
    assert_eq!(ra.peers[0].public_key, pa.public_key);
    assert_eq!(ra.peers[0].private_key, pa.private_key);
    assert_eq!(rb.peers[0].private_key, pb.private_key);
    assert!(ra.peers[0].last_connected >= pa.last_connected);
    assert!(rb.peers[0].last_connected >= pb.last_connected);
}

#[test]
fn a_known_sharer_brings_the_tunnel_up_at_start() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    lab.store_pair("b", &own, &peer.public_key_b64(), Role::Sharer);
    let mut b = lab.app("b", Role::Sharer);
    lab.set_rx("b", 0);
    b.start_known().unwrap();
    for _ in 0..5 {
        b.tick().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(lab.ups("b"), 1, "gateway not raised at start");
    let conf = lab.installed_conf("b");
    assert!(conf.contains(&own.private_key_b64()));
    assert!(conf.contains(&peer.public_key_b64()));
    assert!(!conf.contains("Endpoint"));
    assert_eq!(
        b.state(),
        ConnectionState::Searching,
        "no data yet, no proof yet"
    );
    let log = b.connection_log();
    assert!(
        log.first()
            .is_some_and(|line| line.ends_with("session started (sharer)")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged up: ok (password prompt)")),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|line| line.contains("tunnel inspect")),
        "inspect must not use the privileged template: {log:?}"
    );
}

#[test]
fn a_stranger_gets_no_proof_and_cannot_raise_the_tunnel() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    lab.store_pair("a", &own, &peer.public_key_b64(), Role::Joiner);
    let mut a = lab.app("a", Role::Joiner);
    a.start_known().unwrap();
    let listener = KnownSocket::bind(&cfg()).unwrap();
    let stranger = KeyPair::generate();
    // Knows the joiner's public key, not the pair secret.
    let hello = KnownSession::new(&stranger, &own.public_key_b64(), Role::Sharer)
        .unwrap()
        .hello();
    let stranger_nonce = hello.nonce;
    listener.send(&hello.encode()).unwrap();

    let mut saw_joiner_nonce = None;
    let end = Instant::now() + Duration::from_millis(2500);
    let mut proofs = 0;
    while Instant::now() < end {
        assert_eq!(a.tick().unwrap(), None, "a stranger moved the app");
        while let Some((_, f)) = listener.receive(Duration::from_millis(10)).unwrap() {
            match f {
                // Only this exchange's Joiner Proof counts. A live Jafnify
                // process on the host also sends Joiner Proofs on the group.
                Frame::Proof(p) if p.role == Role::Joiner && p.n_echo == stranger_nonce => {
                    proofs += 1
                }
                Frame::Hello(h) if h.role == Role::Joiner => saw_joiner_nonce = Some(h.nonce),
                _ => {}
            }
        }
        if let Some(n) = saw_joiner_nonce.take() {
            // A forged Proof echoing the real nonce.
            let forged = Proof {
                role: Role::Sharer,
                n_self: [3u8; 16],
                n_echo: n,
                port: 9797,
                peer_addr: [127, 0, 0, 1],
                mac: [7u8; 32],
            };
            listener.send(&forged.encode()).unwrap();
        }
    }
    assert_eq!(proofs, 0, "answered a Hello without the pair secret");
    assert_eq!(lab.ups("a"), 0, "tunnel raised for a stranger");
    assert_eq!(a.state(), ConnectionState::Searching);
}

#[test]
fn a_stalled_known_peer_keeps_the_tunnel_and_retries_on_the_known_track() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let (own, sharer_key) = (KeyPair::generate(), KeyPair::generate());
    lab.store_pair("a", &own, &sharer_key.public_key_b64(), Role::Joiner);
    let mut a = lab.app("a", Role::Joiner);
    a.start_known().unwrap();
    let mut ghost = KnownTrack::open(
        &cfg(),
        &sharer_key,
        &own.public_key_b64(),
        Role::Sharer,
        Instant::now(),
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while a.state() != ConnectionState::Connected {
        assert!(Instant::now() < end, "no reconnect, state {:?}", a.state());
        ghost.poll(Instant::now()).unwrap();
        a.tick().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(lab.ups("a"), 1);
    assert_eq!(lab.downs("a"), 0);

    // The sharer vanishes: rx stays flat.
    lab.set_rx("a", 100);
    let mut clock = Clock(Instant::now());
    let mut events = Vec::new();
    for _ in 0..120 {
        let ev = a.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
        if matches!(ev, Some(UiEvent::Disconnected { .. })) {
            // Peer loss keeps jafn0 configured; only an explicit stop tears down.
            assert_eq!(lab.downs("a"), 0, "peer loss tore the tunnel down");
            assert_eq!(a.state(), ConnectionState::Retry);
            events.push(ev);
            break;
        }
    }
    assert_eq!(events.len(), 1, "stall never detected");
    let log = a.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("state Connected -> Retry")),
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("disconnected at "))
            .count(),
        1,
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|line| line.contains("(detected after ") && line.contains(" s)")),
        "{log:?}"
    );
    clock.advance(RETRY_INTERVAL);
    a.tick_at(clock.advance(Duration::from_millis(1))).unwrap();
    assert_eq!(a.state(), ConnectionState::Searching);
    assert!(
        a.orchestrator_mut().own_token().is_none(),
        "retry went back to the code track"
    );

    // The sharer comes back: Hello/Proof again, but the old rx counter is
    // already 100. It must not report Connected until new data arrives.
    let end = Instant::now() + Duration::from_secs(10);
    while a.state() != ConnectionState::Connecting {
        assert!(
            Instant::now() < end,
            "no reconnect after retry, state {:?}",
            a.state()
        );
        ghost.poll(Instant::now()).unwrap();
        a.tick_at(clock.advance(Duration::from_millis(100)))
            .unwrap();
    }
    assert_eq!(
        a.state(),
        ConnectionState::Connecting,
        "a non-zero old rx counter must not fake Connected"
    );
    assert_eq!(
        lab.count("a", "inspect"),
        0,
        "same-endpoint reuse asked for root"
    );

    lab.set_rx("a", 200);
    a.tick_at(clock.advance(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(a.state(), ConnectionState::Connected);
    assert_eq!(
        lab.ups("a"),
        1,
        "reconnect raised a new tunnel instead of reusing it"
    );
    let log = a.connection_log();
    let reconnects: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("reconnected at "))
        .collect();
    assert_eq!(reconnects.len(), 1, "{log:?}");
    assert!(
        !reconnects[0].contains("(peer") && !reconnects[0].contains("127.0.0.1"),
        "the reconnect line carries no endpoint: {log:?}"
    );
}

#[test]
fn after_a_code_pairing_is_persisted_retries_use_the_known_track() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut a = lab.app("a", Role::Joiner);
    let mut b = lab.app("b", Role::Sharer);
    pair_with_code(&mut a, &mut b);
    assert_eq!(lab.state("a").peers.len(), 1);
    lab.set_rx("a", 100); // flat from here on
    drop(b);

    let mut clock = Clock(Instant::now());
    drive_to_searching(&mut a, &mut clock);
    assert!(
        a.orchestrator_mut().own_token().is_none(),
        "retry repeated the code search"
    );
    assert_eq!(a.state(), ConnectionState::Searching);
}

// A pairing that could not be stored must not strand the App on a known
// track that has no key: it keeps retrying on the code track.
#[test]
fn a_pairing_that_cannot_be_stored_keeps_retrying_on_the_code_track() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut a = lab.app("a", Role::Joiner);
    // state.toml is a directory: the atomic rename onto it fails.
    std::fs::create_dir_all(lab.state_path("a")).unwrap();
    pair_with_ghost(&lab, "a", &mut a);
    run_until(&mut [&mut a], "connected", |x| {
        x[0].state() == ConnectionState::Connected
    });
    assert!(
        a.last_error()
            .is_some_and(|e| e.contains("could not save the pairing")),
        "{:?}",
        a.last_error()
    );
    assert!(!a.is_paired());

    lab.set_rx("a", 100);
    let mut clock = Clock(Instant::now());
    drive_to_searching(&mut a, &mut clock);
    assert!(
        a.orchestrator_mut().own_token().is_some(),
        "no code track to retry on"
    );
}

// ---- forget ----------------------------------------------------------------

#[test]
fn forget_deletes_the_record_and_the_key_and_stops() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let (own, peer) = (KeyPair::generate(), KeyPair::generate());
    lab.store_pair("a", &own, &peer.public_key_b64(), Role::Joiner);
    let mut a = lab.app("a", Role::Joiner);
    a.start_known().unwrap();
    assert_eq!(a.state(), ConnectionState::Searching);
    let marker = lab.dir.path().join("a-cfg/tunnel.toml");
    std::fs::write(
        &marker,
        format!("peer_public_key = \"{}\"\n", peer.public_key_b64()),
    )
    .unwrap();

    a.forget().unwrap();
    assert_eq!(a.state(), ConnectionState::Idle);
    assert!(a.peers().is_empty());
    assert!(!marker.exists(), "forget left a tunnel marker behind");
    assert_eq!(lab.downs("a"), 1, "active tunnel not taken down");
    let text = std::fs::read_to_string(lab.state_path("a")).unwrap();
    assert!(
        !text.contains(&own.private_key_b64()),
        "private key survived forget"
    );
    assert!(lab.state("a").peers.is_empty());
    assert!(matches!(a.start_known(), Err(AppError::NoPairedDevice)));
    // Pairing is possible again.
    a.start("1234").unwrap();
}

#[test]
fn forget_on_an_idle_paired_app_still_cleans_up_the_rule() {
    let lab = Lab::new();
    lab.store_pair(
        "a",
        &KeyPair::generate(),
        &KeyPair::generate().public_key_b64(),
        Role::Joiner,
    );
    let mut a = lab.app("a", Role::Joiner);
    // Idle means no live tunnel, even though the helper writes counters for
    // the connected tests.
    std::fs::remove_file(lab.dir.path().join("a.rx")).unwrap();
    let cleanup_calls = lab.count("a", "/bin/sh");
    a.forget().unwrap();
    assert!(a.peers().is_empty());
    assert!(lab.state("a").peers.is_empty());
    assert_eq!(
        lab.count("a", "/bin/sh"),
        cleanup_calls + 1,
        "a stored pair is reason enough to remove an unreadable leftover rule"
    );
    assert_eq!(lab.ups("a"), 0, "cleanup must not bring a tunnel up");
}
