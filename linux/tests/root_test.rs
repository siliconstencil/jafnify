// linux/tests/root_test.rs
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use jafnify_linux::helper::HandshakeAge;
use jafnify_linux::root::{package_owning, ufw_rule_script, RootCommands};

/// The guard tests change a process-wide variable; run them one at a time.
static ENV: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|e| e.into_inner())
}

struct NoRootEnv;

impl NoRootEnv {
    fn set() -> Self {
        std::env::set_var("JAFNIFY_TEST_NO_ROOT", "1");
        Self
    }
}

impl Drop for NoRootEnv {
    fn drop(&mut self) {
        std::env::remove_var("JAFNIFY_TEST_NO_ROOT");
    }
}

fn panic_text(err: Box<dyn std::any::Any + Send>) -> String {
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Write an executable test script from a child process. The test process
/// never holds the file open for writing, so a fork in another test thread
/// cannot inherit a write descriptor and make its exec fail with ETXTBSY
/// ("Text file busy").
fn write_exec(path: &Path, body: &str) {
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(body)
        .status()
        .unwrap();
    assert!(status.success(), "could not write {}", path.display());
}

fn stub_pkexec(dir: &Path) -> PathBuf {
    let p = dir.join("pkexec");
    write_exec(&p, "#!/bin/sh\nexit 0\n");
    p
}

fn root() -> RootCommands {
    RootCommands {
        pkexec_bin: PathBuf::from("/usr/bin/pkexec"),
        wg_quick_bin: PathBuf::from("/usr/bin/wg-quick"),
        wg_bin: PathBuf::from("/usr/bin/wg"),
        install_bin: PathBuf::from("/usr/bin/install"),
        ufw_bin: PathBuf::from("/usr/sbin/ufw"),
        helper_bin: PathBuf::from("/nonexistent/jafnify-helper"),
        route_bin: PathBuf::from("/nonexistent/jafnify-route"),
        interface: "jafn0".to_string(),
        teardown_conf: PathBuf::from("/tmp/jafnify-test/jafn0.conf"),
        teardown_iface: PathBuf::from("/tmp/jafnify-test/jafn0.iface"),
        teardown_rules: PathBuf::from("/tmp/jafnify-test/49-jafnify.rules"),
    }
}

#[test]
fn write_conf_command_shape() {
    let cmd =
        root().write_conf_command(PathBuf::from("/home/u/.config/jafnify/jafn0.conf").as_path());
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "/usr/bin/install".to_string(),
            "-m".to_string(),
            "600".to_string(),
            "/home/u/.config/jafnify/jafn0.conf".to_string(),
            "/tmp/jafnify-test/jafn0.conf".to_string(),
        ]
    );
    assert_eq!(cmd.get_program(), "/usr/bin/pkexec");
}

#[test]
fn up_command_shape() {
    let cmd = root().wg_quick_up_command();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "/usr/bin/wg-quick".to_string(),
            "up".to_string(),
            "jafn0".to_string()
        ]
    );
}

// The down step also removes /tmp/jafnify-test/jafn0.conf (it holds the private
// key) in the same privileged call.
#[test]
fn down_command_takes_the_tunnel_down_and_removes_the_installed_config() {
    let cmd = root().wg_quick_down_command();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "/bin/sh");
    assert!(args[2].contains("ip link del dev jafn0"), "{}", args[2]);
    assert!(args[2].contains("jafn0 is still present"), "{}", args[2]);
    assert_eq!(args[3], "sh");
    assert_eq!(args[4], "/tmp/jafnify-test/jafn0.conf");
    assert_eq!(args[5], "/tmp/jafnify-test/jafn0.iface");
    assert_eq!(args[6], "");
}

#[test]
fn remove_config_command_deletes_only_the_installed_config() {
    let args: Vec<String> = root()
        .remove_conf_command()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args.last().unwrap(), "/tmp/jafnify-test/jafn0.conf");
    assert!(args.contains(&"rm -f \"$1\"".to_string()), "{args:?}");
}

#[test]
fn turning_the_kill_switch_off_runs_its_removal_as_root() {
    let cmd = root().kill_switch_down_command(false);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "/bin/sh");
    assert!(args[2].contains("jafnify_killswitch"), "{args:?}");
}

