//! The password-prompt note is a published fact from `App`, not a guess from
//! the connection state. A per-user rule only counts when it names this action
//! and the user running the app.

use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::known::pair_record;
use jafnify_core::state::Role;
use jafnify_core::{ConnectionState, KeyPair};
use jafnify_linux::app::{App, AppBuilder, CONNECT_TIMEOUT};
use jafnify_linux::control::Snapshot;
use jafnify_linux::helper::LiveConfig;
use jafnify_linux::polkit;
use jafnify_linux::settings::{save_state, LinuxStateFile, Settings};
use jafnify_linux::view::{view, ViewInput};

static MULTICAST: Mutex<()> = Mutex::new(());

fn multicast_lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new(pkexec_exit: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let calls = dir.path().join("calls.log");
        let tail = if pkexec_exit == 0 {
            // The route verbs and the fixed route/teardown scripts are logged
            // only: a unit test must never run real ip, wg or resolvectl.
            "for a in \"$@\"; do case \"$a\" in *jafnify-route*|*route-fallback*|*route-restore*|*resolvectl*|*jafn0*fwmark*) exit 0;; esac; done\nexec \"$@\"\n".to_string()
        } else {
            format!("exit {pkexec_exit}\n")
        };
        script(
            dir.path(),
            "fake-pkexec.sh",
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n{tail}", calls.display()),
        );
        Self::finish(dir)
    }

    fn finish(dir: tempfile::TempDir) -> Self {
        script(dir.path(), "fake-wg.sh", "#!/bin/sh\nexit 1\n");
        script(dir.path(), "fake-wg-quick.sh", "#!/bin/sh\nexit 0\n");
        script(dir.path(), "fake-install.sh", "#!/bin/sh\nexit 0\n");
        // The action id is bound to the helper, so authorized apps need one.
        script(dir.path(), "fake-helper.sh", "#!/bin/sh\nexit 0\n");
        // Authorization is the test's own marker, never the host's polkit.
        script(
            dir.path(),
            "fake-pkcheck.sh",
            &format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\n[ -f \"{}\" ] && exit 0\nexit 1\n",
                dir.path().join("pkcheck.log").display(),
                dir.path().join("pkcheck-ok").display()
            ),
        );
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.path("calls.log")).unwrap_or_default()
    }

    fn pkcheck_calls(&self) -> String {
        std::fs::read_to_string(self.path("pkcheck.log")).unwrap_or_default()
    }

    /// Mark the fake pkcheck as authorized (exit 0).
    fn authorize(&self) {
        std::fs::write(self.path("pkcheck-ok"), b"1\n").unwrap();
    }

    fn set_rx(&self, n: u64) {
        std::fs::write(self.path("rx_bytes"), format!("{n}\n")).unwrap();
    }

    fn store_pair(&self, role: Role) {
        let peer = KeyPair::generate();
        let rec = pair_record(&KeyPair::generate(), &peer.public_key_b64(), role, None, 1).unwrap();
        save_state(
            &self.path("cfg/state.toml"),
            &LinuxStateFile {
                peers: vec![rec],
                settings: Settings {
                    role,
                    ..Settings::default()
                },
            },
        )
        .unwrap();
    }

    fn app(&self) -> App {
        AppBuilder::new(self.path("cfg"))
            .pkexec_bin(self.path("fake-pkexec.sh"))
            .pkcheck_bin(self.path("fake-pkcheck.sh"))
            .helper_bin(self.path("fake-helper.sh"))
            .wg_bin(self.path("fake-wg.sh"))
            .wg_quick_bin(self.path("fake-wg-quick.sh"))
            .install_bin(self.path("fake-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.path("rx_bytes"))
            .tx_bytes_path(self.path("tx_bytes"))
            .sessions_path(self.path("sessions.toml"))
            .polkit_rules_path(self.path("rules"))
            .installed_conf_path(self.path("jafn0.conf"))
            .build()
    }

    /// Like `app`, but with no helper: `pkexec /bin/sh` uses the generic exec
    /// action, which the Jafnify rule does not cover.
    fn app_without_helper(&self) -> App {
        AppBuilder::new(self.path("cfg"))
            .pkexec_bin(self.path("fake-pkexec.sh"))
            .pkcheck_bin(self.path("fake-pkcheck.sh"))
            .helper_bin(self.path("missing-helper"))
            .wg_bin(self.path("fake-wg.sh"))
            .wg_quick_bin(self.path("fake-wg-quick.sh"))
            .install_bin(self.path("fake-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.path("rx_bytes"))
            .tx_bytes_path(self.path("tx_bytes"))
            .sessions_path(self.path("sessions.toml"))
            .polkit_rules_path(self.path("rules"))
            .installed_conf_path(self.path("jafn0.conf"))
            .build()
    }

    fn app_with_helper(&self, live: &LiveConfig) -> App {
        let helper = self.path("fake-helper.sh");
        let live_path = self.path("live.toml");
        std::fs::write(&live_path, live.to_toml().unwrap()).unwrap();
        script(
            self.dir.path(),
            "fake-helper.sh",
            &format!(
                "#!/bin/sh\nif [ \"$1\" = inspect ]; then cat \"{}\"; exit 0; fi\nexit 0\n",
                live_path.display()
            ),
        );
        AppBuilder::new(self.path("cfg"))
            .pkexec_bin(self.path("fake-pkexec.sh"))
            .pkcheck_bin(self.path("fake-pkcheck.sh"))
            .helper_bin(helper)
            .wg_bin(self.path("fake-wg.sh"))
            .wg_quick_bin(self.path("fake-wg-quick.sh"))
            .install_bin(self.path("fake-install.sh"))
            .iface_pin(Ipv4Addr::LOCALHOST)
            .rx_bytes_path(self.path("rx_bytes"))
            .tx_bytes_path(self.path("tx_bytes"))
            .sessions_path(self.path("sessions.toml"))
            .polkit_rules_path(self.path("rules"))
            .installed_conf_path(self.path("jafn0.conf"))
            .build()
    }
}

fn current_user() -> String {
    polkit::current_user().expect("tests run as a login user")
}

#[test]
fn a_fresh_app_does_not_expect_a_prompt() {
    let lab = Lab::new(0);
    let app = lab.app();
    assert!(!app.polkit_prompt_expected());
    assert!(!ViewInput::from_app(&app).polkit_note);
}

#[test]
fn a_matching_rule_makes_the_privileged_line_prompt_free() {
    let lab = Lab::new(0);
    lab.set_rx(10);
    let user = current_user();
    polkit::install_at(&lab.path("rules"), &user).unwrap();
    lab.authorize();

    let mut app = lab.app();
    app.stop().unwrap();
    let log = app.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged down: ok (polkit rule)")),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|line| line.ends_with("privileged down: ok (password prompt)")),
        "{log:?}"
    );
    assert!(
        !app.polkit_prompt_expected(),
        "a covered call is prompt-free"
    );
}

#[test]
fn without_a_rule_the_privileged_line_says_password_prompt() {
    let lab = Lab::new(0);
    lab.set_rx(10);
    let mut app = lab.app();
    app.stop().unwrap();
    let log = app.connection_log();
    assert!(
        log.iter()
            .any(|line| line.ends_with("privileged down: ok (password prompt)")),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|line| line.ends_with("privileged down: ok (polkit rule)")),
        "{log:?}"
    );
}

#[test]
fn reuse_clears_the_attempt_hint() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.store_pair(Role::Sharer);

    let mut first = lab.app();
    first.start_known().unwrap();
    assert!(
        first.polkit_prompt_expected(),
        "the attempt publishes the hint before the up"
    );
    first.tick().unwrap();
    assert!(!first.polkit_prompt_expected(), "the up call clears it");
    let calls_after_up = lab.calls();
    assert_eq!(
        calls_after_up.lines().count(),
        1,
        "the first up is the only root call: {calls_after_up:?}"
    );
    drop(first);

    let mut second = lab.app();
    second.start_known().unwrap();
    assert!(
        second.polkit_prompt_expected(),
        "a new attempt arms the note again"
    );
    // The interface appears after the attempt starts, so the marker fast path
    // (not adoption's inspect) handles the reuse.
    lab.set_rx(0);
    second.tick().unwrap();
    assert_eq!(
        lab.calls(),
        calls_after_up,
        "reusing the marked tunnel ran another root command"
    );
    assert!(!second.polkit_prompt_expected(), "reuse clears the note");
}

