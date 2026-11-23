//! Mark-scoped route fallback, shared by the root helper verbs, the small
//! `jafnify-route` binary and the fixed shell fallback.
//!
//! wg-quick installs one rule of its own when jafn0 connects:
//! `not from all fwmark <mark> lookup <table>` (plus the shared
//! `table main suppress_prefixlength 0` rule). The fallback deletes only
//! jafn0's rule, so traffic falls through the shared suppress rule to the
//! plain main table and the hotspot's default route wins. Any other
//! full-tunnel VPN keeps its own rule and table untouched.

pub const IFACE: &str = "jafn0";

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteCommand {
    /// Kill switch off, tunnel not Connected: normal routing wins.
    Fallback,
    /// Connected again: hand routing back to the tunnel.
    Restore,
}

/// The binary accepts exactly one of the two verbs and nothing else.
pub fn parse(args: &[String]) -> Result<RouteCommand, String> {
    if args.len() != 1 {
        return Err("usage: jafnify-route route-fallback|route-restore".to_string());
    }
    match args[0].as_str() {
        "route-fallback" => Ok(RouteCommand::Fallback),
        "route-restore" => Ok(RouteCommand::Restore),
        other => Err(format!("unknown command: {other}")),
    }
}

/// `wg show <iface> fwmark` prints `0xca6c`, `0xca6c/0xffff` or `off`.
/// The table is the mark's numeric value, exactly as wg-quick derives it.
pub fn parse_fwmark(text: &str) -> Option<u32> {
    let value = text.trim();
    if value.is_empty() || value == "off" {
        return None;
    }
    let value = value.split('/').next().unwrap_or(value).trim();
    match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => value.parse::<u32>().ok(),
    }
}

/// The hex form wg itself prints and the kernel rule shows.
pub fn hex(mark: u32) -> String {
    format!("{mark:#x}")
}

/// `ip -4 rule show` prints lines like
/// `32765:\tnot from all fwmark 0xca6c/0xffff lookup 51820`.
/// Only the exact mark and table count: another tunnel's rule is not ours.
pub fn tunnel_rule_present(rules: &str, mark: u32, table: u32) -> bool {
    let table = table.to_string();
    rules.lines().any(|line| {
        if !line.contains("not from all fwmark ") {
            return false;
        }
        let Some(at) = line.find("fwmark ") else {
            return false;
        };
        let rest = &line[at + "fwmark ".len()..];
        let Some(token) = rest.split_whitespace().next() else {
            return false;
        };
        let value = token.split('/').next().unwrap_or(token);
        if parse_fwmark(value) != Some(mark) {
            return false;
        }
        let Some(at) = line.find("lookup ") else {
            return false;
        };
        line[at + "lookup ".len()..].split_whitespace().next() == Some(table.as_str())
    })
}

/// The routing tables whose default route goes into `iface`, from the
/// unprivileged `ip -4 route show table all`: wg-quick writes
/// `default dev jafn0 table 51820` and keeps it while its rule is removed, so
/// this finds jafn0's table without `wg show` (which needs root).
pub fn iface_default_tables(routes_all: &str, iface: &str) -> Vec<u32> {
    let mut tables = Vec::new();
    for line in routes_all.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"default") {
            continue;
        }
        let dev = words.windows(2).find(|w| w[0] == "dev").map(|w| w[1]);
        if dev != Some(iface) {
            continue;
        }
        let table = words
            .windows(2)
            .find(|w| w[0] == "table")
            .and_then(|w| w[1].parse::<u32>().ok());
        if let Some(t) = table {
            if !tables.contains(&t) {
                tables.push(t);
            }
        }
    }
    tables
}

/// Whether jafn0's own rule is in place, from unprivileged `ip` output only:
/// Some(true) when a wg-quick rule (mark == table) points at a table whose
/// default route is jafn0, Some(false) when such a table exists without its
/// rule (fallback state), None when jafn0 has no wg-quick table at all.
pub fn iface_rule_installed(rules: &str, routes_all: &str, iface: &str) -> Option<bool> {
    let tables = iface_default_tables(routes_all, iface);
    if tables.is_empty() {
        return None;
    }
    Some(tables.iter().any(|&t| tunnel_rule_present(rules, t, t)))
}

