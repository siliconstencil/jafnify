//! Stale-state cleanup at daemon start, ownership, and honest stop failures.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jafnify_core::state::{Peer, Role};
use jafnify_core::KeyPair;
use jafnify_linux::app::{App, AppBuilder};
use jafnify_linux::settings::{save_state, LinuxStateFile};

struct Lab {
    _dir: tempfile::TempDir,
    _cfg: PathBuf,
    log: PathBuf,
}

#[derive(Clone, Copy)]
enum Inspect {
    Matching,
    Different,
    Fail,
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn live_toml(peer_key: &str) -> String {
    format!(
        "peer_public_key = \"{peer_key}\"\n\
         endpoint = \"127.0.0.1:9797\"\n\
         allowed_ips = \"10.9.9.1/32\"\n\
         address = \"10.9.9.2/32\"\n\
         kill_switch = false\n"
    )
}

fn lab(paired: bool, inspect: Inspect, tunnel: bool) -> (Lab, App) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut peer_key = KeyPair::generate().public_key_b64();
    if paired {
        let own = KeyPair::generate();
        let peer = KeyPair::generate();
        peer_key = peer.public_key_b64();
        let state = LinuxStateFile {
            peers: vec![Peer {
                id: "00112233445566778899aabbccddeeff".to_string(),
                name: "Phone - Test".to_string(),
                public_key: peer.public_key_b64(),
                role: Role::Sharer,
                last_connected: None,
                private_key: Some(own.private_key_b64()),
                last_endpoint: Some("127.0.0.1:9797".to_string()),
            }],
            ..Default::default()
        };
        save_state(&cfg.join("state.toml"), &state).unwrap();
        jafnify_core::terms::record_acceptance(&cfg, 0).unwrap();
    }
    let live_key = match inspect {
        Inspect::Matching => peer_key.clone(),
        Inspect::Different => KeyPair::generate().public_key_b64(),
        Inspect::Fail => String::new(),
    };
    let live = dir.path().join("live.toml");
    let live_text = match inspect {
        Inspect::Fail => String::new(),
        _ => live_toml(&live_key),
    };
    std::fs::write(&live, live_text).unwrap();
    let log = dir.path().join("calls.log");
    let pkexec = script(
        dir.path(),
        "pkexec",
        &format!(
            "#!/bin/sh\n\
             echo \"$@\" >> \"{}\"\n\
             if [ \"$2\" = \"inspect\" ] && [ -s \"{}\" ]; then\n\
               cat \"{}\"\n\
             fi\n\
             exit 0\n",
            log.display(),
            live.display(),
            live.display()
        ),
    );
    let helper = script(dir.path(), "jafnify-helper", "#!/bin/sh\nexit 0\n");
    let ip = script(
        dir.path(),
        "ip",
        &format!(
            "#!/bin/sh\necho \"$@\" >> \"{}.ip\"\nexit 0\n",
            log.display()
        ),
    );
    let route = script(
        dir.path(),
        "jafnify-route",
        &format!(
            "#!/bin/sh\necho \"$@\" >> \"{}.route\"\nexit 0\n",
            log.display()
        ),
    );
    let pkcheck = script(dir.path(), "pkcheck", "#!/bin/sh\nexit 0\n");
    let rx = dir.path().join("rx");
    let iface = dir.path().join("jafn0.iface");
    if tunnel {
        std::fs::write(&rx, "10\n").unwrap();
        std::fs::create_dir_all(&iface).unwrap();
    }
    let tx = dir.path().join("tx");
    std::fs::write(&tx, "20\n").unwrap();
    let conf = dir.path().join("jafn0.conf");
    let app = AppBuilder::new(cfg.clone())
        .pkexec_bin(pkexec)
        .pkcheck_bin(pkcheck)
        .helper_bin(helper)
        .ip_bin(ip)
        .route_bin(route)
        .rx_bytes_path(rx)
        .tx_bytes_path(tx)
        .installed_conf_path(conf)
        .teardown_iface_path(iface)
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build();
    (
        Lab {
            _dir: dir,
            _cfg: cfg,
            log,
        },
        app,
    )
}

fn calls(lab: &Lab) -> String {
    std::fs::read_to_string(&lab.log).unwrap_or_default()
}

fn write_marker(lab: &Lab, peer_key: &str) {
    std::fs::write(lab._cfg.join("tunnel.toml"), live_toml(peer_key)).unwrap();
}

/// The peer key of the tunnel the fake inspect reports.
fn live_peer(lab: &Lab) -> String {
    let text = std::fs::read_to_string(lab._dir.path().join("live.toml")).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("peer_public_key = \""))
        .map(|value| value.trim_end_matches('"').to_string())
        .unwrap()
}

#[test]
fn stop_removes_an_unpaired_orphan_proven_by_its_marker() {
    // F2: an interrupted join leaves no saved peer but does leave jafn0 and
    // its marker. The marker names the live peer key, so stop may remove it.
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    write_marker(&lab, &live_peer(&lab));
    app.stop().unwrap();
    assert!(
        calls(&lab).contains("down"),
        "the orphan was left behind: {}",
        calls(&lab)
    );
}

#[test]
fn stop_never_removes_an_unpaired_tunnel_without_a_marker() {
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    app.stop().unwrap();
    assert!(
        !calls(&lab).contains("down"),
        "a foreign tunnel was torn down: {}",
        calls(&lab)
    );
    assert!(calls(&lab).is_empty(), "{}", calls(&lab));
}

#[test]
fn stop_never_removes_a_tunnel_a_stale_marker_does_not_name() {
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    write_marker(&lab, &KeyPair::generate().public_key_b64());
    app.stop().unwrap();
    assert!(
        !calls(&lab).contains("down"),
        "a marker proved another tunnel: {}",
        calls(&lab)
    );
}

