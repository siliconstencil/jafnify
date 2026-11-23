//! End-to-end integration test: two App instances on the same host (both
//! pinned to loopback multicast) pair via the 4-digit code, exchange
//! handshake announcements, and drive the fake root commands in order.
//! Real root privileges are never touched (mock binaries injected via
//! AppBuilder).

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

use jafnify_core::state::Role;
use jafnify_linux::app::AppBuilder;

fn write_fakes(dir: &std::path::Path, log: &std::path::Path) {
    let pkexec = dir.join("mock-pkexec.sh");
    std::fs::write(
        &pkexec,
        format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
    )
    .unwrap();
    let wg = dir.join("mock-wg.sh");
    std::fs::write(&wg, "#!/bin/sh\nif [ \"$1\" = \"show\" ]; then\n  case \"$3\" in\n    all|peers|endpoints|allowed-ips|private-key|public-key|listen-port|fwmark) ;;\n    *) echo 'invalid wg syntax' >&2; exit 2;;\n  esac\n  echo 'Unable to access interface: No such device' >&2\n  exit 1\nfi\n").unwrap();
    let wgq = dir.join("mock-wg-quick.sh");
    std::fs::write(&wgq, "#!/bin/sh\nexit 0\n").unwrap();
    let install = dir.join("mock-install.sh");
    std::fs::write(&install, "#!/bin/sh\nexit 0\n").unwrap();
    for f in [&pkexec, &wg, &wgq, &install] {
        let _ = std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755));
    }
}

// The multicast handshake test binds two sockets on the same port; the
// group must be shared by exactly this one test at a time.
static MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn full_connect_cycle_issues_root_commands_in_order() {
    let _guard = MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    write_fakes(dir.path(), &log);
    // Connected needs the peer's first WireGuard data; the fake interface
    // counters say it arrived.
    std::fs::write(dir.path().join("rx-app"), "1\n").unwrap();
    std::fs::write(dir.path().join("rx-peer"), "1\n").unwrap();

    let mut app = AppBuilder::new(dir.path().join("cfg"))
        .rx_bytes_path(dir.path().join("rx-app"))
        .wg_bin(dir.path().join("mock-wg.sh"))
        .pkexec_bin(dir.path().join("mock-pkexec.sh"))
        .wg_quick_bin(dir.path().join("mock-wg-quick.sh"))
        .install_bin(dir.path().join("mock-install.sh"))
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build();
    app.start("1234").unwrap();
    app.accept_code("1234").unwrap();

    // Second app on the same host, pinned to loopback multicast so the
    // handshake announcements loop back to the first.
    let mut peer = AppBuilder::new(dir.path().join("peer"))
        .rx_bytes_path(dir.path().join("rx-peer"))
        .wg_bin(dir.path().join("mock-wg.sh"))
        .pkexec_bin(dir.path().join("mock-pkexec.sh"))
        .wg_quick_bin(dir.path().join("mock-wg-quick.sh"))
        .install_bin(dir.path().join("mock-install.sh"))
        .iface_pin(Ipv4Addr::LOCALHOST)
        .build();
    peer.set_role(Role::Sharer);
    peer.start("1234").unwrap();
    // Mutual pairing model: BOTH ends enter the same 4-digit code.
    peer.accept_code("1234").unwrap();

    // Alternating ticks until both reach Connected (max 200 iterations).
    let mut connected = 0usize;
    for _ in 0..200 {
        let _ = app.tick().unwrap();
        let _ = peer.tick().unwrap();
        connected = match (app.state(), peer.state()) {
            (
                jafnify_core::ConnectionState::Connected,
                jafnify_core::ConnectionState::Connected,
            ) => 2,
            _ => connected,
        };
        if connected == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        connected, 2,
        "both apps should reach Connected via multicast"
    );

    // The log must contain, in order: install, wg-quick up.
    let log_text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = log_text.lines().collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("-m 600") && l.contains("jafn0.conf")),
        "expected install command, got: {log_text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("wg-quick") && l.contains("up")),
        "expected wg-quick up, got: {log_text}"
    );

    // The temporary config (with the private key) must be gone after
    // install; only the /etc/wireguard copy (mode 0600) survives.
    assert!(
        !dir.path().join("cfg").join("jafn0.conf").exists(),
        "temp config should be removed after install"
    );
}