/// Kill switch off, not Connected: remove jafn0's own rule and the IPv6
/// blackhole. The interference-free shared rule is left alone.
pub fn fallback_commands(mark: u32, table: u32) -> Vec<Vec<String>> {
    vec![rule("del", mark, table), blackhole("del")]
}

/// Connected again: put jafn0's rule back (only when it is missing, so a
/// teardown's successful delete cannot turn into a failing add loop) and the
/// blackhole. The table route was never removed.
pub fn restore_commands(
    mark: u32,
    table: u32,
    rule_present: bool,
    priority: u32,
) -> Vec<Vec<String>> {
    let mut cmds = Vec::new();
    if !rule_present {
        cmds.push(rule_add_at(priority, mark, table));
    }
    cmds.push(blackhole("replace"));
    cmds
}

fn rule(verb: &str, mark: u32, table: u32) -> Vec<String> {
    ["-4", "rule", verb, "not", "from", "all", "fwmark"]
        .iter()
        .map(|s| s.to_string())
        .chain([hex(mark), "lookup".to_string(), table.to_string()])
        .collect()
}

/// wg-quick adds the tunnel rule first and the suppress rule second, so the
/// kernel places them suppress (N) then tunnel (N + 1). A re-add without a
/// priority would land before the suppress rule and pull LAN traffic into the
/// tunnel, so the restore names the slot right after the suppress rule.
pub const DEFAULT_TUNNEL_RULE_PRIORITY: u32 = 32765;

pub fn restore_priority(rules: &str) -> u32 {
    rules
        .lines()
        .filter(|line| line.contains("lookup main suppress_prefixlength 0"))
        .filter_map(|line| line.split_once(':')?.0.trim().parse::<u32>().ok())
        .min()
        .map_or(DEFAULT_TUNNEL_RULE_PRIORITY, |p| p.saturating_add(1))
}

fn rule_add_at(priority: u32, mark: u32, table: u32) -> Vec<String> {
    ["-4", "rule", "add", "priority"]
        .iter()
        .map(|s| s.to_string())
        .chain([priority.to_string()])
        .chain(
            ["not", "from", "all", "fwmark"]
                .iter()
                .map(|s| s.to_string()),
        )
        .chain([hex(mark), "lookup".to_string(), table.to_string()])
        .collect()
}