#[test]
fn a_successful_adoption_clears_the_attempt_hint() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.store_pair(Role::Sharer);
    {
        let mut first = lab.app();
        first.start_known().unwrap();
        first.tick().unwrap();
    }
    let marker = std::fs::read_to_string(lab.path("cfg/tunnel.toml")).unwrap();
    let live: LiveConfig = toml::from_str(&marker).unwrap();
    lab.set_rx(1);

    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    assert_eq!(app.state(), ConnectionState::Connecting);
    assert!(!app.polkit_prompt_expected(), "adoption clears the hint");
}

#[test]
fn inspect_and_a_mismatched_adoption_leave_the_hint_clear() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.store_pair(Role::Sharer);
    lab.set_rx(5);
    let live = LiveConfig {
        peer_public_key: KeyPair::generate().public_key_b64(),
        endpoint: None,
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.7.0.2/32".to_string(),
        kill_switch: false,
    };
    let mut app = lab.app_with_helper(&live);
    app.start_known().unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "the search after a failed adoption is still an attempt"
    );
    app.tick().unwrap();
    assert!(
        lab.calls().contains("inspect"),
        "the adoption check must inspect: {:?}",
        lab.calls()
    );
    assert!(!app.polkit_prompt_expected(), "inspect/up clears the hint");
}

