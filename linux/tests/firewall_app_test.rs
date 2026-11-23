//! The App tells a blocked search from a quiet one and adds the narrow rule
//! only on request. Everything is fake: `ip` and `pkexec` are scripts, `ufw`
//! is a path that is never executed, sockets are pinned to loopback.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::pair_record;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, KeyPair};
use jafnify_linux::app::{App, AppBuilder, AppError};
use jafnify_linux::firewall::{FirewallKind, ECHO_GRACE};
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};

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
        let script = |name: &str, body: String| {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // The laptop of the real test: wlo1 10.20.89.70/24, default via the phone.
        script(
            "fake-ip.sh",
            r#"#!/bin/sh
case "$*" in
  "-4 -o route show default") echo "default via 10.20.89.13 dev wlo1 proto dhcp metric 600" ;;
  "-4 -o addr show dev wlo1") echo "3: wlo1    inet 10.20.89.70/24 brd 10.20.89.255 scope global dynamic noprefixroute wlo1" ;;
esac
"#
            .to_string(),
        );
        let log = dir.path().join("calls.log");
        script(
            "mock-pkexec.sh",
            format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );
        // polkit with the password dialog dismissed.
        script(
            "mock-pkexec-denied.sh",
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 126\n",
                log.display()
            ),
        );
        Self { dir }
    }

    /// A stored pair: the laptop starts on the known track, no code.
    fn store_pair(&self) {
        let rec = pair_record(
            &KeyPair::generate(),
            &KeyPair::generate().public_key_b64(),
            Role::Joiner,
            None,
            1,
        )
        .unwrap();
        save_state(
            &self.dir.path().join("cfg/state.toml"),
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
    }

    fn ufw_conf(&self, enabled: bool) -> PathBuf {
        let p = self.dir.path().join("ufw.conf");
        std::fs::write(
            &p,
            format!("ENABLED={}\n", if enabled { "yes" } else { "no" }),
        )
        .unwrap();
        p
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn builder(&self) -> AppBuilder {
        AppBuilder::new(self.dir.path().join("cfg"))
            .ip_bin(self.dir.path().join("fake-ip.sh"))
            .pkexec_bin(self.dir.path().join("mock-pkexec.sh"))
            .ufw_bin(PathBuf::from("/usr/sbin/ufw"))
            .wg_bin(PathBuf::from("/nonexistent/wg"))
            .iface_pin(Ipv4Addr::LOCALHOST)
    }

    fn app(&self, ufw: bool) -> App {
        self.builder().ufw_conf(self.ufw_conf(ufw)).build()
    }
}

fn late() -> Instant {
    Instant::now() + ECHO_GRACE + Duration::from_secs(1)
}

#[test]
fn adding_a_ufw_rule_does_not_clear_the_known_attempt_note() {
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.store_pair();
    let mut app = lab.app(true);
    app.start_known().unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "the known search is an uncovered attempt"
    );
    app.allow_firewall().unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "a ufw call is unrelated to the tunnel password note"
    );
}

#[test]
fn a_quiet_search_with_ufw_on_is_a_ufw_block_scoped_to_the_lan_interface() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(true);
    app.start("1234").unwrap();
    // Nothing was announced, so nothing can have come back.
    app.firewall_poll(late());
    assert_eq!(app.firewall_blocked(), Some(FirewallKind::Ufw));
    assert_eq!(app.firewall_iface(), Some("wlo1"));
    assert_eq!(app.lan_iface().as_deref(), Some("wlo1"));
}

#[test]
fn a_quiet_search_without_ufw_is_another_firewall() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(false);
    app.start("1234").unwrap();
    app.firewall_poll(late());
    assert_eq!(app.firewall_blocked(), Some(FirewallKind::Other));
}

#[test]
fn it_is_too_early_to_say_right_after_the_search_started() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(true);
    app.start("1234").unwrap();
    app.firewall_poll(Instant::now());
    assert_eq!(app.firewall_blocked(), None);
}

