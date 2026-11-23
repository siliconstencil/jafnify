// linux/tests/app_test.rs
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::config::InboundGuard;
use jafnify_core::state::Role;
use jafnify_core::{
    ConfigGenerator, ConnectionState, Discovery, DiscoveryConfig, Orchestrator, JOINER_IP,
    SHARER_IP,
};
use jafnify_linux::app::{
    build_config, parse_iface_ipv4, parse_lan_iface, App, AppBuilder, AppError, UiEvent,
};

// Tests that tick an App share the multicast port; run them one at a time.
static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

/// App pinned to loopback multicast. pkexec points nowhere, so no test can
/// ever reach a real root command.
fn loopback_app(dir: &tempfile::TempDir, auto_accept: Duration) -> App {
    AppBuilder::new(dir.path().to_path_buf())
        .auto_accept_delay(auto_accept)
        .wg_bin(PathBuf::from("/nonexistent/wg"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build()
}

fn app_for(dir: &tempfile::TempDir) -> App {
    AppBuilder::new(dir.path().to_path_buf())
        .wg_bin(PathBuf::from("/nonexistent/wg"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build()
}

#[test]
fn new_loads_defaults_with_missing_config() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_for(&dir);
    assert_eq!(app.state(), ConnectionState::Idle);
    assert_eq!(app.status_text(), "Not sharing");
    assert!(app.peers().is_empty());
}

#[test]
fn start_rejects_malformed_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for(&dir);
    let err = app.start("12ab").unwrap_err();
    assert!(matches!(err, jafnify_linux::app::AppError::InvalidCode));
}

#[test]
fn start_searches_after_valid_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for(&dir);
    app.start("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
}

#[test]
fn accept_wrong_code_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for(&dir);
    app.start("1234").unwrap();
    let err = app.accept_code("5678").unwrap_err();
    assert!(matches!(
        err,
        jafnify_linux::app::AppError::Pairing(jafnify_core::pairing::PairingError::CodeMismatch)
    ));
    assert_eq!(app.state(), ConnectionState::Searching);
}

#[test]
fn accept_correct_code_moves_to_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for(&dir);
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
}

#[test]
fn sharer_config_has_no_endpoint() {
    let cfg = build_config(
        Role::Sharer,
        &("A".repeat(43) + "="),
        &("B".repeat(43) + "="),
        Some("192.168.43.1:9797".parse().unwrap()),
        InboundGuard::Iptables,
        false,
        None,
    );
    assert_eq!(cfg.interface.address, SHARER_IP);
    assert_eq!(cfg.peer.endpoint, None);
    assert_eq!(cfg.interface.mtu, Some(1280));
}

#[test]
fn joiner_config_has_endpoint_from_source_ip() {
    let cfg = build_config(
        Role::Joiner,
        &("A".repeat(43) + "="),
        &("B".repeat(43) + "="),
        Some("192.168.43.1:9797".parse().unwrap()),
        InboundGuard::Iptables,
        false,
        None,
    );
    assert_eq!(cfg.interface.address, JOINER_IP);
    assert_eq!(cfg.peer.endpoint.as_deref(), Some("192.168.43.1:9797"));
    assert_eq!(cfg.peer.persistent_keepalive, Some(25));
}

#[test]
fn status_text_reflects_disconnect_reason() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for(&dir);
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().on_disconnect("link down".into());
    assert!(app.status_text().contains("link down"));
    let _ = SocketAddr::from(([0, 0, 0, 0], 0)); // keep import used
    let _ = ConfigGenerator; // keep import used
    let _ = Orchestrator::default(); // keep import used
}

// Findings 2+4 (lab report): discovery tokens (frame 0x01) must be
// announced, and auto-accept must be armed by a PEER being found -- never by
// start() alone, or a lone device pairs with its own multicast loopback.

#[test]
fn lone_joiner_stays_searching() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let mut app = loopback_app(&dir, Duration::from_millis(1));
    app.start("1234").unwrap();
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end {
        let ev = app.tick().unwrap();
        assert_ne!(ev, Some(UiEvent::PeerFound), "paired with itself");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_eq!(app.status_text(), "Searching for the other device");
}

#[test]
fn searching_app_announces_discovery_token() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let listener = Discovery::bind(&DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        ..Default::default()
    })
    .unwrap();
    let mut app = loopback_app(&dir, Duration::from_secs(3));
    app.start("1234").unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    let mut heard = false;
    while Instant::now() < end && !heard {
        app.tick().unwrap();
        if let Some((_, token)) = listener.try_receive(Duration::from_millis(50)).unwrap() {
            assert!(token.verify());
            heard = true;
        }
    }
    assert!(heard, "no 0x01 discovery frame announced while Searching");
}

