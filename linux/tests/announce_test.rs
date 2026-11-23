//! 0x02 handshake robustness (lab restart case). A single 0x02 can
//! be lost; a side that reports Connected on receipt of the peer's 0x02 and
//! then falls silent deadlocks the other side. Rules under test:
//!   1. Connecting re-announces 0x02 every ANNOUNCE_INTERVAL, also after the
//!      tunnel is up, until the first rx from the peer.
//!   2. Connected is reported only after the first rx (> 0).
//!   3. Connecting gives up CONNECT_TIMEOUT after the tunnel came up with no rx.
//!
//! The peer is a "ghost": a raw `Handshake` socket on loopback the test drives
//! frame by frame. Fake pkexec/wg-quick, injected clock, rx counter in a file.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::pairing::PairingCode;
use jafnify_core::pake::PakeLink;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, DiscoveryConfig, KeyPair, ANNOUNCE_INTERVAL, CONNECT_TIMEOUT};
use jafnify_linux::app::{App, AppBuilder, UiEvent};

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
        let log = dir.path().join("calls.log");
        let fakes = [
            (
                "pkexec.sh",
                format!(
                    "#!/bin/sh\necho \"$@\" >> \"{log}\"\n\
                     case \"$3\" in *install*) echo \"$(basename \"$6\") up $8\" >> \"{log}\";; \
                       *\"-m 600\"*) echo \"wg-quick.sh up jafn0\" >> \"{log}\";; esac\n\
                     exit 0\n",
                    log = log.display()
                ),
            ),
            ("wg.sh", "#!/bin/sh\nexit 1\n".to_string()),
            ("wg-quick.sh", "#!/bin/sh\nexit 0\n".to_string()),
            ("install.sh", "#!/bin/sh\nexit 0\n".to_string()),
        ];
        for (name, body) in fakes {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.path().join("rx"), "0\n").unwrap();
        Self { dir }
    }

    /// Make the pairing impossible to store (state.toml is a directory), so a
    /// retry stays on the code track. A pairing that IS stored retries on the
    /// known track instead (see known_peer_test).
    fn unsaveable(&self) {
        std::fs::create_dir_all(self.dir.path().join("cfg/state.toml")).unwrap();
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(self.dir.path().join("rx"), format!("{n}\n")).unwrap();
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn ups(&self) -> usize {
        self.calls().matches("wg-quick.sh up jafn0").count()
    }

    fn app(&self) -> App {
        let p = |n: &str| self.dir.path().join(n);
        AppBuilder::new(self.dir.path().join("cfg"))
            .wg_bin(p("wg.sh"))
            .pkexec_bin(p("pkexec.sh"))
            .wg_quick_bin(p("wg-quick.sh"))
            .install_bin(p("install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(p("rx"))
            .build()
    }
}

/// A peer driven by hand: the sharer side of the code track.
struct Ghost {
    pake: PakeLink,
}

impl Ghost {
    fn new() -> Self {
        let cfg = DiscoveryConfig {
            iface: Some(Ipv4Addr::LOCALHOST),
            ..Default::default()
        };
        let key = KeyPair::generate();
        let code = PairingCode::parse("1234").unwrap();
        let pake =
            PakeLink::open(&cfg, Role::Sharer, &code, &key, jafnify_core::LISTEN_PORT).unwrap();
        Self { pake }
    }

    fn announce(&mut self) {
        self.announce_at(Instant::now());
    }

    fn announce_at(&mut self, now: Instant) {
        self.pake.poll(now).unwrap();
    }

    /// Opposite-role Hellos received since the last call. Polling here also
    /// sends the ghost's own Hello and answers with its Confirm.
    fn frames(&mut self) -> usize {
        let before = self.pake.hellos_seen();
        self.pake.poll(Instant::now()).unwrap();
        (self.pake.hellos_seen() - before) as usize
    }
}

/// Fake clock; every step also sleeps briefly so loopback multicast lands.
struct Clock(Instant);

impl Clock {
    fn advance(&mut self, d: Duration) -> Instant {
        self.0 += d;
        std::thread::sleep(Duration::from_millis(5));
        self.0
    }
}

/// App already past pairing: Connecting, sending the PAKE Hello.
fn connecting_app(lab: &Lab) -> App {
    let mut app = lab.app();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    app
}

/// Tick (step 100 ms) until the tunnel was brought up. Returns the events seen.
fn tick_until_up(app: &mut App, lab: &Lab, clock: &mut Clock, ghost: &mut Ghost) -> Vec<UiEvent> {
    let mut events = Vec::new();
    for _ in 0..100 {
        let now = clock.advance(Duration::from_millis(100));
        ghost.announce_at(now);
        if let Some(ev) = app.tick_at(now).unwrap() {
            events.push(ev);
        }
        if lab.ups() == 1 {
            return events;
        }
    }
    panic!("tunnel never brought up; state {:?}", app.state());
}

#[test]
fn not_connected_until_first_rx() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connecting_app(&lab);
    let mut ghost = Ghost::new();
    let mut clock = Clock(Instant::now());
    ghost.announce();

    let events = tick_until_up(&mut app, &lab, &mut clock, &mut ghost);
    assert!(!events.contains(&UiEvent::Connected), "{events:?}");
    for _ in 0..5 {
        app.tick_at(clock.advance(Duration::from_millis(200)))
            .unwrap();
    }
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert_eq!(app.status_text(), "Connecting");

    lab.set_rx(148);
    let ev = app
        .tick_at(clock.advance(Duration::from_millis(200)))
        .unwrap();
    assert_eq!(ev, Some(UiEvent::Connected));
    assert_eq!(app.state(), ConnectionState::Connected);
}

#[test]
fn hellos_retransmit_until_the_ghost_answers_and_stop_after_the_data() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connecting_app(&lab);
    let mut ghost = Ghost::new();
    let mut clock = Clock(Instant::now());

    app.tick_at(clock.0).unwrap();
    assert_eq!(ghost.frames(), 1, "no first hello");
    app.tick_at(clock.advance(ANNOUNCE_INTERVAL)).unwrap();
    assert_eq!(ghost.frames(), 1, "no hello retransmission");

    ghost.announce();
    tick_until_up(&mut app, &lab, &mut clock, &mut ghost);

    // The Hello is the liveness of the attempt: it keeps coming while the
    // tunnel is up but no data arrived.
    for _ in 0..3 {
        app.tick_at(clock.advance(ANNOUNCE_INTERVAL)).unwrap();
    }
    assert!(ghost.frames() >= 1, "the attempt stopped announcing itself");

    // The first data ends the attempt: no more Hellos after that.
    lab.set_rx(10);
    app.tick_at(clock.advance(ANNOUNCE_INTERVAL)).unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    // Drain the Hello that was already on the wire when the data arrived.
    ghost.frames();
    for _ in 0..2 {
        app.tick_at(clock.advance(ANNOUNCE_INTERVAL)).unwrap();
    }
    assert_eq!(ghost.frames(), 0, "hellos continue after the data");
}