#[test]
fn available_checks_pkexec_on_path() {
    // /usr/bin/pkexec must exist on any system with polkit; if this
    // assertion is false the CI machine lacks polkit and the availability
    // check itself is what fails.
    assert_eq!(
        root().is_available(),
        PathBuf::from("/usr/bin/pkexec").exists()
    );
}

#[test]
fn the_test_guard_refuses_the_real_pkexec_even_when_a_test_binary_asks() {
    let _g = env_lock();
    let _env = NoRootEnv::set();
    let err = std::panic::catch_unwind(|| {
        let root = RootCommands::new("jafn0");
        let _ = root.run(root.wg_quick_up_command());
    })
    .expect_err("the env guard must refuse the real pkexec");
    let msg = panic_text(err);
    assert!(msg.contains("refusing to spawn"), "{msg}");
    assert!(msg.contains("JAFNIFY_TEST_NO_ROOT"), "{msg}");
}

#[test]
fn the_test_guard_allows_a_stub_pkexec() {
    let _g = env_lock();
    let _env = NoRootEnv::set();
    let dir = tempfile::tempdir().unwrap();
    let mut root = RootCommands::new("jafn0");
    root.pkexec_bin = stub_pkexec(dir.path());
    let out = root
        .run(root.wg_quick_up_command())
        .expect("a stub under the temp dir must be allowed");
    assert!(out.status.success());
}

#[test]
fn connect_without_the_helper_is_one_combined_call() {
    let mut root = root();
    root.helper_bin = PathBuf::from("/nonexistent/jafnify-helper");
    let (cmd, input) = root.connect_command(Path::new("/tmp/jafn0.conf"), "request");
    assert!(input.is_none());
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "/bin/sh");
    assert!(args[2].contains("-m 600"), "{args:?}");
    assert!(args[2].contains("&&"), "one call, not two: {args:?}");
    assert!(args[2].contains("up"), "{args:?}");
}

#[test]
fn connect_with_the_helper_sends_the_request_on_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("jafnify-helper");
    std::fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();
    let mut root = root();
    root.helper_bin = helper.clone();
    let (cmd, input) = root.connect_command(Path::new("/tmp/jafn0.conf"), "role = \"joiner\"");
    assert_eq!(input.as_deref(), Some(b"role = \"joiner\"".as_slice()));
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], helper.to_string_lossy());
    assert_eq!(args[1], "up");
}

#[test]
fn install_helper_chains_steps_and_installs_both_binaries_to_usr_local_bin() {
    let root = root();
    let cmd = root.install_helper_command(jafnify_linux::root::InstallPaths {
        jafnify: Path::new("/tmp/jafnify"),
        gui: Path::new("/tmp/jafnify-gui"),
        helper: Path::new("/tmp/jafnify-helper"),
        route: Path::new("/tmp/jafnify-route"),
        policy: Path::new("/tmp/org.jafnify.policy"),
        desktop: Path::new("/tmp/jafnify.desktop"),
        icon: Path::new("/tmp/jafnify.svg"),
        symbolic: Path::new("/tmp/jafnify-symbolic.svg"),
        rules: Some(Path::new("/tmp/49-jafnify.rules")),
    });
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let script = &args[2];
    assert!(script.contains(" && "), "no partial install: {script}");
    assert!(
        script.contains("failed at"),
        "name the failed step: {script}"
    );
    assert!(script.contains("install jafnify"), "{script}");
    assert!(script.contains("install jafnify-gui"), "{script}");
    assert!(script.contains("install route helper"), "{script}");
    assert!(script.contains("install polkit rule"), "{script}");
    // The rule is the adoption consent: install only replaces an existing one.
    assert!(
        script.contains("[ -e /etc/polkit-1/rules.d/49-jafnify.rules ]"),
        "install must never create the rule: {script}"
    );
    assert!(
        script.contains("/etc/polkit-1/rules.d/49-jafnify.rules"),
        "{script}"
    );
    assert!(
        script.contains("/usr/libexec/jafnify/jafnify-route"),
        "{script}"
    );
    assert!(script.contains("/usr/local/bin/jafnify"), "{script}");
    assert!(script.contains("/usr/local/bin/jafnify-gui"), "{script}");
    assert!(
        !script.contains("; install"),
        "steps must chain with &&: {script}"
    );
    let syntax = std::process::Command::new("/bin/sh")
        .args(["-n", "-c", script])
        .output()
        .unwrap();
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
}