fn blackhole(action: &str) -> Vec<String> {
    ["-6", "route", action, "blackhole", "::/0", "metric", "1"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// What the IPv6 guard did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ipv6Guard {
    /// No IPv6 stack on this host: nothing can leave over IPv6.
    NoStack,
    /// The blackhole is in place.
    Blackholed,
    /// The blackhole could not be added: IPv6 can leave around the tunnel.
    Failed(String),
}

/// The IPv6 guard: while the tunnel is up, IPv6 must not leave around it (the
/// tunnel carries IPv4 only). The stack is optional: a kernel booted with
/// `ipv6.disable=1` has neither /proc/net/if_inet6 nor /proc/sys/net/ipv6 and
/// `ip -6 route` fails there. So the guard is skipped without a stack, and
/// only a real failure to add the blackhole is reported.
pub fn ipv6_guard(stack_present: bool, added: Result<(), String>) -> Ipv6Guard {
    if !stack_present {
        return Ipv6Guard::NoStack;
    }
    match added {
        Ok(()) => Ipv6Guard::Blackholed,
        Err(e) => Ipv6Guard::Failed(e),
    }
}

/// Does this host have an IPv6 stack? The kernel exposes it with
/// /proc/net/if_inet6 and /proc/sys/net/ipv6.
pub fn ipv6_stack_present() -> bool {
    Path::new("/proc/net/if_inet6").exists() && Path::new("/proc/sys/net/ipv6").exists()
}

/// The blackhole that keeps IPv6 inside the tunnel. `replace` is idempotent;
/// metric 1 beats any RA default route.
pub fn run_ipv6_blackhole(action: &str) -> Result<(), String> {
    let ip = ip_binary();
    let args = blackhole(action);
    let ok = Command::new(ip)
        .args(&args)
        .status()
        .map_err(|e| format!("cannot run {ip}: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("{ip} {} failed", args.join(" ")))
    }
}

/// Add the IPv6 blackhole (the tunnel is up). Nothing without an IPv6 stack;
/// a failure is reported, so the caller can warn instead of hiding it.
pub fn run_ipv6_guard_add() -> Ipv6Guard {
    ipv6_guard(ipv6_stack_present(), run_ipv6_blackhole("replace"))
}

/// Remove the IPv6 blackhole (the tunnel goes down or falls back). Nothing
/// without an IPv6 stack; a failed removal leaves nothing to leak, so it is
/// not reported.
pub fn run_ipv6_guard_remove() {
    if !ipv6_stack_present() {
        return;
    }
    let _ = run_ipv6_blackhole("del");
}

/// Undo the tunnel's DNS routing for the fallback window.
pub fn resolve_fallback() -> &'static [&'static str] {
    &["revert", "jafn0"]
}

/// Re-apply the same DNS routing the PostUp installed: the server, the search
/// domain and the default route. `revert` in the fallback clears all three.
pub fn resolve_restore() -> [&'static [&'static str]; 3] {
    [
        &["dns", IFACE, jafnify_core::GATEWAY_DNS_IP],
        &["domain", IFACE, "~."],
        &["default-route", IFACE, "yes"],
    ]
}

/// The kill switch owns the packet flow while it is on, so the fallback must
/// refuse instead of second-guessing it. `Some(true)` means the tool reported
/// the chain/table; a missing tool cannot have installed it.
pub fn kill_switch_present(iptables: Option<bool>, nft: Option<bool>) -> bool {
    iptables.unwrap_or(false) || nft.unwrap_or(false)
}

/// The kernel prints the IPv6 default as `default`, not `::/0`
/// (`blackhole default dev lo metric 1 pref medium`).
pub fn blackhole_present(routes: &str) -> bool {
    routes.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("blackhole") && matches!(words.next(), Some("default") | Some("::/0"))
    })
}

// ---- runtime (root only) ---------------------------------------------------

pub fn ip_binary() -> &'static str {
    for candidate in ["/usr/sbin/ip", "/sbin/ip", "/usr/bin/ip", "/bin/ip"] {
        if Path::new(candidate).exists() {
            return candidate;
        }
    }
    "/usr/bin/ip"
}

pub fn wg_binary() -> &'static str {
    for candidate in ["/usr/bin/wg", "/usr/sbin/wg", "/bin/wg", "/sbin/wg"] {
        if Path::new(candidate).exists() {
            return candidate;
        }
    }
    "/usr/bin/wg"
}

fn interface_present() -> bool {
    Path::new("/sys/class/net").join(IFACE).exists()
}