// A first Hello that is lost before the peer has a socket is covered by
// `hellos_retransmit_until_the_ghost_answers_and_stop_after_the_data`; the old
// 0x02-specific loss test has no separate PAKE equivalent.

#[test]
fn gives_up_connect_timeout_after_tunnel_up_without_rx() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connecting_app(&lab);
    let mut ghost = Ghost::new();
    let mut clock = Clock(Instant::now());
    ghost.announce();
    tick_until_up(&mut app, &lab, &mut clock, &mut ghost);
    // First tick after bring-up starts the (fresh) window.
    let t = clock.advance(Duration::from_millis(100));
    app.tick_at(t).unwrap();

    app.tick_at(t + CONNECT_TIMEOUT - Duration::from_secs(1))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    // Only calls after the bring-up count: the harness's rx file exists from
    // the start, so `start` may run a no-op teardown first.
    let after_up = lab
        .calls()
        .rsplit("wg-quick.sh up jafn0")
        .next()
        .unwrap()
        .to_string();
    assert!(!after_up.contains("wg-quick.sh jafn0 /etc"), "{after_up}");

    let ev = app.tick_at(t + CONNECT_TIMEOUT).unwrap();
    assert!(matches!(ev, Some(UiEvent::Disconnected { .. })), "{ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
    assert!(
        lab.calls().contains("ip link del dev jafn0"),
        "teardown was not requested: {:?}",
        lab.calls()
    );
}

