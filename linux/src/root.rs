//! pkexec (polkit) root escalation. Commands are built as values and
//! executed by the caller; the GUI shows the polkit dialog, which
//! caches the password for the session.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use crate::exec;
use crate::helper::{parse_handshake_age, HandshakeAge};

/// Set by test processes: no test may spawn a real privileged tool.
pub const TEST_NO_ROOT_ENV: &str = "JAFNIFY_TEST_NO_ROOT";

/// Installed by `jafnify install-helper`; the polkit action is bound to it.
pub const HELPER_PATH: &str = crate::flavor::HELPER_PATH;

/// The small password-free route binary (`org.jafnify.route`).
pub const ROUTE_PATH: &str = crate::flavor::ROUTE_PATH;

/// The package that owns `helper`, if a package manager installed it. A
/// package manager owns its files: `jafnify uninstall` must refuse instead of
/// deleting them and leaving dpkg inconsistent. The dpkg list file is checked
/// first (no command needed), then `dpkg-query -S`.
pub fn package_owning(
    helper: &Path,
    info_dir: &Path,
    package: &str,
    dpkg_query: &Path,
) -> Option<String> {
    if info_dir.join(format!("{package}.list")).is_file() {
        return Some(package.to_string());
    }
    let mut cmd = Command::new(dpkg_query);
    cmd.arg("-S").arg(helper);
    let out = exec::output(cmd).ok()?;
    if !out.status.success() {
        return None;
    }
    let helper = helper.to_string_lossy();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let (names, path) = line.split_once(": ")?;
            if path.trim() != helper.as_ref() {
                return None;
            }
            let name = names.split(',').next().unwrap_or("").trim();
            (!name.is_empty()).then(|| name.to_string())
        })
}

/// No route binary: fixed mark-scoped scripts (no user input). They delete or
/// re-add only jafn0's own wg-quick rule, never a shared main-table rule, so
/// another full-tunnel VPN keeps its routing.
const ROUTE_FALLBACK_SCRIPT: &str = "if iptables -S JAFNIFY_KS >/dev/null 2>&1 \
     || nft list table inet jafnify_killswitch >/dev/null 2>&1; then \
         exit 1; \
     fi; \
     mark=$(wg show jafn0 fwmark 2>/dev/null || true); \
     if [ -n \"$mark\" ] && [ \"$mark\" != \"off\" ]; then \
         mark=${mark%%/*}; table=$((mark)); \
         ip -4 rule del not from all fwmark \"$mark\" lookup \"$table\" 2>/dev/null || true; \
     fi; \
     ip -6 route del blackhole ::/0 metric 1 2>/dev/null || true; \
     command -v resolvectl >/dev/null 2>&1 && resolvectl revert jafn0 2>/dev/null || true";
const ROUTE_RESTORE_SCRIPT: &str = "mark=$(wg show jafn0 fwmark 2>/dev/null || true); \
     if [ -n \"$mark\" ] && [ \"$mark\" != \"off\" ]; then \
         mark=${mark%%/*}; table=$((mark)); \
         if ! ip -4 rule show 2>/dev/null | grep -q \"fwmark $mark\"; then \
             pref=$(ip -4 rule show 2>/dev/null \
                 | sed -n 's/^\\([0-9]*\\):.*lookup main suppress_prefixlength 0.*/\\1/p' \
                 | sort -n | head -n 1); \
             pref=$(( ${pref:-32764} + 1 )); \
             ip -4 rule add priority \"$pref\" not from all fwmark \"$mark\" lookup \"$table\" \
                 || exit $?; \
         fi; \
     fi; \
     ip -6 route replace blackhole ::/0 metric 1 2>/dev/null || true; \
     if command -v resolvectl >/dev/null 2>&1; then \
         resolvectl dns jafn0 10.233.0.1 2>/dev/null || true; \
         resolvectl domain jafn0 ~. 2>/dev/null || true; \
         resolvectl default-route jafn0 yes 2>/dev/null || true; \
     fi";

