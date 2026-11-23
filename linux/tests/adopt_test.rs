//! Adopting a live tunnel only after the helper confirms it and the peer sends
//! data. A marker is a hint, never proof; a refused teardown must stay stopped.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::state::{Peer, Role};
use jafnify_core::{ConnectionState, KeyPair};
use jafnify_linux::app::{App, AppBuilder, CONNECT_TIMEOUT};
use jafnify_linux::applog::Logger;
use jafnify_linux::helper::LiveConfig;
use jafnify_linux::settings::{save_state, LinuxStateFile};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Lab {
    dir: tempfile::TempDir,
    cfg: std::path::PathBuf,
    peer: KeyPair,
    log: std::path::PathBuf,
    rx: std::path::PathBuf,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let own = KeyPair::generate();
        let peer = KeyPair::generate();
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
        let log = dir.path().join("calls.log");
        let rx = dir.path().join("rx");
        Self {
            dir,
            cfg,
            peer,
            log,
            rx,
        }
    }

    fn live(&self) -> LiveConfig {
        LiveConfig {
            peer_public_key: self.peer.public_key_b64(),
            endpoint: Some("127.0.0.1:9797".to_string()),
            allowed_ips: "0.0.0.0/0".to_string(),
            address: "10.9.9.2".to_string(),
            kill_switch: false,
        }
    }

    fn script(&self, name: &str, body: &str) -> std::path::PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// `live` is what the helper's inspect prints. `down_code` is the exit code
    /// of the helper's down; 0 also removes the rx counter file.
    fn app(
        &self,
        marker: Option<&str>,
        live: Option<&LiveConfig>,
        rx: u64,
        down_code: i32,
        with_helper: bool,
    ) -> App {
        std::fs::write(&self.rx, format!("{rx}\n")).unwrap();
        let tx = self.dir.path().join("tx");
        std::fs::write(&tx, "20\n").unwrap();
        let marker_path = self.dir.path().join("tunnel.toml");
        let _ = std::fs::remove_file(&marker_path);
        if let Some(pk) = marker {
            std::fs::write(&marker_path, format!("peer_public_key = \"{pk}\"\n")).unwrap();
        }
        let live_path = self.dir.path().join("live.toml");
        let _ = std::fs::remove_file(&live_path);
        if let Some(live) = live {
            std::fs::write(&live_path, live.to_toml().unwrap()).unwrap();
        }
        let helper = self.script(
            "jafnify-helper",
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = inspect ]; then\n\
                   [ -f \"{live}\" ] && cat \"{live}\" && exit 0\n\
                   exit 1\n\
                 fi\n\
                 if [ \"$1\" = down ]; then\n\
                   if [ {down} -eq 0 ]; then rm -f \"{rx}\"; fi\n\
                   exit {down}\n\
                 fi\n\
                 exit 0\n",
                live = live_path.display(),
                down = down_code,
                rx = self.rx.display(),
            ),
        );
        let pkexec = self.script(
            "pkexec",
            &format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\nfor a in \"$@\"; do case \"$a\" in *jafnify-route*|*route-fallback*|*route-restore*|*resolvectl*|*jafn0*fwmark*) exit 0;; esac; done\nexec \"$@\"\n",
                self.log.display()
            ),
        );
        let wgq = self.script("wg-quick", "#!/bin/sh\nexit 0\n");
        let helper_bin = if with_helper {
            helper
        } else {
            self.dir.path().join("no-helper")
        };
        AppBuilder::new(self.cfg.clone())
            .rx_bytes_path(self.rx.clone())
            .tx_bytes_path(tx)
            .marker_path(marker_path)
            .pkexec_bin(pkexec)
            .helper_bin(helper_bin)
            .wg_quick_bin(wgq)
            .iface_pin(Ipv4Addr::LOCALHOST)
            .logger(Some(
                Logger::open(self.dir.path().join("app.log"))
                    .unwrap()
                    .without_stderr(),
            ))
            .build()
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(&self.rx, format!("{n}\n")).unwrap();
    }

    fn marker_exists(&self) -> bool {
        self.dir.path().join("tunnel.toml").exists()
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn app_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("app.log")).unwrap_or_default()
    }
}

#[test]
fn matching_live_config_waits_for_peer_data() {
    let lab = Lab::new();
    let live = lab.live();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), Some(&live), 10, 0, true);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert_ne!(app.state(), ConnectionState::Connected);
    assert!(lab.calls().contains("inspect"), "{}", lab.calls());
    assert!(
        lab.app_log()
            .contains("adopted existing jafn0 after inspect"),
        "{}",
        lab.app_log()
    );

    lab.set_rx(11);
    app.tick().unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
}