#[test]
fn our_own_announcements_coming_back_mean_there_is_no_block() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(true);
    app.start("1234").unwrap();
    app.tick().unwrap(); // announces the discovery token
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        app.firewall_poll(late());
        std::thread::sleep(Duration::from_millis(20));
        if app.firewall_blocked().is_none() {
            break;
        }
    }
    assert_eq!(
        app.firewall_blocked(),
        None,
        "our own frame never came back"
    );
}

#[test]
fn the_known_track_is_watched_too() {
    // Same check on both tracks: a paired laptop behind ufw stays on
    // "Searching" just the same.
    let _g = multicast_lock();
    let lab = Lab::new();
    lab.store_pair();
    let mut app = lab.app(true);
    app.start_known().unwrap();
    app.firewall_poll(late());
    assert_eq!(app.firewall_blocked(), Some(FirewallKind::Ufw));
}

#[test]
fn no_block_is_reported_once_the_search_is_over() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(true);
    app.start("1234").unwrap();
    app.firewall_poll(late());
    assert!(app.firewall_blocked().is_some());
    app.stop().unwrap();
    assert_eq!(app.state(), ConnectionState::Idle);
    assert_eq!(app.firewall_blocked(), None);
}

// ---- the narrow rule, only on request -----------------------------------------

#[test]
fn allowing_runs_exactly_one_interface_scoped_ufw_rule() {
    let lab = Lab::new();
    let mut app = lab.app(true);
    assert_eq!(app.allow_firewall().unwrap(), "wlo1");
    assert_eq!(app.firewall_iface(), None, "no block has been seen yet");
    let calls = lab.calls();
    assert!(calls.contains("/usr/sbin/ufw wlo1"), "{calls}");
    assert!(
        !calls.contains("from"),
        "the rule must not be subnet scoped: {calls}"
    );
    assert_eq!(
        calls.matches("/usr/sbin/ufw wlo1").count(),
        1,
        "exactly one privileged call: {calls}"
    );
}

#[test]
fn nothing_is_added_when_ufw_is_not_enabled() {
    let lab = Lab::new();
    let mut app = lab.app(false);
    let err = app.allow_firewall().unwrap_err();
    assert!(matches!(err, AppError::FirewallRule(_)));
    assert!(err.to_string().contains("ufw"));
    assert_eq!(lab.calls(), "", "a root command ran");
}

#[test]
fn nothing_is_added_without_a_lan_interface_to_scope_it() {
    let lab = Lab::new();
    let mut app = lab
        .builder()
        .ip_bin(PathBuf::from("/nonexistent/ip"))
        .ufw_conf(lab.ufw_conf(true))
        .build();
    let err = app.allow_firewall().unwrap_err();
    assert!(matches!(err, AppError::FirewallRule(_)));
    assert!(
        err.to_string()
            .contains("no LAN interface found to scope the rule to"),
        "{err}"
    );
    assert_eq!(lab.calls(), "");
}

#[test]
fn a_refused_password_prompt_adds_nothing_and_is_reported_as_such() {
    let lab = Lab::new();
    let mut app = lab
        .builder()
        .pkexec_bin(lab.dir.path().join("mock-pkexec-denied.sh"))
        .ufw_conf(lab.ufw_conf(true))
        .build();
    let err = app.allow_firewall().unwrap_err();
    assert!(matches!(err, AppError::PermissionDenied));
    assert_eq!(app.last_error(), Some("Permission was denied"));
}

#[test]
fn after_allowing_the_search_is_judged_afresh() {
    let _g = multicast_lock();
    let lab = Lab::new();
    let mut app = lab.app(true);
    app.start("1234").unwrap();
    app.firewall_poll(late());
    assert!(app.firewall_blocked().is_some());
    app.allow_firewall().unwrap();
    assert_eq!(
        app.firewall_blocked(),
        None,
        "still blamed on the old verdict"
    );
}