#[test]
fn a_paired_up_publishes_the_hint_then_clears_it_after_the_call() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.store_pair(Role::Sharer);
    let mut app = lab.app();
    app.start_known().unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "Searching must show the pending password prompt"
    );
    assert!(
        ViewInput::from_app(&app).polkit_note,
        "the view must publish the attempt hint"
    );
    app.tick().unwrap();
    assert!(!app.polkit_prompt_expected(), "the up call clears it");
    assert!(!ViewInput::from_app(&app).polkit_note);
    assert!(
        !app.connection_log()
            .iter()
            .any(|line| line.ends_with("privileged up: ok (polkit rule)")),
        "there was no covering rule"
    );
}

#[test]
fn a_code_attempt_publishes_the_hint_unless_pkcheck_allows() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    let mut app = lab.app();
    app.start("1234").unwrap();
    assert!(app.polkit_prompt_expected(), "exit 1 must show the note");
    assert!(ViewInput::from_app(&app).polkit_note);
    let calls = lab.pkcheck_calls();
    assert!(
        calls.contains("--action-id org.jafnify.tunnel"),
        "pkcheck must name the action: {calls:?}"
    );
    assert!(
        calls.contains("--process"),
        "pkcheck must name the process: {calls:?}"
    );
    assert!(
        !calls.contains("--allow-user-interaction"),
        "the hint check must not open a dialog: {calls:?}"
    );
    drop(app);

    lab.authorize();
    let mut covered = lab.app();
    covered.start("1234").unwrap();
    assert!(
        !covered.polkit_prompt_expected(),
        "exit 0 must hide the note"
    );
    assert!(!ViewInput::from_app(&covered).polkit_note);
}

#[test]
fn a_helper_less_fallback_never_uses_pkcheck_authorization() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.authorize();
    let mut app = lab.app_without_helper();
    app.start("1234").unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "pkexec /bin/sh is not covered by the helper-bound action"
    );
    assert!(ViewInput::from_app(&app).polkit_note);
    assert!(
        lab.pkcheck_calls().is_empty(),
        "pkcheck must not run without the helper: {:?}",
        lab.pkcheck_calls()
    );
}

#[test]
fn a_rule_file_does_not_hide_the_note_when_pkcheck_denies() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    let user = current_user();
    polkit::install_at(&lab.path("rules"), &user).unwrap();
    let mut app = lab.app();
    app.start("1234").unwrap();
    assert!(
        app.polkit_prompt_expected(),
        "the note follows pkcheck, not the rule file"
    );
}