/// The fixed privileged script that adds the interface+port discovery rule if
/// it is missing, then removes every older Jafnify discovery rule (the old
/// group-only rule and the older subnet-scoped ones), on any interface. Values
/// are passed as arguments (`$1` ufw, `$2` interface, `$3` port, `$4` comment,
/// `$5` multicast group used only to recognise the old rule), never spliced
/// into the code. `set -e` stops before cleanup when the add fails.
pub fn ufw_rule_script() -> &'static str {
    r#"set -eu
u=$1; iface=$2; port=$3; comment=$4; group=$5
# Reads `ufw status numbered` on stdin and prints the Jafnify rule lines that
# are the wanted rule (IPv4 or its IPv6 twin) when $1 is keep, and every other
# Jafnify rule line when $1 is stale. Variables are matched literally.
pick() {
  while IFS= read -r l; do
    case "$l" in *"$comment"*) ;; *) continue ;; esac
    case "$l" in
      *"$group"*) k=stale ;;
      *" $port/udp on $iface "*|*" $port/udp (v6) on $iface "*) k=keep ;;
      *) k=stale ;;
    esac
    if [ "$k" = "$1" ]; then printf '%s\n' "$l"; fi
  done
  return 0
}
status=$("$u" status numbered)
if [ -z "$(printf '%s\n' "$status" | pick keep)" ]; then
  "$u" allow in on "$iface" to any port "$port" proto udp comment "$comment"
  # An added rule renumbers the table (every IPv6 rule moves down), so read
  # the table again: delete by the numbers it shows now.
  status=$("$u" status numbered)
fi
printf '%s\n' "$status" | pick stale \
  | sed -n 's/^\[ *\([0-9][0-9]*\)\].*/\1/p' | sort -rn \
  | while read -r n; do "$u" --force delete "$n"; done
"#
}

#[derive(Debug, Clone)]
pub struct RootCommands {
    pub pkexec_bin: PathBuf,
    pub wg_quick_bin: PathBuf,
    pub wg_bin: PathBuf,
    pub install_bin: PathBuf,
    pub ufw_bin: PathBuf,
    pub helper_bin: PathBuf,
    pub route_bin: PathBuf,
    pub interface: String,
    pub teardown_conf: PathBuf,
    pub teardown_iface: PathBuf,
    pub teardown_rules: PathBuf,
}

/// The files one `install-helper` call installs, in script argument order.
pub struct InstallPaths<'a> {
    pub jafnify: &'a Path,
    pub gui: &'a Path,
    pub helper: &'a Path,
    pub route: &'a Path,
    pub policy: &'a Path,
    pub desktop: &'a Path,
    pub icon: &'a Path,
    pub symbolic: &'a Path,
    pub rules: Option<&'a Path>,
}

impl RootCommands {
    pub fn new(interface: &str) -> Self {
        Self {
            pkexec_bin: PathBuf::from("pkexec"),
            wg_quick_bin: PathBuf::from("wg-quick"),
            wg_bin: PathBuf::from("wg"),
            install_bin: PathBuf::from("install"),
            ufw_bin: PathBuf::from("ufw"),
            helper_bin: PathBuf::from(HELPER_PATH),
            route_bin: PathBuf::from(ROUTE_PATH),
            interface: interface.to_string(),
            teardown_conf: PathBuf::from(format!("/etc/wireguard/{interface}.conf")),
            teardown_iface: PathBuf::from(format!("/sys/class/net/{interface}")),
            teardown_rules: PathBuf::from(crate::polkit::RULES_PATH),
        }
    }

    pub fn has_helper(&self) -> bool {
        if !self.helper_bin.is_file() {
            return false;
        }
        // A test must not silently pick up a helper installed on the operator's
        // machine: only an explicit temp stub counts.
        if test_guard_active() {
            return is_temp_stub(&self.helper_bin);
        }
        true
    }