#[test]
fn two_apps_find_each_other_then_auto_accept() {
    let _g = multicast_lock();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = loopback_app(&d1, Duration::from_millis(50));
    let mut b = loopback_app(&d2, Duration::from_millis(50));
    a.start("1234").unwrap();
    b.start("1234").unwrap();
    assert_eq!(a.state(), ConnectionState::Searching);
    assert_eq!(b.state(), ConnectionState::Searching);
    let end = Instant::now() + Duration::from_secs(10);
    let mut peer_found = 0;
    while a.state() != ConnectionState::Connecting || b.state() != ConnectionState::Connecting {
        assert!(
            Instant::now() < end,
            "peers never paired: {:?} / {:?}",
            a.state(),
            b.state()
        );
        // Stop ticking an app once Connecting: it would otherwise react to
        // the other's handshake frame.
        for app in [&mut a, &mut b] {
            if app.state() != ConnectionState::Connecting
                && app.tick().unwrap() == Some(UiEvent::PeerFound)
            {
                peer_found += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(peer_found, 2, "each side must report PeerFound");
}

#[test]
fn connecting_app_ignores_its_own_handshake_announcement() {
    let _g = multicast_lock();
    let dir = tempfile::tempdir().unwrap();
    let mut app = loopback_app(&dir, Duration::from_secs(3));
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    // Our own 0x02 frame loops back; it must not be taken for a peer.
    for _ in 0..30 {
        let ev = app.tick().expect("own announcement must be ignored");
        assert_ne!(ev, Some(UiEvent::Connected));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert!(app.last_error().is_none());
    assert!(!dir.path().join("jafn0.conf").exists());
}

// `ip route get default` answers `local 0.0.0.0 dev lo src 127.0.0.1`, which
// pinned discovery to loopback (lab report, finding 1). The LAN
// interface now comes from the default-route table, skipping tunnels.

#[test]
fn lan_iface_is_default_route_dev() {
    let out = "default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n";
    assert_eq!(parse_lan_iface(out).as_deref(), Some("wlan0"));
}

#[test]
fn lan_iface_skips_tunnel_default_routes() {
    let out = "default dev wg0 scope link\n\
               default dev jafn0 scope link\n\
               default dev tun0 scope link\n\
               default via 10.0.0.1 dev eth0 proto dhcp metric 100\n";
    assert_eq!(parse_lan_iface(out).as_deref(), Some("eth0"));
}

#[test]
fn lan_iface_none_when_only_tunnel_or_loopback() {
    assert_eq!(parse_lan_iface("default dev wg0 scope link\n"), None);
    assert_eq!(parse_lan_iface("default dev lo scope link\n"), None);
    // The old (buggy) `ip route get default` output must never yield an iface.
    assert_eq!(
        parse_lan_iface("local 0.0.0.0 dev lo src 127.0.0.1 uid 1000\n"),
        None
    );
    assert_eq!(parse_lan_iface(""), None);
}

#[test]
fn iface_ipv4_is_first_non_loopback_inet() {
    let out = "3: wlan0    inet 192.168.1.5/24 brd 192.168.1.255 scope global dynamic wlan0\n";
    assert_eq!(parse_iface_ipv4(out), Some(Ipv4Addr::new(192, 168, 1, 5)));
    assert_eq!(
        parse_iface_ipv4("1: lo    inet 127.0.0.1/8 scope host lo\n"),
        None
    );
    assert_eq!(parse_iface_ipv4(""), None);
}

#[test]
fn start_without_lan_interface_errors_instead_of_unpinned() {
    // An unpinned socket would send multicast into wg0 (policy table), so
    // start() must refuse rather than fall back to the kernel default.
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().to_path_buf())
        .wg_bin(PathBuf::from("/nonexistent/wg"))
        .ip_bin(PathBuf::from("/nonexistent/ip"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .build();
    let err = app.start("1234").unwrap_err();
    assert!(matches!(err, AppError::NoLanInterface));
    assert!(err.to_string().contains("no LAN interface found"));
    assert_eq!(app.state(), ConnectionState::Idle);
    assert!(app
        .last_error()
        .is_some_and(|e| e.contains("no LAN interface found")));
}

#[test]
fn iface_pin_override_skips_detection() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = AppBuilder::new(dir.path().to_path_buf())
        .wg_bin(PathBuf::from("/nonexistent/wg"))
        .ip_bin(PathBuf::from("/nonexistent/ip"))
        .pkexec_bin(PathBuf::from("/nonexistent/pkexec"))
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build();
    app.start("1234").unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
}