// Lab: after Retry the FIRST attempt against a restarted peer wasted
// 25 s. While Searching nobody reads the handshake socket, so it queues our own
// looped-back frames; Connecting then reads ONE frame per tick and the peer's
// 0x02 sits behind the backlog.
#[test]
fn first_attempt_after_a_retry_connects_without_draining_a_backlog() {
    use jafnify_core::pairing::DiscoveryToken;
    use jafnify_core::Discovery;

    let _g = multicast_lock();
    let lab = Lab::new();
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    };
    let disc = Discovery::bind(&cfg).unwrap();
    let mut app = connecting_app(&lab);
    lab.unsaveable();
    let mut clock = Clock(Instant::now());

    // Normal first pairing, then the peer dies (rx flat) -> Retry -> Searching.
    let mut ghost = Ghost::new();
    ghost.announce();
    tick_until_up(&mut app, &lab, &mut clock, &mut ghost);
    lab.set_rx(5);
    app.tick_at(clock.advance(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    for _ in 0..120 {
        app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
        if app.state() == ConnectionState::Searching {
            break;
        }
    }
    assert_eq!(app.state(), ConnectionState::Searching);
    // A lone minute of searching: one 0x01 per second, all looped back.
    for _ in 0..50 {
        app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
    }
    assert_eq!(lab.ups(), 1);

    // A fresh peer appears: it is found, pairs, and says its 0x02.
    let mut fresh = Ghost::new();
    let nonce = [9u8; 16];
    let mut connecting = false;
    for _ in 0..40 {
        disc.announce(&DiscoveryToken::derive(&nonce)).unwrap();
        app.tick_at(clock.advance(Duration::from_millis(250)))
            .unwrap();
        if app.state() == ConnectionState::Connecting {
            connecting = true;
            break;
        }
    }
    assert!(connecting, "never reached Connecting: {:?}", app.state());
    // The tunnel must come up within ~2 s of the first Hello, not after
    // ~25 s. The fresh ghost polls with the App so it can answer the Confirm.
    for _ in 0..8 {
        fresh.announce();
        app.tick_at(clock.advance(Duration::from_millis(250)))
            .unwrap();
    }
    assert_eq!(
        lab.ups(),
        2,
        "first attempt after Retry wasted time: state {:?}, status {:?}, calls {:?}",
        app.state(),
        app.status_text(),
        lab.calls()
    );
}

// A wrong-code Hello sets a diagnostic flag but never aborts the
// search. The attempt still ends at the normal Connecting timeout.
#[test]
fn a_flood_of_wrong_code_hellos_does_not_stop_the_pairing() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = connecting_app(&lab);
    let mut clock = Clock(Instant::now());
    let guesser_key = KeyPair::generate();
    let wrong = PairingCode::parse("5000").unwrap();
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    };
    let mut guesser = PakeLink::open(
        &cfg,
        Role::Sharer,
        &wrong,
        &guesser_key,
        jafnify_core::LISTEN_PORT,
    )
    .unwrap();
    for _ in 0..10 {
        guesser.poll(Instant::now()).unwrap();
        app.tick_at(clock.advance(Duration::from_millis(20)))
            .unwrap();
    }
    assert_eq!(
        app.state(),
        ConnectionState::Connecting,
        "{}",
        app.status_text()
    );
    assert!(
        !lab.calls().contains("wg-quick.sh up jafn0"),
        "tunnel raised for a guesser"
    );

    let ev = app.tick_at(clock.advance(CONNECT_TIMEOUT)).unwrap();
    assert!(matches!(ev, Some(UiEvent::Disconnected { .. })), "{ev:?}");
    assert_eq!(app.state(), ConnectionState::Retry);
}

/// Up, then the peer goes silent: Retry. Returns with the app in Retry.
fn connected_then_silent(lab: &Lab, app: &mut App, clock: &mut Clock) {
    let mut ghost = Ghost::new();
    ghost.announce();
    tick_until_up(app, lab, clock, &mut ghost);
    lab.set_rx(5);
    app.tick_at(clock.advance(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    for _ in 0..120 {
        app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
        if app.state() == ConnectionState::Retry {
            return;
        }
    }
    panic!("never went to Retry: {:?}", app.state());
}

// With the kill switch on, a lost
// tunnel is NOT taken down: the interface keeps its kill switch and no traffic
// falls back to the plain hotspot while the app retries.
#[test]
fn with_the_kill_switch_on_a_lost_tunnel_is_not_taken_down() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app();
    app.set_kill_switch(true).unwrap();
    // Saving the setting created the state file; the pairing must not be stored.
    std::fs::remove_file(lab.dir.path().join("cfg/state.toml")).unwrap();
    lab.unsaveable();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    let mut clock = Clock(Instant::now());
    connected_then_silent(&lab, &mut app, &mut clock);
    // Only the calls after the (first) bring-up count: the harness's rx file
    // exists before any tunnel does, so a first bring-up may run a no-op down.
    let calls = lab.calls();
    let after_up = calls.rsplit("wg-quick.sh up jafn0").next().unwrap();
    assert!(
        !after_up.contains("wg-quick.sh jafn0 /etc"),
        "tunnel taken down under the kill switch: {calls}"
    );
}

// The retry after a lost tunnel brings it up again. When the old interface
// is still there (kill switch on), it is taken down first, then raised.
#[test]
fn a_reconnect_with_the_old_interface_still_up_takes_it_down_first() {
    use jafnify_core::pairing::DiscoveryToken;
    use jafnify_core::Discovery;

    let _g = multicast_lock();
    let lab = Lab::new();
    let cfg = DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    };
    let disc = Discovery::bind(&cfg).unwrap();
    let mut app = lab.app();
    app.set_kill_switch(true).unwrap();
    std::fs::remove_file(lab.dir.path().join("cfg/state.toml")).unwrap();
    lab.unsaveable();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    let mut clock = Clock(Instant::now());
    connected_then_silent(&lab, &mut app, &mut clock);

    for _ in 0..120 {
        if app.state() == ConnectionState::Searching {
            break;
        }
        app.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
    }
    let mut fresh = Ghost::new();
    let nonce = [7u8; 16];
    for _ in 0..40 {
        disc.announce(&DiscoveryToken::derive(&nonce)).unwrap();
        app.tick_at(clock.advance(Duration::from_millis(250)))
            .unwrap();
        if app.state() == ConnectionState::Connecting {
            break;
        }
    }
    for _ in 0..12 {
        fresh.announce();
        app.tick_at(clock.advance(Duration::from_millis(250)))
            .unwrap();
    }
    let calls = lab.calls();
    let parts: Vec<&str> = calls.split("wg-quick.sh up jafn0").collect();
    assert_eq!(parts.len(), 3, "expected two bring-ups: {calls}");
    assert!(
        parts[1].contains("ip link del dev jafn0"),
        "old interface not taken down before the reconnect: {calls}"
    );
}