#[test]
fn cleanup_removes_an_unpaired_orphan_proven_by_its_marker() {
    // F2 at the next start: an unpaired config cannot adopt, so the marker
    // proves the tunnel is its own leftover and cleanup removes it.
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    write_marker(&lab, &live_peer(&lab));
    app.cleanup_stale_state().unwrap();
    assert!(
        calls(&lab).contains("down"),
        "the orphan survived cleanup: {}",
        calls(&lab)
    );
}

#[test]
fn cleanup_never_touches_an_unpaired_instances_foreign_tunnel() {
    // The live incident: a second, unpaired instance saw the other
    // instance's jafn0 and ran `helper down` at startup.
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    app.cleanup_stale_state().unwrap();
    assert!(
        !calls(&lab).contains("down"),
        "a foreign tunnel was torn down: {}",
        calls(&lab)
    );
    assert!(
        calls(&lab).is_empty(),
        "no privileged call at all: {}",
        calls(&lab)
    );
    assert!(app.foreign_tunnel());
}

#[test]
fn cleanup_is_a_no_op_when_nothing_is_left_over() {
    let (lab, mut app) = lab(false, Inspect::Matching, false);
    app.cleanup_stale_state().unwrap();
    assert!(calls(&lab).is_empty(), "{}", calls(&lab));
    assert!(!app.foreign_tunnel());
}

#[test]
fn cleanup_keeps_a_tunnel_the_stored_pair_can_adopt() {
    let (lab, mut app) = lab(true, Inspect::Matching, true);
    app.cleanup_stale_state().unwrap();
    assert!(
        !calls(&lab).contains("down"),
        "an adoptable tunnel was torn down: {}",
        calls(&lab)
    );
    assert!(!app.foreign_tunnel());
}

#[test]
fn cleanup_keeps_the_marker_proven_tunnel_without_an_inspect() {
    let (lab, mut app) = lab(true, Inspect::Fail, true);
    let peer_key = app.peers()[0].public_key.clone();
    write_marker(&lab, &peer_key);
    app.cleanup_stale_state().unwrap();
    assert!(calls(&lab).is_empty(), "{}", calls(&lab));
    assert!(!app.foreign_tunnel());
}

#[test]
fn cleanup_does_not_touch_a_paired_instances_foreign_tunnel() {
    let (lab, mut app) = lab(true, Inspect::Different, true);
    app.cleanup_stale_state().unwrap();
    assert!(
        !calls(&lab).contains("down"),
        "a foreign tunnel was torn down: {}",
        calls(&lab)
    );
    assert!(app.foreign_tunnel());
}

#[test]
fn a_foreign_tunnel_never_drives_liveness_or_repair() {
    // The second half of the incident: an idle second instance counted the
    // other tunnel's rx and ran route-fallback on it.
    let (lab, mut app) = lab(false, Inspect::Matching, true);
    app.set_role(Role::Joiner);
    for _ in 0..3 {
        app.tick().unwrap();
    }
    assert!(calls(&lab).is_empty(), "{}", calls(&lab));
    let route = std::fs::read_to_string(format!("{}.route", lab.log.display())).unwrap_or_default();
    assert!(!route.contains("route-fallback"), "{route}");
    let ip = std::fs::read_to_string(format!("{}.ip", lab.log.display())).unwrap_or_default();
    assert!(ip.is_empty(), "a foreign tunnel's rules were read: {ip}");
}

#[test]
fn the_foreign_status_clears_once_the_other_tunnel_is_gone() {
    // Live: the other instance disconnected, jafn0 vanished, yet
    // the idle instance kept "Another Jafnify connection is active" and a
    // disabled Connect until restart.
    let (lab, mut app) = lab(true, Inspect::Different, true);
    app.cleanup_stale_state().unwrap();
    assert!(app.foreign_tunnel());
    // The kernel removes the interface and its counters together.
    std::fs::remove_dir(lab._dir.path().join("jafn0.iface")).unwrap();
    std::fs::remove_file(lab._dir.path().join("rx")).unwrap();
    app.tick().unwrap();
    assert!(
        !app.foreign_tunnel(),
        "the foreign status outlived the tunnel"
    );
    assert!(!calls(&lab).contains("down"), "{}", calls(&lab));
}

#[test]
fn the_foreign_status_appears_when_another_tunnel_comes_up_while_idle() {
    let (lab, mut app) = lab(true, Inspect::Different, false);
    app.cleanup_stale_state().unwrap();
    assert!(!app.foreign_tunnel());
    std::fs::create_dir_all(lab._dir.path().join("jafn0.iface")).unwrap();
    std::fs::write(lab._dir.path().join("rx"), "10\n").unwrap();
    app.tick().unwrap();
    assert!(
        app.foreign_tunnel(),
        "an idle instance missed the other tunnel"
    );
    assert!(!calls(&lab).contains("down"), "{}", calls(&lab));
}

#[test]
fn fallback_without_a_tunnel_makes_no_privileged_call() {
    // Lab (repro-solo): a lone joiner stopped while searching ran
    // `route fallback` with no jafn0 at all. Nothing routes into a tunnel
    // that does not exist, so there is nothing to fall back from, and on a
    // desktop the call could raise a pointless password prompt.
    let (lab, mut app) = lab(false, Inspect::Matching, false);
    assert!(!app.fall_back_routing());
    assert!(calls(&lab).is_empty(), "{}", calls(&lab));
    let route = std::fs::read_to_string(format!("{}.route", lab.log.display())).unwrap_or_default();
    assert!(route.is_empty(), "{route}");
}