    /// The password-free route binary (`org.jafnify.route`). Same guard rule.
    pub fn has_route_binary(&self) -> bool {
        if !self.route_bin.is_file() {
            return false;
        }
        if test_guard_active() {
            return is_temp_stub(&self.route_bin);
        }
        true
    }

    /// One privileged step: helper `up` (request on stdin), or one combined
    /// install + wg-quick up when the helper is not installed.
    pub fn connect_command(&self, source: &Path, request: &str) -> (Command, Option<Vec<u8>>) {
        if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("up");
            (cmd, Some(request.as_bytes().to_vec()))
        } else {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg("/bin/sh")
                .arg("-c")
                .arg("\"$5\" -m 600 \"$1\" \"$3\" && \"$2\" up \"$4\"")
                .arg("sh")
                .arg(source)
                .arg(&self.wg_quick_bin)
                .arg(&self.teardown_conf)
                .arg(&self.interface)
                .arg(&self.install_bin);
            (cmd, None)
        }
    }

    /// helper `down`, or the combined down + rm fallback.
    pub fn down_command(&self) -> Command {
        if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("down");
            cmd
        } else {
            self.wg_quick_down_command()
        }
    }

    /// helper `down` also removes a leftover config; otherwise rm alone.
    pub fn remove_conf_command_fixed(&self) -> Command {
        if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("down");
            cmd
        } else {
            self.remove_conf_command()
        }
    }

    /// Read-only status for adopting a tunnel; the helper path avoids a
    /// generic `wg` grant.
    pub fn status_command(&self) -> Command {
        if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("status");
            cmd
        } else {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.wg_bin)
                .arg("show")
                .arg(&self.interface)
                .arg("peers");
            cmd
        }
    }

    /// Read-only live peer fields (pubkey, endpoint, allowed IPs) plus address
    /// and kill-switch state. No private key is printed.
    pub fn inspect_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.helper_bin).arg("inspect");
        cmd
    }

    /// Update only the existing peer's endpoint. The endpoint is the stdin
    /// payload; the helper validates it and takes the peer key from the
    /// installed config.
    pub fn endpoint_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.helper_bin).arg("endpoint");
        cmd
    }

    /// Re-add the shared main-table suppress rule. Fixed helper verb only.
    pub fn suppress_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.helper_bin).arg("suppress");
        cmd
    }

    /// Read-only handshake age through the helper. Fixed verb, no arguments.
    pub fn handshake_age_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.helper_bin).arg("handshake-age");
        cmd
    }

    /// The peer's newest handshake age. A missing helper, a failed call or an
    /// unreadable answer is `Unknown`; nothing is written to the interface.
    pub fn handshake_age(&self) -> HandshakeAge {
        if !self.has_helper() {
            return HandshakeAge::Unknown;
        }
        let out = match self.run(self.handshake_age_command()) {
            Ok(out) if out.status.success() => out,
            _ => return HandshakeAge::Unknown,
        };
        parse_handshake_age(&String::from_utf8_lossy(&out.stdout))
    }

    /// One call that stops the tunnel, removes the installed config and removes
    /// the per-user polkit rule. Forget may prompt once; a plain stop does not
    /// remove the rule.
    pub fn forget_command(&self) -> Command {
        if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("forget");
            return cmd;
        }
        self.fixed_teardown_command(true)
    }

    /// One fixed uninstall script. It stops the tunnel through the helper when
    /// present, then removes only Jafnify's own installed paths and rule.
    pub fn uninstall_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh").arg("-c").arg(crate::flavor::flavored(
            "if [ -x /usr/libexec/jafnify/jafnify-helper ]; then \
                 /usr/libexec/jafnify/jafnify-helper forget || exit $?; \
                 elif [ -x /usr/bin/wg-quick ]; then /usr/bin/wg-quick down jafn0 || true; fi; \
                 if [ -x /usr/sbin/ufw ]; then u=/usr/sbin/ufw; elif [ -x /usr/bin/ufw ]; then u=/usr/bin/ufw; else u=; fi; \
                 if [ -n \"$u\" ]; then \"$u\" status numbered 2>/dev/null | grep -F \"Jafnify discovery\" \
                 | sed -n 's/^\\[ *\\([0-9][0-9]*\\)\\].*/\\1/p' | sort -rn \
                 | while read -r n; do \"$u\" --force delete \"$n\" >/dev/null 2>&1 || true; done; fi; \
                 rm -f /usr/local/bin/jafnify /usr/local/bin/jafnify-gui \
                 /usr/libexec/jafnify/jafnify-helper \
                 /usr/libexec/jafnify/jafnify-route \
                 /usr/share/polkit-1/actions/org.jafnify.policy \
                 /etc/polkit-1/rules.d/49-jafnify.rules \
                 /etc/wireguard/jafn0.conf \
                 /usr/share/applications/jafnify.desktop \
                 /usr/share/icons/hicolor/scalable/apps/jafnify.svg \
                 /usr/share/icons/hicolor/scalable/apps/jafnify-symbolic.svg; \
                 rmdir /usr/libexec/jafnify 2>/dev/null || true",
        ));
        cmd
    }

    /// One call that installs both frontends (so the desktop entry can use an
    /// absolute path), the helper, policy, per-user rule (if given), desktop
    /// entry and icons. Steps are chained with `&&`; a failure stops and names
    /// the step. The rule is rewritten on upgrade, so an old file without the
    /// route grant is replaced.
    pub fn install_helper_command(&self, paths: InstallPaths<'_>) -> Command {
        let InstallPaths {
            jafnify,
            gui,
            helper,
            route,
            policy,
            desktop,
            icon,
            symbolic,
            rules,
        } = paths;
        let script = "step='create directories' && \
             install -d -o root -g root -m 755 /usr/local/bin /usr/libexec/jafnify \
             /usr/share/polkit-1/actions /etc/polkit-1/rules.d /usr/share/applications \
             /usr/share/icons/hicolor/scalable/apps && \
             step='install jafnify' && \
             install -o root -g root -m 755 \"$1\" /usr/local/bin/.jafnify.new && \
             mv -f /usr/local/bin/.jafnify.new /usr/local/bin/jafnify && \
             step='install jafnify-gui' && \
             install -o root -g root -m 755 \"$2\" /usr/local/bin/.jafnify-gui.new && \
             mv -f /usr/local/bin/.jafnify-gui.new /usr/local/bin/jafnify-gui && \
             step='install helper' && \
             install -o root -g root -m 755 \"$3\" /usr/libexec/jafnify/.jafnify-helper.new && \
             mv -f /usr/libexec/jafnify/.jafnify-helper.new /usr/libexec/jafnify/jafnify-helper && \
             step='install route helper' && \
             install -o root -g root -m 755 \"$4\" /usr/libexec/jafnify/.jafnify-route.new && \
             mv -f /usr/libexec/jafnify/.jafnify-route.new /usr/libexec/jafnify/jafnify-route && \
             step='install policy' && \
             install -o root -g root -m 644 \"$5\" /usr/share/polkit-1/actions/org.jafnify.policy && \
             step='install polkit rule' && \
             if [ -n \"$9\" ] && [ -e /etc/polkit-1/rules.d/49-jafnify.rules ]; then \
             install -o root -g root -m 644 \"$9\" /etc/polkit-1/rules.d/49-jafnify.rules; fi && \
             step='install desktop entry' && \
             install -o root -g root -m 644 \"$6\" /usr/share/applications/jafnify.desktop && \
             step='install icon' && \
             install -o root -g root -m 644 \"$7\" /usr/share/icons/hicolor/scalable/apps/jafnify.svg && \
             step='install symbolic icon' && \
             install -o root -g root -m 644 \"$8\" /usr/share/icons/hicolor/scalable/apps/jafnify-symbolic.svg || \
             { code=$?; echo \"jafnify install-helper: failed at: $step\" >&2; exit $code; }";
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh")
            .arg("-c")
            .arg(crate::flavor::flavored(script))
            .arg("sh")
            .arg(jafnify)
            .arg(gui)
            .arg(helper)
            .arg(route)
            .arg(policy)
            .arg(desktop)
            .arg(icon)
            .arg(symbolic)
            .arg(rules.unwrap_or_else(|| Path::new("")));
        cmd
    }

    /// `pkexec /bin/sh -c <fixed script> sh <ufw> <iface> <port> <comment>
    /// <group>`: add the one interface+port rule if missing, remove every older
    /// Jafnify discovery rule (group-only and subnet-scoped), never more. Only
    /// after the user's consent.
    pub fn ufw_allow_command(&self, iface: &str) -> Command {
        let (group, port) = crate::firewall::group_and_port();
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh")
            .arg("-c")
            .arg(ufw_rule_script())
            .arg("sh")
            .arg(&self.ufw_bin)
            .arg(iface)
            .arg(port)
            .arg("Jafnify discovery")
            .arg(group);
        cmd
    }

    /// `pkexec install -m 600 <source> /etc/wireguard/<iface>.conf`
    pub fn write_conf_command(&self, source: &Path) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.install_bin)
            .arg("-m")
            .arg("600")
            .arg(source)
            .arg(&self.teardown_conf);
        cmd
    }

    /// `pkexec wg-quick up <iface>`
    pub fn wg_quick_up_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg(&self.wg_quick_bin).arg("up").arg(&self.interface);
        cmd
    }

    /// `pkexec /bin/sh -c '"$1" down "$2"; rm -f "$3"' sh <wg-quick> <iface> <conf>`:
    /// take the tunnel down and remove the installed config in one privileged
    /// call. The values are passed as arguments, never spliced into the script.
    pub fn wg_quick_down_command(&self) -> Command {
        self.fixed_teardown_command(false)
    }

    /// The same fixed teardown the helper uses. The optional third argument is
    /// the per-user polkit rule removed by `forget`; `down` passes an empty
    /// string and leaves the rule in place.
    fn fixed_teardown_command(&self, remove_rule: bool) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh")
            .arg("-c")
            .arg(crate::helper::DOWN_SCRIPT)
            .arg("sh")
            .arg(&self.teardown_conf)
            .arg(&self.teardown_iface)
            .arg(if remove_rule {
                self.teardown_rules.as_os_str()
            } else {
                std::ffi::OsStr::new("")
            });
        cmd
    }

    /// `pkexec /bin/sh -c 'rm -f "$1"' sh <conf>`: remove the installed config
    /// alone (no tunnel up, e.g. after a failed bring-up).
    pub fn remove_conf_command(&self) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh")
            .arg("-c")
            .arg("rm -f \"$1\"")
            .arg("sh")
            .arg(&self.teardown_conf);
        cmd
    }

    /// `pkexec /bin/sh -c <kill switch removal>`: turn the kill switch off while
    /// the tunnel stays up. Same names as the hook that installs it.
    pub fn kill_switch_down_command(&self, iptables: bool) -> Command {
        let mut cmd = Command::new(&self.pkexec_bin);
        cmd.arg("/bin/sh")
            .arg("-c")
            .arg(jafnify_core::config::kill_switch_down(iptables));
        cmd
    }

    /// Kill switch off, tunnel not Connected: let the normal route win again.
    /// Route binary first (no password), then the tunnel helper (older
    /// installs), then one fixed mark-scoped shell command.
    pub fn route_fallback_command(&self) -> Command {
        if self.has_route_binary() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.route_bin).arg("route-fallback");
            cmd
        } else if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("route-fallback");
            cmd
        } else {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg("/bin/sh").arg("-c").arg(ROUTE_FALLBACK_SCRIPT);
            cmd
        }
    }

    /// Connected again: hand routing back to the tunnel.
    pub fn route_restore_command(&self) -> Command {
        if self.has_route_binary() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.route_bin).arg("route-restore");
            cmd
        } else if self.has_helper() {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg(&self.helper_bin).arg("route-restore");
            cmd
        } else {
            let mut cmd = Command::new(&self.pkexec_bin);
            cmd.arg("/bin/sh").arg("-c").arg(ROUTE_RESTORE_SCRIPT);
            cmd
        }
    }

    pub fn is_available(&self) -> bool {
        self.pkexec_bin.exists()
            || std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).any(|d| d.join(&self.pkexec_bin).exists()))
                .unwrap_or(false)
    }

    /// The only way a root command is spawned. Under the test guard, the
    /// executable must be a stub under the temp dir, so a test that forgot to
    /// inject one fails loudly instead of raising a polkit prompt on the host.
    pub fn run(&self, cmd: Command) -> std::io::Result<Output> {
        self.run_with_input(cmd, None)
    }

    pub fn run_with_input(
        &self,
        mut cmd: Command,
        input: Option<&[u8]>,
    ) -> std::io::Result<Output> {
        self.refuse_real_root(&cmd);
        if let Some(bytes) = input {
            cmd.stdin(Stdio::piped());
            let mut child = exec::spawn(cmd)?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(bytes)?;
            }
            child.wait_with_output()
        } else {
            exec::output(cmd)
        }
    }

    /// Start a privileged command without waiting for it. The caller polls the
    /// child, so a pending polkit prompt cannot stall the state loop.
    pub fn spawn(&self, mut cmd: Command) -> std::io::Result<Child> {
        self.refuse_real_root(&cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        exec::spawn(cmd)
    }

    fn refuse_real_root(&self, cmd: &Command) {
        if !test_guard_active() {
            return;
        }
        let program = Path::new(cmd.get_program());
        if is_temp_stub(program) {
            return;
        }
        panic!(
            "refusing to spawn {:?}: tests must point pkexec at a stub under {:?} \
             (set from {TEST_NO_ROOT_ENV}); this would have run a real privileged tool on the host",
            program,
            std::env::temp_dir()
        );
    }
}

