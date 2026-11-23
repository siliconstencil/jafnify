// linux/tests/backend_test.rs
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use jafnify_linux::backend::{
    interface_present_at, interface_state, parse_wg_show_all, InterfaceState, WgBackend, WgError,
    WgStatus,
};

/// Serializes fake-wg runs: parallel test processes share the multicast port
/// and can race on fake binaries (backend_not_found flaked in parallel runs).
static MUTEX: Mutex<()> = Mutex::new(());

const FIXTURE: &str = "interface: jafn0
  public key: AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdefghi=
  listening port: 9797

peer: QwErTyUiOpAsDfGhJkLzXcVbNm0123456789qwertyuiop=
  endpoint: 192.168.43.1:9797
  allowed ips: 10.9.9.1/32
  latest handshake: 5 seconds ago
  transfer: 1.23 KiB received, 4.56 KiB sent
";

#[test]
fn parse_full_fixture() {
    let _guard = MUTEX.lock().unwrap();
    let s: WgStatus = parse_wg_show_all(FIXTURE);
    assert!(s.interface_up);
    assert_eq!(s.listen_port, Some(9797));
    assert_eq!(
        s.public_key.as_deref(),
        Some("AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdefghi=")
    );
    assert_eq!(s.peers.len(), 1);
    assert_eq!(s.peers[0].endpoint.as_deref(), Some("192.168.43.1:9797"));
    assert_eq!(
        s.peers[0].latest_handshake.as_deref(),
        Some("5 seconds ago")
    );
}

#[test]
fn parse_missing_interface_is_down() {
    let _guard = MUTEX.lock().unwrap();
    let s = parse_wg_show_all("Unable to access interface: No such device");
    assert!(!s.interface_up);
    assert!(s.peers.is_empty());
}

#[test]
fn parse_empty_interface() {
    let _guard = MUTEX.lock().unwrap();
    let s = parse_wg_show_all("interface: jafn0\n");
    assert!(s.interface_up);
    assert!(s.peers.is_empty());
}

#[test]
fn backend_with_fake_wg_reads_status() {
    let _guard = MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("wg");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" != \"show\" ] || [ \"$3\" != \"all\" ]; then\n\
           echo 'invalid wg syntax' >&2\n\
           exit 2\n\
         fi\n\
         cat <<'EOF'\ninterface: jafn0\n  public key: fake\n  listening port: 9797\nEOF\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let backend = WgBackend {
        wg_bin: fake.clone(),
    };
    let s = backend.status("jafn0").unwrap();
    assert!(s.interface_up);
    assert_eq!(s.public_key.as_deref(), Some("fake"));
}

#[test]
fn backend_missing_wg_errors() {
    let _guard = MUTEX.lock().unwrap();
    let backend = WgBackend {
        wg_bin: PathBuf::from("/nonexistent/wg"),
    };
    let err = backend.status("jafn0").unwrap_err();
    assert!(matches!(err, WgError::Io(_)));
}

#[test]
fn a_status_read_that_needs_root_is_not_reported_down() {
    // The live bug: `wg show jafn0 all` fails for an unprivileged user, so
    // the old CLI printed "interface: down" while the tunnel carried traffic.
    assert_eq!(interface_state(false, true), InterfaceState::Up);
    assert_eq!(interface_state(true, false), InterfaceState::Up);
    assert_eq!(interface_state(false, false), InterfaceState::Down);
}

#[test]
fn interface_presence_comes_from_sysfs() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!interface_present_at(dir.path(), "jafn0"));
    std::fs::create_dir_all(dir.path().join("jafn0")).unwrap();
    assert!(interface_present_at(dir.path(), "jafn0"));
}

#[test]
fn backend_not_found_interface_detected() {
    let _guard = MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("wg");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'Unable to access interface: No such device' >&2\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let backend = WgBackend {
        wg_bin: fake.clone(),
    };
    let err = backend.status("jafn0").unwrap_err();
    assert!(matches!(err, WgError::NotFound));
    let _ = Command::new("true"); // keep Command import used on all platforms
}