// ---- the fixed ufw add/cleanup script, against a fake ufw ---------------------

const IFACE_RULE: &str =
    "[ 2] 9799/udp on wlo1           ALLOW IN    Anywhere                   # Jafnify discovery";
const OLD_GROUP_RULE: &str =
    "[ 3] 239.255.42.99 9799/udp    ALLOW IN    Anywhere on wlo1    # Jafnify discovery";
const OLD_SUBNET_RULE: &str =
    "[ 1] 239.255.42.99 9799/udp    ALLOW IN    10.20.89.0/24     # Jafnify discovery";

struct FakeUfw {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    status: PathBuf,
    after: PathBuf,
}

impl FakeUfw {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("ufw");
        let log = dir.path().join("calls.log");
        let status = dir.path().join("status.txt");
        let after = dir.path().join("after.txt");
        write_exec(
            &bin,
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> \"$FAKE_UFW_LOG\"\n\
             if [ \"$1\" = status ]; then cat \"$FAKE_UFW_STATUS\"; fi\n\
             if [ \"$1\" = allow ] && [ \"${FAKE_UFW_ALLOW_FAIL:-0}\" = 1 ]; then exit 1; fi\n\
             if [ \"$1\" = allow ] && [ -f \"$FAKE_UFW_AFTER\" ]; then cp \"$FAKE_UFW_AFTER\" \"$FAKE_UFW_STATUS\"; fi\n\
             exit 0\n",
        );
        Self {
            _dir: dir,
            bin,
            log,
            status,
            after,
        }
    }

    fn status(&self, text: &str) {
        std::fs::write(&self.status, text).unwrap();
    }

    /// The table `ufw status numbered` shows once the allow has run.
    fn after_allow(&self, text: &str) {
        std::fs::write(&self.after, text).unwrap();
    }

    fn run(&self, allow_fails: bool) -> std::process::Output {
        std::process::Command::new("/bin/sh")
            .args(["-c", ufw_rule_script(), "sh"])
            .arg(&self.bin)
            .args(["wlo1", "9799", "Jafnify discovery", "239.255.42.99"])
            .env("FAKE_UFW_LOG", &self.log)
            .env("FAKE_UFW_STATUS", &self.status)
            .env("FAKE_UFW_AFTER", &self.after)
            .env("FAKE_UFW_ALLOW_FAIL", if allow_fails { "1" } else { "0" })
            .output()
            .unwrap()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn an_existing_interface_rule_is_kept_and_only_the_stale_subnet_rule_is_deleted() {
    let ufw = FakeUfw::new();
    ufw.status(&format!(
        "Status: active\n\n{OLD_SUBNET_RULE}\n{IFACE_RULE}\n"
    ));
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        ufw.calls(),
        vec![
            "status numbered".to_string(),
            "--force delete 1".to_string()
        ],
        "the interface rule must not be added again or deleted"
    );
}

#[test]
fn an_old_subnet_rule_is_replaced_by_the_interface_rule() {
    let ufw = FakeUfw::new();
    ufw.status(&format!("Status: active\n\n{OLD_SUBNET_RULE}\n"));
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        ufw.calls(),
        vec![
            "status numbered".to_string(),
            "allow in on wlo1 to any port 9799 proto udp comment Jafnify discovery".to_string(),
            "status numbered".to_string(),
            "--force delete 1".to_string(),
        ]
    );
}

// K2: the old group-only rule silenced the unicast copies, so it is replaced
// and the new port rule is added.
#[test]
fn an_old_group_only_rule_is_replaced_by_the_port_rule() {
    let ufw = FakeUfw::new();
    ufw.status(&format!("Status: active\n\n{OLD_GROUP_RULE}\n"));
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        ufw.calls(),
        vec![
            "status numbered".to_string(),
            "allow in on wlo1 to any port 9799 proto udp comment Jafnify discovery".to_string(),
            "status numbered".to_string(),
            "--force delete 3".to_string(),
        ]
    );
}

#[test]
fn with_no_jafnify_rule_the_interface_rule_is_added_and_nothing_is_deleted() {
    let ufw = FakeUfw::new();
    ufw.status("Status: active\n\n     To                         Action      From\n");
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        ufw.calls(),
        vec![
            "status numbered".to_string(),
            "allow in on wlo1 to any port 9799 proto udp comment Jafnify discovery".to_string(),
            "status numbered".to_string(),
        ]
    );
}