pub(crate) fn test_guard_active() -> bool {
    if cfg!(test) || std::env::var_os(TEST_NO_ROOT_ENV).is_some_and(|value| value == "1") {
        return true;
    }
    // Integration test binaries live in target/<profile>/deps. A real
    // `jafnify` or `jafnify-gui` is a direct child of the profile dir, so it is
    // not affected by this guard.
    std::env::current_exe().is_ok_and(|exe| {
        exe.parent()
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == "deps")
    })
}

fn is_temp_stub(program: &Path) -> bool {
    let resolved = resolve_on_path(program);
    let (Ok(canonical), Ok(temp)) = (resolved.canonicalize(), std::env::temp_dir().canonicalize())
    else {
        return false;
    };
    canonical.starts_with(temp)
}

/// The temp-stub check other modules use to keep tests off the host's tools.
pub(crate) fn is_test_stub(program: &Path) -> bool {
    is_temp_stub(program)
}

fn resolve_on_path(program: &Path) -> PathBuf {
    if program.components().count() > 1 {
        return program.to_path_buf();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return program.to_path_buf();
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::RootCommands;

    #[test]
    fn the_restore_script_puts_back_the_gateway_dns_and_the_rule_order() {
        let dns = format!("resolvectl dns jafn0 {}", jafnify_core::GATEWAY_DNS_IP);
        assert!(super::ROUTE_RESTORE_SCRIPT.contains(&dns), "{dns}");
        assert!(super::ROUTE_RESTORE_SCRIPT.contains("ip -4 rule add priority"));
        assert!(super::ROUTE_FALLBACK_SCRIPT.starts_with("if iptables -S JAFNIFY_KS"));
    }

    #[test]
    fn the_test_guard_refuses_the_real_pkexec() {
        let root = RootCommands::new("jafn0");
        let err = std::panic::catch_unwind(|| root.run(super::Command::new("pkexec")))
            .expect_err("the test guard must refuse the real pkexec");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(msg.contains("refusing to spawn"), "{msg}");
    }
}
