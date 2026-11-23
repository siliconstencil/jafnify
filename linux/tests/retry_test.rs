//! Retry path (a step-one follow-up). After the peer vanishes the
//! App goes Connected -> Retry -> Searching. Two bugs were seen on real
//! binaries:
//!   A. with NO peer alive, the lone joiner found a "peer", connected and
//!      raised `jafn0` again (0.0.0.0/0 into a dead tunnel): stale frames from
//!      the vanished peer, still queued on the old sockets, were taken as live.
//!   B. a freshly started peer (same code) was never found: the joiner sat in
//!      Connecting forever, announcing nothing the new peer could hear.
//! Loopback multicast, fake pkexec/wg-quick, injected clock via `tick_at`.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::state::Role;
use jafnify_core::ConnectionState;
use jafnify_linux::app::{App, AppBuilder, UiEvent};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Lab {
    dir: tempfile::TempDir,
    /// Pairing -> Connecting delay. The product uses 3 s; during that time
    /// each side keeps announcing 0x01 while the other is not reading it.
    accept_delay: Duration,
}

impl Lab {
    fn new() -> Self {
        Self::with_accept_delay(Duration::from_millis(50))
    }

    fn with_accept_delay(accept_delay: Duration) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            accept_delay,
        }
    }

    fn script(&self, name: &str, body: String) -> std::path::PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// One App with its own state dir, fake pkexec (logs to `<name>.log`) and
    /// rx counter file (`<name>.rx`). The fake interface has already received
    /// the peer's WG data, so Connecting completes once the tunnel is up.
    fn app(&self, name: &str, role: Role) -> App {
        self.set_rx(name, 1);
        let log = self.dir.path().join(format!("{name}.log"));
        let pkexec = self.script(
            &format!("{name}-pkexec.sh"),
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{log}\"\n\
                 case \"$3\" in *\"-m 600\"*) echo \"wg-quick.sh up jafn0\" >> \"{log}\";; \
                   *install*) echo \"$(basename \"$6\") up $8\" >> \"{log}\";; esac\n\
                 exit 0\n",
                log = log.display()
            ),
        );
        let wg = self.script("wg.sh", "#!/bin/sh\nexit 1\n".into());
        let wgq = self.script("wg-quick.sh", "#!/bin/sh\nexit 0\n".into());
        let install = self.script("install.sh", "#!/bin/sh\nexit 0\n".into());
        let mut app = AppBuilder::new(self.dir.path().join(format!("{name}-cfg")))
            .wg_bin(wg)
            .pkexec_bin(pkexec)
            .wg_quick_bin(wgq)
            .install_bin(install)
            .iface_pin(Ipv4Addr::LOCALHOST)
            .auto_accept_delay(self.accept_delay)
            .rx_bytes_path(self.dir.path().join(format!("{name}.rx")))
            .build();
        app.set_role(role);
        app
    }

    /// Make `name`'s pairing impossible to store (state.toml is a directory),
    /// so its retries stay on the code track. A pairing that IS stored retries
    /// on the known track instead (see known_peer_test).
    fn unsaveable(&self, name: &str) {
        std::fs::create_dir_all(self.dir.path().join(format!("{name}-cfg/state.toml"))).unwrap();
    }

    fn set_rx(&self, name: &str, n: u64) {
        std::fs::write(self.dir.path().join(format!("{name}.rx")), format!("{n}\n")).unwrap();
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
}

/// Fake clock advanced in `step`s; every tick also sleeps a little real time
/// so loopback multicast gets delivered.
struct Clock(Instant);

impl Clock {
    fn advance(&mut self, step: Duration) -> Instant {
        self.0 += step;
        std::thread::sleep(Duration::from_millis(3));
        self.0
    }
}

