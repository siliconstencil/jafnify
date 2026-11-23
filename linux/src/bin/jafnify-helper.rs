//! Root-only tunnel helper, invoked through pkexec and the
//! `org.jafnify.tunnel` polkit action. Fixed commands only: `up` reads a
//! structured request from stdin, `down` removes the interface/config, and
//! `status` reports the kernel peer key (for adopting an existing tunnel).

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use jafnify_linux::helper::{
    interface_path, parse_command, parse_latest_handshakes, parse_status, peer_value, read_request,
    render, status_text, suppress_rule_present, teardown, HandshakeAge, HelperCommand, LiveConfig,
    Status, CONFIG_PATH, IFACE, IP, WG, WG_QUICK,
};
use jafnify_linux::polkit;
use jafnify_linux::route;

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("jafnify-helper: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_command(&args)? {
        HelperCommand::Up => up(),
        HelperCommand::Down => down(),
        HelperCommand::Status => status(),
        HelperCommand::Inspect => inspect(),
        HelperCommand::Endpoint => endpoint(),
        HelperCommand::Forget => forget(),
        HelperCommand::Suppress => suppress(),
        HelperCommand::HandshakeAge => handshake_age(),
        HelperCommand::RouteFallback => route_fallback(),
        HelperCommand::RouteRestore => route_restore(),
    }
}

fn up() -> Result<(), String> {
    let request = read_request(&mut io::stdin().lock())?;
    let text = render(&request)?;
    write_config(&text).map_err(|e| format!("cannot write {CONFIG_PATH}: {e}"))?;
    let ok = Command::new(WG_QUICK)
        .args(["up", IFACE])
        .status()
        .map_err(|e| format!("cannot run {WG_QUICK}: {e}"))?
        .success();
    if !ok {
        let _ = fs::remove_file(CONFIG_PATH);
        return Err(format!("{WG_QUICK} up {IFACE} failed"));
    }
    if let Some(user) = polkit::invoking_user() {
        if let Err(e) = polkit::install_at(Path::new(polkit::RULES_PATH), &user) {
            let _ = Command::new(WG_QUICK).args(["down", IFACE]).status();
            let _ = fs::remove_file(CONFIG_PATH);
            return Err(format!("cannot install the polkit rule: {e}"));
        }
    }
    // The default-route step is denied inside wg-quick's AppArmor profile, so
    // repeat the resolver setup here and report a real failure without
    // failing an otherwise healthy tunnel.
    if let Err(e) = route::run_resolve_apply() {
        eprintln!("{}{}", jafnify_linux::helper::DNS_WARNING_PREFIX, e);
    }
    // The IPv6 guard lives here for the same reason: wg-quick's hooks may not
    // read /proc/sys/net/ipv6, and `ip -6 route` fails on a kernel booted
    // with ipv6.disable=1. A failure is a warning, never a failed tunnel.
    if let route::Ipv6Guard::Failed(reason) = route::run_ipv6_guard_add() {
        eprintln!(
            "{}{}",
            jafnify_linux::helper::IPV6_WARNING_PREFIX,
            reason
        );
    }
    Ok(())
}

fn down() -> Result<(), String> {
    teardown(&interface_path(), Path::new(CONFIG_PATH))
}

/// Forget is the one unpair action that also removes the per-user rule. A
/// plain disconnect keeps the rule so the next reconnect is prompt-free.
fn forget() -> Result<(), String> {
    down()?;
    polkit::remove_at(Path::new(polkit::RULES_PATH))
        .map_err(|e| format!("cannot remove {}: {e}", polkit::RULES_PATH))
}

