//! While `jafn0` exists, the shared `suppress_prefixlength 0` rule keeps the
//! hotspot LAN reachable. Another tool (wg-quick down of a co-existing wg0)
//! can delete it. The App checks without root and re-adds it through the fixed
//! helper verb, without blocking the state loop.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use jafnify_core::ConnectionState;
use jafnify_linux::app::{App, AppBuilder};

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new(rule_present: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ip = dir.path().join("ip");
        std::fs::write(
            &ip,
            "#!/bin/sh\n\
             echo \"$@\" >> \"$(dirname \"$0\")/ip.log\"\n\
             case \"$*\" in\n\
               \"-4 rule show\") cat \"$(dirname \"$0\")/rules\" 2>/dev/null; exit 0;;\n\
             esac\n\
             exit 0\n",
        )
        .unwrap();
        let rules = dir.path().join("rules");
        if rule_present {
            std::fs::write(
                &rules,
                "0:\tfrom all lookup local\n\
                 32764:\tfrom all lookup main suppress_prefixlength 0\n",
            )
            .unwrap();
        }
        let helper = dir.path().join("jafnify-helper");
        std::fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();
        let pkexec = dir.path().join("pkexec");
        std::fs::write(
            &pkexec,
            "#!/bin/sh\n\
             echo \"$@\" >> \"$(dirname \"$0\")/calls.log\"\n\
             exit 0\n",
        )
        .unwrap();
        for p in [&ip, &helper, &pkexec] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn p(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.p("calls.log")).unwrap_or_default()
    }

    fn app(&self) -> App {
        std::fs::write(self.p("rx_bytes"), "1\n").unwrap();
        AppBuilder::new(self.p("cfg"))
            .ip_bin(self.p("ip"))
            .pkexec_bin(self.p("pkexec"))
            .helper_bin(self.p("jafnify-helper"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.p("rx_bytes"))
            .build()
    }
}

fn connected_app(lab: &Lab) -> App {
    let mut app = lab.app();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().confirm_connected();
    assert_eq!(app.state(), ConnectionState::Connected);
    app
}

#[test]
fn a_missing_suppress_rule_is_readded_without_blocking() {
    let lab = Lab::new(false);
    let mut app = connected_app(&lab);
    let t0 = Instant::now();

    let started = Instant::now();
    app.tick_at(t0).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "a pending helper call blocked the state loop"
    );
    assert_eq!(app.state(), ConnectionState::Connected);

    let end = Instant::now() + Duration::from_secs(2);
    while !lab.calls().contains("suppress") && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        lab.calls().contains("suppress"),
        "helper suppress not called: {:?}",
        lab.calls()
    );
}

#[test]
fn an_existing_suppress_rule_runs_no_helper() {
    let lab = Lab::new(true);
    let mut app = connected_app(&lab);
    app.tick_at(Instant::now()).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let calls = lab.calls();
    assert!(
        !calls.contains("suppress"),
        "the suppress helper must not run: {calls:?}"
    );
    assert!(
        calls
            .lines()
            .all(|line| line.ends_with("jafnify-helper handshake-age")),
        "the only helper call while Connected is the read-only handshake age: {calls:?}"
    );
}

#[test]
fn the_suppress_check_is_throttled() {
    let lab = Lab::new(false);
    let mut app = connected_app(&lab);
    let t0 = Instant::now();
    app.tick_at(t0).unwrap();
    let first = std::fs::read_to_string(lab.p("ip.log")).unwrap();
    assert_eq!(first.lines().filter(|l| *l == "-4 rule show").count(), 1);

    // Let the first helper exit and be reaped before the next window.
    std::thread::sleep(Duration::from_millis(100));
    app.tick_at(t0 + Duration::from_secs(1)).unwrap();
    let second = std::fs::read_to_string(lab.p("ip.log")).unwrap();
    assert_eq!(second.lines().filter(|l| *l == "-4 rule show").count(), 1);

    app.tick_at(t0 + Duration::from_secs(3)).unwrap();
    let third = std::fs::read_to_string(lab.p("ip.log")).unwrap();
    assert_eq!(third.lines().filter(|l| *l == "-4 rule show").count(), 2);
}