/// Pair `a` (joiner) and `b` (sharer) through real discovery + handshake.
fn pair(a: &mut App, b: &mut App) {
    a.start("1234").unwrap();
    b.start("1234").unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while a.state() != ConnectionState::Connected || b.state() != ConnectionState::Connected {
        assert!(
            Instant::now() < end,
            "never paired: {:?} / {:?}",
            a.state(),
            b.state()
        );
        a.tick().unwrap();
        b.tick().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `a` has lost its peer (b is gone, rx flat). Drive its fake clock until it
/// is back in Searching; every event on the way is returned.
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

#[test]
fn lone_joiner_stays_searching_across_retry_after_peer_vanishes() {
    let _g = multicast_lock();
    // Production auto-accept delay (3 s): the peer's 0x01 tokens pile up
    // unread while we are in Pairing/Connecting.
    let lab = Lab::with_accept_delay(Duration::from_secs(3));
    let mut a = lab.app("a", Role::Joiner);
    let mut b = lab.app("b", Role::Sharer);
    lab.unsaveable("a");
    pair(&mut a, &mut b);
    assert_eq!(lab.ups("a"), 1);
    lab.set_rx("a", 100); // flat from here on: nothing arrives from the peer
    drop(b);

    let mut clock = Clock(Instant::now());
    let events = drive_to_searching(&mut a, &mut clock);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::Disconnected { .. })),
        "{events:?}"
    );

    // No peer exists anywhere. Another 90 s of searching must stay quiet.
    for _ in 0..90 {
        let ev = a.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
        assert!(
            !matches!(ev, Some(UiEvent::PeerFound) | Some(UiEvent::Connected)),
            "stale peer taken as live: {ev:?}"
        );
    }
    assert_eq!(a.state(), ConnectionState::Searching);
    assert_eq!(lab.ups("a"), 1, "jafn0 raised again with no peer alive");
}

#[test]
fn fresh_peer_with_same_code_connects_after_retry() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut a = lab.app("a", Role::Joiner);
    let mut b = lab.app("b", Role::Sharer);
    lab.unsaveable("a");
    pair(&mut a, &mut b);
    lab.set_rx("a", 100);
    drop(b);
    let mut clock = Clock(Instant::now());
    drive_to_searching(&mut a, &mut clock);

    // The sharer is restarted with the same code.
    let mut b2 = lab.app("b2", Role::Sharer);
    b2.start("1234").unwrap();
    let end = Instant::now() + Duration::from_secs(15);
    while a.state() != ConnectionState::Connected || b2.state() != ConnectionState::Connected {
        assert!(
            Instant::now() < end,
            "no reconnect: {:?} / {:?}",
            a.state(),
            b2.state()
        );
        // Both Apps share the fake clock (a real-clock peer would see the
        // other's timeouts run ~100x too fast).
        let now = clock.advance(Duration::from_millis(200));
        a.tick_at(now).unwrap();
        b2.tick_at(now).unwrap();
    }
    assert_eq!(lab.ups("a"), 2);
    assert_eq!(lab.ups("b2"), 1);
}

#[test]
fn connecting_with_no_peer_times_out_without_touching_the_tunnel() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut a = lab.app("a", Role::Joiner);
    a.start("1234").unwrap();
    a.accept_code("1234").unwrap();
    assert_eq!(a.state(), ConnectionState::Connecting);

    let mut clock = Clock(Instant::now());
    let mut lost = None;
    for _ in 0..60 {
        if let Some(ev) = a.tick_at(clock.advance(Duration::from_secs(1))).unwrap() {
            lost = Some(ev);
            break;
        }
    }
    assert!(
        matches!(lost, Some(UiEvent::Disconnected { .. })),
        "Connecting never gave up: {lost:?}"
    );
    assert_eq!(a.state(), ConnectionState::Retry);
    // Nothing was brought up, so no root prompt is needed to "clean up".
    assert_eq!(lab.count("a", "wg-quick"), 0);
    assert_eq!(lab.count("a", "install"), 0);
}

// Lab BUG B exactly: a joiner stuck in Connecting (peer vanished) and a
// restarted sharer in Searching must find each other without user action.
#[test]
fn joiner_stuck_in_connecting_finds_restarted_sharer() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut a = lab.app("a", Role::Joiner);
    a.start("1234").unwrap();
    a.accept_code("1234").unwrap(); // stale pairing: Connecting, nobody there
    let mut clock = Clock(Instant::now());
    for _ in 0..3 {
        a.tick_at(clock.advance(Duration::from_secs(1))).unwrap();
    }

    let mut b = lab.app("b", Role::Sharer);
    b.start("1234").unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while a.state() != ConnectionState::Connected || b.state() != ConnectionState::Connected {
        assert!(
            Instant::now() < end,
            "stuck: {:?} / {:?}",
            a.state(),
            b.state()
        );
        let now = clock.advance(Duration::from_millis(250));
        a.tick_at(now).unwrap();
        b.tick_at(now).unwrap();
    }
    assert_eq!(lab.ups("a"), 1);
}