#[test]
fn pkcheck_allows_the_attempt_even_without_a_readable_rule() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.authorize();
    lab.store_pair(Role::Sharer);
    let mut app = lab.app();
    app.start_known().unwrap();
    assert!(!app.polkit_prompt_expected());
    assert!(!ViewInput::from_app(&app).polkit_note);
}

#[test]
fn pkcheck_allows_the_attempt_for_a_stored_pair() {
    let _g = multicast_lock();
    let lab = Lab::new(0);
    lab.store_pair(Role::Sharer);
    polkit::install_at(&lab.path("rules"), &current_user()).unwrap();
    lab.authorize();
    let mut app = lab.app();
    app.start_known().unwrap();
    assert!(!app.polkit_prompt_expected());
    assert!(!ViewInput::from_app(&app).polkit_note);
}

#[test]
fn connected_and_retry_clear_the_attempt_hint() {
    let _g = multicast_lock();
    let lab = Lab::new(0);

    let mut app = lab.app();
    app.start("1234").unwrap();
    assert!(app.polkit_prompt_expected());
    app.accept_code("1234").unwrap();
    app.orchestrator_mut().confirm_connected();
    app.tick().unwrap();
    assert_eq!(app.state(), ConnectionState::Connected);
    assert!(!app.polkit_prompt_expected(), "Connected clears the hint");
    drop(app);

    let mut retry = lab.app();
    retry.start("1234").unwrap();
    retry.accept_code("1234").unwrap();
    let t0 = Instant::now();
    retry.tick_at(t0).unwrap();
    assert!(
        retry.polkit_prompt_expected(),
        "Connecting still expects a prompt"
    );
    retry
        .tick_at(t0 + CONNECT_TIMEOUT + Duration::from_secs(1))
        .unwrap();
    assert_eq!(retry.state(), ConnectionState::Retry);
    assert!(!retry.polkit_prompt_expected(), "Retry clears the hint");
}

#[test]
fn a_denied_up_leaves_the_hint_clear() {
    let _g = multicast_lock();
    let lab = Lab::new(126);
    lab.store_pair(Role::Sharer);
    let mut app = lab.app();
    app.start_known().unwrap();
    assert!(app.polkit_prompt_expected(), "the attempt was announced");
    app.tick().unwrap();
    assert!(!app.polkit_prompt_expected(), "a denial clears the hint");
    assert!(
        !app.connection_log()
            .iter()
            .any(|line| line.ends_with("privileged up: denied (polkit rule)")),
        "there was no covering rule"
    );
}

#[test]
fn a_denied_teardown_leaves_the_hint_clear() {
    let lab = Lab::new(126);
    lab.set_rx(10);
    let mut app = lab.app();
    assert!(app.stop().is_err());
    assert!(!app.polkit_prompt_expected());
}

#[test]
fn a_denied_forget_leaves_the_hint_clear() {
    let lab = Lab::new(126);
    lab.set_rx(10);
    let mut app = lab.app();
    assert!(app.forget().is_err());
    assert!(!app.polkit_prompt_expected());
}

#[test]
fn the_view_note_is_true_only_when_the_published_flag_is_true() {
    for state in [
        ConnectionState::Idle,
        ConnectionState::Searching,
        ConnectionState::Connected,
        ConnectionState::Retry,
    ] {
        let mut input = ViewInput::new(state);
        input.denied = state == ConnectionState::Idle;
        assert!(!view(&input).polkit_note, "{state:?}");
        input.polkit_note = true;
        assert!(view(&input).polkit_note, "{state:?}");
    }
}

#[test]
fn an_old_snapshot_without_the_note_still_loads() {
    let snapshot = Snapshot {
        polkit_note: true,
        ..Snapshot::default()
    };
    let mut value = serde_json::to_value(&snapshot).unwrap();
    value.as_object_mut().unwrap().remove("polkit_note");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(!old.polkit_note);
}