#[test]
fn an_already_interface_scoped_jafnify_rule_needs_no_change() {
    let ufw = FakeUfw::new();
    ufw.status(&format!("Status: active\n\n{IFACE_RULE}\n"));
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(ufw.calls(), vec!["status numbered".to_string()]);
}

#[test]
fn a_failed_add_stops_before_any_stale_rule_is_deleted() {
    let ufw = FakeUfw::new();
    ufw.status(&format!("Status: active\n\n{OLD_SUBNET_RULE}\n"));
    let out = ufw.run(true);
    assert!(
        !out.status.success(),
        "the add failure must stop the script"
    );
    let calls = ufw.calls();
    assert_eq!(calls.first().map(String::as_str), Some("status numbered"));
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1].starts_with("allow "), "{calls:?}");
    assert!(!calls.iter().any(|c| c.contains("delete ")), "{calls:?}");
}

#[test]
fn a_stale_rule_at_a_higher_number_is_deleted_first() {
    let ufw = FakeUfw::new();
    ufw.status(&format!(
        "Status: active\n\n{IFACE_RULE}\n{OLD_SUBNET_RULE}\n[ 4] 239.255.42.99 9799/udp    ALLOW IN    192.168.0.0/24     # Jafnify discovery\n"
    ));
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    let calls = ufw.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.contains("delete "))
            .collect::<Vec<_>>(),
        vec!["--force delete 4", "--force delete 1"],
        "{calls:?}"
    );
}

#[test]
fn the_desktop_entry_uses_an_absolute_installed_binary() {
    let desktop = include_str!("../../assets/linux/jafnify.desktop");
    assert!(
        desktop.contains("Exec=/usr/local/bin/jafnify-gui"),
        "{desktop}"
    );
}

#[test]
fn forget_is_one_call_that_removes_the_tunnel_config_and_rules() {
    let cmd = root().forget_command();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "/bin/sh");
    let script = &args[2];
    assert!(script.contains("ip link del dev jafn0"), "{script}");
    assert!(script.contains("rm -f -- \"$3\""), "{script}");
    assert!(script.contains("jafn0 is still present"), "{script}");
    assert_eq!(args[4], "/tmp/jafnify-test/jafn0.conf");
    assert_eq!(args[5], "/tmp/jafnify-test/jafn0.iface");
    assert_eq!(args[6], "/tmp/jafnify-test/49-jafnify.rules");
    let syntax = std::process::Command::new("/bin/sh")
        .args(["-n", "-c", script])
        .output()
        .unwrap();
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
}

#[test]
fn forget_uses_the_helper_verb_when_the_helper_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("jafnify-helper");
    write_exec(&helper, "#!/bin/sh\nexit 0\n");
    let mut root = root();
    root.helper_bin = helper.clone();
    let args: Vec<String> = root
        .forget_command()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![helper.to_string_lossy().into_owned(), "forget".to_string()]
    );
}

#[test]
fn inspect_and_endpoint_are_fixed_helper_verbs() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("jafnify-helper");
    write_exec(&helper, "#!/bin/sh\nexit 0\n");
    let mut root = root();
    root.helper_bin = helper.clone();
    for (cmd, verb) in [
        (root.inspect_command(), "inspect"),
        (root.endpoint_command(), "endpoint"),
    ] {
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![helper.to_string_lossy().into_owned(), verb.to_string()]
        );
    }
}

#[test]
fn the_handshake_age_is_a_fixed_read_only_helper_verb() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("jafnify-helper");
    write_exec(&helper, "#!/bin/sh\nprintf '42\\n'\n");
    let pkexec = dir.path().join("pkexec");
    write_exec(&pkexec, "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in *jafnify-route*|*route-fallback*|*route-restore*|*resolvectl*|*jafn0*fwmark*) exit 0;; esac; done\nexec \"$@\"\n");

    let mut root = root();
    root.helper_bin = helper.clone();
    root.pkexec_bin = pkexec;
    let args: Vec<String> = root
        .handshake_age_command()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            helper.to_string_lossy().into_owned(),
            "handshake-age".to_string()
        ]
    );
    assert_eq!(root.handshake_age(), HandshakeAge::Seconds(42));
}