/// Re-add the shared main-table rule another tool may have deleted. Fixed
/// command, no arguments, only while the tunnel exists.
fn suppress() -> Result<(), String> {
    if !interface_path().exists() {
        return Ok(());
    }
    let ip = if Path::new("/usr/sbin/ip").exists() {
        "/usr/sbin/ip"
    } else {
        IP
    };
    let show = Command::new(ip)
        .args(["-4", "rule", "show"])
        .output()
        .map_err(|e| format!("cannot run {ip}: {e}"))?;
    if !show.status.success() {
        return Err(format!("{ip} -4 rule show failed"));
    }
    if suppress_rule_present(&String::from_utf8_lossy(&show.stdout)) {
        return Ok(());
    }
    let ok = Command::new(ip)
        .args([
            "-4",
            "rule",
            "add",
            "table",
            "main",
            "suppress_prefixlength",
            "0",
        ])
        .status()
        .map_err(|e| format!("cannot run {ip}: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("{ip} -4 rule add failed"))
    }
}

/// Kill switch off, not Connected: remove jafn0's own wg-quick rule, the IPv6
/// blackhole and the tunnel DNS routing. jafn0 and its table route stay, so a
/// returning peer costs no root call. Kept for older installs that only have
/// this helper; the route binary shares the same code.
fn route_fallback() -> Result<(), String> {
    route::run_fallback()
}

/// Connected again: put the rule, the blackhole and the DNS routing back.
fn route_restore() -> Result<(), String> {
    route::run_restore()
}

/// Read-only: the age of the peer's newest WireGuard handshake. Fixed command,
/// no arguments, no shell string. Prints whole seconds or `never`.
fn handshake_age() -> Result<(), String> {
    let out = Command::new(WG)
        .args(["show", IFACE, "latest-handshakes"])
        .output()
        .map_err(|e| format!("cannot run {WG}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{WG} show {IFACE} latest-handshakes failed"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match parse_latest_handshakes(&String::from_utf8_lossy(&out.stdout), now) {
        Some(HandshakeAge::Seconds(seconds)) => {
            println!("{seconds}");
            Ok(())
        }
        Some(HandshakeAge::Never) => {
            println!("never");
            Ok(())
        }
        Some(HandshakeAge::Unknown) | None => Err("cannot read the latest handshakes".to_string()),
    }
}

fn status() -> Result<(), String> {
    if !interface_path().exists() {
        print!("{}", status_text(&Status::Down));
        return Ok(());
    }
    let out = Command::new(WG)
        .args(["show", IFACE, "peers"])
        .output()
        .map_err(|e| format!("cannot run {WG}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{WG} show {IFACE} failed"));
    }
    let peer = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string);
    print!(
        "{}",
        status_text(&parse_status(&format!("up\n{}", peer.unwrap_or_default())))
    );
    Ok(())
}

/// Read the live kernel peer fields plus the installed config's address and
/// kill-switch marker. The private key is never printed.
fn inspect() -> Result<(), String> {
    if !interface_path().exists() {
        return Err(format!("{IFACE} is not up"));
    }
    let text =
        fs::read_to_string(CONFIG_PATH).map_err(|e| format!("cannot read {CONFIG_PATH}: {e}"))?;
    let cfg = jafnify_core::ConfigGenerator::parse(&text)
        .map_err(|e| format!("bad installed config: {e}"))?;
    let peer = wg_show(&["show", IFACE, "peers"])?
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| format!("{IFACE} has no peer"))?
        .to_string();
    let endpoints = wg_show(&["show", IFACE, "endpoints"])?;
    let endpoint = peer_value(&endpoints, &peer).filter(|value| value != "(none)");
    let allowed = wg_show(&["show", IFACE, "allowed-ips"])?;
    let allowed_ips = peer_value(&allowed, &peer).unwrap_or_default();
    let live = LiveConfig {
        peer_public_key: peer,
        endpoint,
        allowed_ips,
        address: cfg.interface.address,
        kill_switch: text.contains("JAFNIFY_KS") || text.contains("jafnify_killswitch"),
    };
    print!("{}", live.to_toml()?);
    Ok(())
}

/// Update only the endpoint of the existing peer. The peer key comes from the
/// installed config, never from the unprivileged caller.
fn endpoint() -> Result<(), String> {
    if !interface_path().exists() {
        return Err(format!("{IFACE} is not up"));
    }
    let mut input = String::new();
    io::stdin()
        .take(128)
        .read_to_string(&mut input)
        .map_err(|e| format!("cannot read endpoint: {e}"))?;
    let endpoint: SocketAddr = input
        .trim()
        .parse()
        .map_err(|_| "bad endpoint".to_string())?;
    let text =
        fs::read_to_string(CONFIG_PATH).map_err(|e| format!("cannot read {CONFIG_PATH}: {e}"))?;
    let cfg = jafnify_core::ConfigGenerator::parse(&text)
        .map_err(|e| format!("bad installed config: {e}"))?;
    let ok = Command::new(WG)
        .args([
            "set",
            IFACE,
            "peer",
            &cfg.peer.public_key,
            "endpoint",
            &endpoint.to_string(),
        ])
        .status()
        .map_err(|e| format!("cannot run {WG}: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("{WG} set {IFACE} endpoint failed"))
    }
}

fn wg_show(args: &[&str]) -> Result<String, String> {
    let out = Command::new(WG)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {WG}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{WG} {} failed", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_config(text: &str) -> io::Result<()> {
    let dir = std::path::Path::new(CONFIG_PATH)
        .parent()
        .expect("CONFIG_PATH has a parent");
    if !dir.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let tmp = dir.join(".jafn0.conf.jafnify");
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, CONFIG_PATH)
}