#[test]
fn matching_live_config_without_peer_data_falls_back_to_normal_connect() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let live = lab.live();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), Some(&live), 10, 0, true);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    app.tick_at(Instant::now() + CONNECT_TIMEOUT + Duration::from_secs(1))
        .unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_ne!(app.state(), ConnectionState::Connected);
    assert!(!lab.marker_exists(), "fallback left the adoption marker");
    assert!(lab.calls().contains("inspect"), "{}", lab.calls());
    assert!(lab.calls().contains("down"), "{}", lab.calls());
}

#[test]
fn a_matching_marker_still_requires_inspect() {
    let lab = Lab::new();
    let live = lab.live();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), Some(&live), 10, 0, true);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert!(lab.calls().contains("inspect"), "{}", lab.calls());
    assert!(!lab.calls().contains("status"), "{}", lab.calls());
}

#[test]
fn a_marker_alone_is_not_enough() {
    let lab = Lab::new();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), None, 10, 0, false);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_ne!(app.state(), ConnectionState::Connected);
    assert_eq!(lab.calls(), "");
}

#[test]
fn a_mismatched_live_config_is_not_adopted() {
    let lab = Lab::new();
    let mut live = lab.live();
    live.peer_public_key = KeyPair::generate().public_key_b64();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), Some(&live), 10, 0, true);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    assert_ne!(app.state(), ConnectionState::Connected);
    assert!(lab.calls().contains("inspect"), "{}", lab.calls());
    assert!(!lab.calls().contains("down"), "{}", lab.calls());
}

#[test]
fn a_mismatched_live_config_does_not_reuse_the_marker() {
    let lab = Lab::new();
    let mut live = lab.live();
    live.peer_public_key = KeyPair::generate().public_key_b64();
    live.address = "10.9.9.1".to_string();
    let mut app = lab.app(None, Some(&live), 10, 0, true);
    app.set_role(Role::Sharer);
    std::fs::write(
        lab.dir.path().join("tunnel.toml"),
        format!(
            "peer_public_key = \"{}\"\nallowed_ips = \"0.0.0.0/0\"\n\
             address = \"10.9.9.1\"\nkill_switch = false\n",
            lab.peer.public_key_b64()
        ),
    )
    .unwrap();

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    app.tick().unwrap();

    let calls = lab.calls();
    assert!(calls.contains("inspect"), "{calls}");
    assert!(calls.contains("down"), "{calls}");
    assert!(calls.contains("up"), "{calls}");
    assert!(
        !lab.app_log()
            .contains("reusing existing jafn0 (no privileged call)"),
        "{}",
        lab.app_log()
    );
}

#[test]
fn live_config_wins_over_a_stale_marker() {
    let lab = Lab::new();
    let live = lab.live();
    let stale = KeyPair::generate().public_key_b64();
    let mut app = lab.app(Some(&stale), Some(&live), 10, 0, true);

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert!(lab.calls().contains("inspect"), "{}", lab.calls());
}

#[test]
fn inspect_failure_does_not_reuse_the_marker() {
    let lab = Lab::new();
    let mut app = lab.app(None, None, 10, 0, true);
    app.set_role(Role::Sharer);
    std::fs::write(
        lab.dir.path().join("tunnel.toml"),
        format!(
            "peer_public_key = \"{}\"\nallowed_ips = \"0.0.0.0/0\"\n\
             address = \"10.9.9.1\"\nkill_switch = false\n",
            lab.peer.public_key_b64()
        ),
    )
    .unwrap();

    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Searching);
    app.tick().unwrap();

    let calls = lab.calls();
    assert!(calls.contains("inspect"), "{calls}");
    assert!(calls.contains("down"), "{calls}");
    assert!(calls.contains("up"), "{calls}");
    assert!(
        !lab.app_log()
            .contains("reusing existing jafn0 (no privileged call)"),
        "{}",
        lab.app_log()
    );
}

#[test]
fn a_denied_teardown_stays_idle_and_keeps_the_tunnel() {
    let lab = Lab::new();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), None, 10, 126, true);

    let err = app.stop().unwrap_err();
    assert!(matches!(
        err,
        jafnify_linux::app::AppError::PermissionDenied
    ));
    assert_eq!(app.state(), ConnectionState::Idle);
    assert_eq!(
        app.last_error(),
        Some("could not remove the tunnel: permission denied")
    );
    assert!(
        lab.marker_exists(),
        "the marker must survive a denied teardown"
    );
    app.tick().unwrap();
    assert_eq!(
        app.state(),
        ConnectionState::Idle,
        "Stop must not auto-restart"
    );
}

#[test]
fn a_successful_teardown_clears_the_marker() {
    let lab = Lab::new();
    let mut app = lab.app(Some(&lab.peer.public_key_b64()), None, 10, 0, true);

    app.stop().unwrap();
    assert_eq!(app.state(), ConnectionState::Idle);
    assert!(!lab.marker_exists(), "successful teardown left the marker");
}