/// jafn0's WireGuard mark; the table is its numeric value, as wg-quick does.
fn live_mark() -> Result<Option<u32>, String> {
    let wg = wg_binary();
    let out = Command::new(wg)
        .args(["show", IFACE, "fwmark"])
        .output()
        .map_err(|e| format!("cannot run {wg}: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(parse_fwmark(&String::from_utf8_lossy(&out.stdout)))
}

fn rule_show() -> Result<String, String> {
    let ip = ip_binary();
    let out = Command::new(ip)
        .args(["-4", "rule", "show"])
        .output()
        .map_err(|e| format!("cannot run {ip}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{ip} -4 rule show failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_ip(args: &[String], critical: bool) -> Result<(), String> {
    let ip = ip_binary();
    let ok = Command::new(ip)
        .args(args)
        .status()
        .map_err(|e| format!("cannot run {ip}: {e}"))?
        .success();
    if ok || !critical {
        Ok(())
    } else {
        Err(format!("{ip} {} failed", args.join(" ")))
    }
}

/// One resolver step with its real error text. The root helper can run this
/// outside wg-quick's AppArmor profile, where systemd-resolved accepts the
/// default-route call.
fn run_resolve(args: &[&str]) -> Result<(), String> {
    let out = Command::new("resolvectl")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run resolvectl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr.trim();
    if reason.is_empty() {
        Err(format!("resolvectl {} failed", args.join(" ")))
    } else {
        Err(reason.to_string())
    }
}

/// Re-apply jafn0's DNS routing after wg-quick's hooks. The PostUp covers the
/// search domain; the default route is denied inside wg-quick's profile, so
/// the root helper repeats all three steps here and reports what failed.
pub fn run_resolve_apply() -> Result<(), String> {
    if !interface_present() {
        return Ok(());
    }
    let mut errors = Vec::new();
    for args in resolve_restore() {
        if let Err(e) = run_resolve(args) {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Is the kill-switch firewall installed right now? Either guard tool may
/// have put it there; a tool that is not installed cannot have.
pub fn kill_switch_on() -> bool {
    kill_switch_present(
        probe(
            &["/usr/sbin/iptables", "/sbin/iptables", "iptables"],
            &["-S", "JAFNIFY_KS"],
        ),
        probe(
            &["/usr/sbin/nft", "/sbin/nft", "nft"],
            &["list", "table", "inet", "jafnify_killswitch"],
        ),
    )
}

fn probe(programs: &[&str], args: &[&str]) -> Option<bool> {
    for program in programs {
        if let Ok(out) = Command::new(program).args(args).output() {
            return Some(out.status.success());
        }
    }
    None
}

/// Remove jafn0's own routing (not the interface). Refuses while the kill
/// switch is on: the user asked for that blocking, so nothing may undo it.
pub fn run_fallback() -> Result<(), String> {
    if kill_switch_on() {
        return Err("the kill switch is on; the tunnel routing is kept".to_string());
    }
    if !interface_present() {
        return Ok(());
    }
    if let Some(mark) = live_mark()? {
        if tunnel_rule_present(&rule_show()?, mark, mark) {
            run_ip(&rule("del", mark, mark), true)?;
        }
    }
    // The blackhole and the tunnel DNS routing belong to jafn0 regardless of
    // how its mark is configured.
    run_ipv6_guard_remove();
    // The resolver step can fail (a stopped resolver). The fallback itself
    // must not fail for it, but the fault is real: report it once, the same
    // way the restore reports its own.
    if let Err(e) = run_resolve(resolve_fallback()) {
        eprintln!("jafnify-route: dns routing: {e}");
    }
    Ok(())
}

/// Connected again: hand routing back to the tunnel.
pub fn run_restore() -> Result<(), String> {
    if !interface_present() {
        return Ok(());
    }
    if let Some(mark) = live_mark()? {
        let rules = rule_show()?;
        if !tunnel_rule_present(&rules, mark, mark) {
            run_ip(&rule_add_at(restore_priority(&rules), mark, mark), true)?;
        }
    }
    // The IPv6 blackhole goes back with the rule. A failure to add it is
    // reported like the DNS one: the routing is back, the leak is real.
    if let Ipv6Guard::Failed(reason) = run_ipv6_guard_add() {
        eprintln!("jafnify-route: ipv6 guard: {reason}");
    }
    let mut dns_errors = Vec::new();
    for args in resolve_restore() {
        if let Err(e) = run_resolve(args) {
            dns_errors.push(e);
        }
    }
    if !dns_errors.is_empty() {
        // The route rule is back; a resolver step failed. Say so once instead
        // of pretending the whole restore worked.
        eprintln!("jafnify-route: dns routing: {}", dns_errors.join("; "));
    }
    Ok(())
}