#[test]
fn the_suppress_command_is_the_fixed_helper_verb() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("jafnify-helper");
    write_exec(&helper, "#!/bin/sh\nexit 0\n");
    let mut root = root();
    root.helper_bin = helper.clone();
    let cmd = root.suppress_command();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            helper.to_string_lossy().into_owned(),
            "suppress".to_string()
        ]
    );
    assert_eq!(cmd.get_program(), "/usr/bin/pkexec");
}

#[test]
fn uninstall_removes_only_fixed_paths_and_is_valid_sh() {
    let cmd = root().uninstall_command();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let script = &args[2];
    for path in [
        "/usr/local/bin/jafnify",
        "/usr/local/bin/jafnify-gui",
        "/usr/libexec/jafnify/jafnify-helper",
        "/usr/libexec/jafnify/jafnify-route",
        "/usr/share/polkit-1/actions/org.jafnify.policy",
        "/etc/polkit-1/rules.d/49-jafnify.rules",
        "/usr/share/applications/jafnify.desktop",
    ] {
        assert!(script.contains(path), "uninstall misses {path}: {script}");
    }
    assert!(
        script.contains("forget"),
        "uninstall must stop the tunnel: {script}"
    );
    assert!(
        script.contains("rmdir /usr/libexec/jafnify"),
        "uninstall must remove its now-empty libexec directory: {script}"
    );
    let syntax = std::process::Command::new("/bin/sh")
        .args(["-n", "-c", script])
        .output()
        .unwrap();
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
}

fn stub_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    write_exec(&p, body);
    p
}

#[test]
fn a_dpkg_list_file_marks_the_install_as_packaged() {
    let dir = tempfile::tempdir().unwrap();
    let info = dir.path().join("info");
    std::fs::create_dir(&info).unwrap();
    std::fs::write(info.join("jafnify.list"), "/usr/bin/jafnify\n").unwrap();
    let dpkg = stub_script(dir.path(), "dpkg-query", "#!/bin/sh\nexit 1\n");
    assert_eq!(
        package_owning(
            Path::new("/usr/libexec/jafnify/jafnify-helper"),
            &info,
            "jafnify",
            &dpkg
        )
        .as_deref(),
        Some("jafnify")
    );
}

#[test]
fn dpkg_query_owner_marks_the_install_as_packaged() {
    let dir = tempfile::tempdir().unwrap();
    let info = dir.path().join("info");
    let dpkg = stub_script(
        dir.path(),
        "dpkg-query",
        "#!/bin/sh\nprintf 'jafnify: /usr/libexec/jafnify/jafnify-helper\\n'\n",
    );
    assert_eq!(
        package_owning(
            Path::new("/usr/libexec/jafnify/jafnify-helper"),
            &info,
            "jafnify",
            &dpkg
        )
        .as_deref(),
        Some("jafnify")
    );
}

#[test]
fn a_tarball_install_is_not_packaged() {
    let dir = tempfile::tempdir().unwrap();
    let info = dir.path().join("info");
    let dpkg = stub_script(dir.path(), "dpkg-query", "#!/bin/sh\nexit 1\n");
    assert_eq!(
        package_owning(
            Path::new("/usr/libexec/jafnify/jafnify-helper"),
            &info,
            "jafnify",
            &dpkg
        ),
        None
    );
}

#[test]
fn the_test_flavor_uses_its_own_package_name() {
    let dir = tempfile::tempdir().unwrap();
    let info = dir.path().join("info");
    std::fs::create_dir(&info).unwrap();
    std::fs::write(info.join("jafnify-test.list"), "/usr/bin/jafnify-test\n").unwrap();
    let dpkg = stub_script(dir.path(), "dpkg-query", "#!/bin/sh\nexit 1\n");
    assert_eq!(
        package_owning(
            Path::new("/usr/libexec/jafnify-test/jafnify-test-helper"),
            &info,
            "jafnify-test",
            &dpkg
        )
        .as_deref(),
        Some("jafnify-test")
    );
}

#[test]
fn the_fixed_route_scripts_are_scoped_to_jafn0s_fwmark_rule() {
    let root = root();
    for (cmd, verb) in [
        (root.route_fallback_command(), "del"),
        (root.route_restore_command(), "add"),
    ] {
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let script = &args[2];
        assert!(script.contains("wg show jafn0 fwmark"), "{script}");
        assert!(script.contains("not from all fwmark"), "{script}");
        assert!(script.contains(verb), "{script}");
        if verb == "del" {
            // The fixed fallback must refuse while the kill switch owns the
            // packet flow, exactly like the route binary.
            assert!(script.contains("JAFNIFY_KS"), "{script}");
            assert!(script.contains("jafnify_killswitch"), "{script}");
            assert!(script.contains("exit 1"), "{script}");
        }
        assert!(
            !script.contains("priority 100"),
            "the global main-table rule must not come back: {script}"
        );
        assert!(
            !script.contains("table main"),
            "another VPN's routing must stay untouched: {script}"
        );
    }
}

#[test]
fn the_route_binary_is_preferred_when_installed() {
    let _g = env_lock();
    let _env = NoRootEnv::set();
    let dir = tempfile::tempdir().unwrap();
    let route = dir.path().join("jafnify-route");
    write_exec(&route, "#!/bin/sh\nexit 0\n");
    let mut root = root();
    root.route_bin = route.clone();
    for (cmd, verb) in [
        (root.route_fallback_command(), "route-fallback"),
        (root.route_restore_command(), "route-restore"),
    ] {
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![route.to_string_lossy().into_owned(), verb.to_string()]
        );
    }
}

// ufw puts an added IPv4 rule after the other IPv4 rules, so every IPv6 rule
// moves down one number. Deleting by the numbers read before the add would hit
// the rule above the stale one: here the user's own IPv6 rule.
#[test]
fn stale_rules_are_deleted_by_their_numbers_after_the_add() {
    let ufw = FakeUfw::new();
    ufw.status(
        "Status: active\n\n\
         [ 1] 22/tcp                     ALLOW IN    Anywhere\n\
         [ 2] 9799/udp on wlo2           ALLOW IN    Anywhere                   # Jafnify discovery\n\
         [ 3] 22/tcp (v6)                ALLOW IN    Anywhere (v6)\n\
         [ 4] 9799/udp (v6) on wlo2      ALLOW IN    Anywhere (v6)              # Jafnify discovery\n",
    );
    ufw.after_allow(
        "Status: active\n\n\
         [ 1] 22/tcp                     ALLOW IN    Anywhere\n\
         [ 2] 9799/udp on wlo2           ALLOW IN    Anywhere                   # Jafnify discovery\n\
         [ 3] 9799/udp on wlo1           ALLOW IN    Anywhere                   # Jafnify discovery\n\
         [ 4] 22/tcp (v6)                ALLOW IN    Anywhere (v6)\n\
         [ 5] 9799/udp (v6) on wlo2      ALLOW IN    Anywhere (v6)              # Jafnify discovery\n\
         [ 6] 9799/udp (v6) on wlo1      ALLOW IN    Anywhere (v6)              # Jafnify discovery\n",
    );
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    let calls = ufw.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.contains("delete "))
            .collect::<Vec<_>>(),
        vec!["--force delete 5", "--force delete 2"],
        "only the wlo2 rules go, the user's 22/tcp (v6) and the new wlo1 pair stay: {calls:?}"
    );
}

// The IPv6 twin of the wanted rule is part of it, not a stale rule.
#[test]
fn the_wanted_rules_ipv6_twin_is_kept() {
    let ufw = FakeUfw::new();
    ufw.status(
        "Status: active\n\n\
         [ 1] 9799/udp on wlo1           ALLOW IN    Anywhere                   # Jafnify discovery\n\
         [ 2] 9799/udp (v6) on wlo1      ALLOW IN    Anywhere (v6)              # Jafnify discovery\n",
    );
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(ufw.calls(), vec!["status numbered".to_string()]);
}

// wlo1 must not count as wlo10 (or port 9799 as 19799).
#[test]
fn a_rule_on_a_longer_interface_name_is_not_the_wanted_rule() {
    let ufw = FakeUfw::new();
    ufw.status(
        "Status: active\n\n\
         [ 1] 9799/udp on wlo10          ALLOW IN    Anywhere                   # Jafnify discovery\n",
    );
    let out = ufw.run(false);
    assert!(out.status.success(), "{out:?}");
    let calls = ufw.calls();
    assert!(calls.iter().any(|c| c.starts_with("allow ")), "{calls:?}");
    assert!(calls.contains(&"--force delete 1".to_string()), "{calls:?}");
}
