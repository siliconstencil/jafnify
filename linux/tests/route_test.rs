//! The mark-scoped route fallback: delete only jafn0's own wg-quick rule (never
//! a shared main-table rule, so another full-tunnel VPN keeps working), drop the
//! IPv6 blackhole and the tunnel DNS routing. Restore puts exactly those back,
//! in wg-quick's own form.

use jafnify_linux::route::{
    blackhole_present, fallback_commands, hex, ipv6_guard, kill_switch_present, parse, parse_fwmark,
    resolve_fallback, resolve_restore, restore_commands, restore_priority, tunnel_rule_present,
    Ipv6Guard, RouteCommand,
};

#[test]
fn the_ipv6_guard_is_skipped_without_an_ipv6_stack() {
    // A kernel booted with ipv6.disable=1 has no IPv6 stack: nothing can leave
    // over IPv6, so the blackhole is not needed and `ip -6 route` would fail.
    assert_eq!(ipv6_guard(false, Ok(())), Ipv6Guard::NoStack);
    assert_eq!(
        ipv6_guard(false, Err("ip -6 route replace failed".to_string())),
        Ipv6Guard::NoStack
    );
}

#[test]
fn the_ipv6_guard_reports_a_blackhole_it_could_not_add() {
    assert_eq!(ipv6_guard(true, Ok(())), Ipv6Guard::Blackholed);
    assert_eq!(
        ipv6_guard(true, Err("ip -6 route replace failed".to_string())),
        Ipv6Guard::Failed("ip -6 route replace failed".to_string())
    );
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_two_verbs_are_the_only_ones_accepted() {
    assert_eq!(
        parse(&args(&["route-fallback"])),
        Ok(RouteCommand::Fallback)
    );
    assert_eq!(parse(&args(&["route-restore"])), Ok(RouteCommand::Restore));
    assert!(parse(&[]).is_err());
    assert!(parse(&args(&["route-fallback", "x"])).is_err());
    assert!(parse(&args(&["up"])).is_err());
}

#[test]
fn a_wireguard_fwmark_is_read_in_hex_with_or_without_a_mask() {
    assert_eq!(parse_fwmark("0xca6c"), Some(0xca6c));
    assert_eq!(parse_fwmark("0xca6c/0xffff"), Some(0xca6c));
    assert_eq!(parse_fwmark("51820"), Some(51820));
    assert_eq!(parse_fwmark(" off "), None);
    assert_eq!(parse_fwmark(""), None);
    assert_eq!(parse_fwmark("nonsense"), None);
}

#[test]
fn only_jafn0s_own_rule_counts() {
    let rules = "0:\tfrom all lookup local\n\
                 32764:\tfrom all lookup main suppress_prefixlength 0\n\
                 32765:\tnot from all fwmark 0xca6c/0xffff lookup 51820\n\
                 32766:\tfrom all lookup main";
    assert!(tunnel_rule_present(rules, 0xca6c, 51820));
    assert!(!tunnel_rule_present(rules, 0xc000, 51820));
    assert!(!tunnel_rule_present(rules, 0xca6c, 99));
    let other = "250:\tnot from all fwmark 0xc000/0xffff lookup 51820";
    assert!(!tunnel_rule_present(other, 0xca6c, 51820));
    assert!(tunnel_rule_present(other, 0xc000, 51820));
    // A plain (non-inverted) rule is not wg-quick's and is not ours.
    let plain = "250:\tfrom all fwmark 0xca6c/0xffff lookup 51820";
    assert!(!tunnel_rule_present(plain, 0xca6c, 51820));
    assert!(!tunnel_rule_present("", 0xca6c, 51820));
}

#[test]
fn fallback_removes_only_jafn0s_rule_and_the_blackhole() {
    assert_eq!(
        fallback_commands(0xca6c, 51820),
        vec![
            args(&[
                "-4", "rule", "del", "not", "from", "all", "fwmark", "0xca6c", "lookup", "51820"
            ]),
            args(&["-6", "route", "del", "blackhole", "::/0", "metric", "1"]),
        ]
    );
    // The old design added a global main-table rule. It must never come back.
    assert!(fallback_commands(0xca6c, 51820)
        .iter()
        .all(|cmd| !cmd.contains(&"main".to_string())));
}

#[test]
fn restore_readds_the_wireguard_rule_right_after_the_suppress_rule() {
    // Without an explicit priority the kernel puts the re-added rule at
    // 32763, before the shared suppress rule, and LAN traffic would enter the
    // tunnel. wg-quick's order is suppress first, then the tunnel rule.
    let cmds = restore_commands(0xca6c, 51820, false, 32765);
    assert_eq!(
        cmds[0],
        args(&[
            "-4", "rule", "add", "priority", "32765", "not", "from", "all", "fwmark", "0xca6c",
            "lookup", "51820"
        ])
    );
    assert_eq!(
        cmds[1],
        args(&["-6", "route", "replace", "blackhole", "::/0", "metric", "1"])
    );
}

#[test]
fn the_restore_priority_follows_the_suppress_rule() {
    let rules = "0:\tfrom all lookup local\n\
                 32764:\tfrom all lookup main suppress_prefixlength 0\n\
                 32766:\tfrom all lookup main\n";
    assert_eq!(restore_priority(rules), 32765);
    let moved = "0:\tfrom all lookup local\n\
                 31000:\tfrom all lookup main suppress_prefixlength 0\n\
                 32764:\tfrom all lookup main suppress_prefixlength 0\n";
    assert_eq!(restore_priority(moved), 31001);
    // No suppress rule at all: wg-quick's usual slot.
    assert_eq!(restore_priority("0:\tfrom all lookup local\n"), 32765);
}

#[test]
fn restore_is_idempotent_when_the_rule_is_still_there() {
    assert_eq!(
        restore_commands(0xca6c, 51820, true, 32765),
        vec![args(&[
            "-6",
            "route",
            "replace",
            "blackhole",
            "::/0",
            "metric",
            "1"
        ])]
    );
}

#[test]
fn the_dns_commands_are_pinned() {
    assert_eq!(resolve_fallback(), &["revert", "jafn0"]);
    let restore = resolve_restore();
    // `revert` clears the server as well as the domain: the server must come
    // back or DNS breaks (and leaks) after the tunnel returns.
    assert_eq!(restore[0], &["dns", "jafn0", jafnify_core::GATEWAY_DNS_IP]);
    assert_eq!(restore[1], &["domain", "jafn0", "~."]);
    assert_eq!(restore[2], &["default-route", "jafn0", "yes"]);
}

#[test]
fn the_guard_refuses_when_either_firewall_has_the_kill_switch() {
    assert!(kill_switch_present(Some(true), Some(false)));
    assert!(kill_switch_present(Some(false), Some(true)));
    assert!(!kill_switch_present(Some(false), Some(false)));
    assert!(!kill_switch_present(None, None));
}

#[test]
fn the_ipv6_blackhole_is_recognised() {
    assert!(blackhole_present("blackhole ::/0 metric 1024"));
    // What `ip -6 route show` really prints for the PostUp blackhole.
    assert!(blackhole_present(
        "blackhole default dev lo metric 1 pref medium"
    ));
    assert!(!blackhole_present("default via 192.168.77.1 dev jl-lan"));
    assert!(!blackhole_present(""));
}

#[test]
fn the_hex_form_matches_wg_output() {
    assert_eq!(hex(0xca6c), "0xca6c");
    assert_eq!(hex(51820), "0xca6c");
}

// Live: `jafnify fallback` as the normal user (the supervised
// tray's ExecStopPost) did nothing, because `wg show jafn0 fwmark` needs root.
// The same fact is readable from unprivileged `ip` output.
const RULES_UP: &str = "0:\tfrom all lookup local\n\
32764:\tfrom all lookup main suppress_prefixlength 0\n\
32765:\tnot from all fwmark 0xca6c lookup 51820\n\
32766:\tfrom all lookup main\n\
32767:\tfrom all lookup default\n";
const RULES_DOWN: &str = "0:\tfrom all lookup local\n\
32764:\tfrom all lookup main suppress_prefixlength 0\n\
32766:\tfrom all lookup main\n\
32767:\tfrom all lookup default\n";
const ROUTES_ALL: &str = "default dev jafn0 table 51820 scope link \n\
default via 10.174.190.72 dev wlo1 proto dhcp src 10.174.190.70 metric 600 \n\
10.174.190.0/24 dev wlo1 proto kernel scope link src 10.174.190.70 metric 600 \n\
local 10.233.0.2 dev jafn0 table local proto kernel scope host src 10.233.0.2 \n";

#[test]
fn jafn0s_table_is_found_without_wg_show() {
    use jafnify_linux::route::iface_default_tables;
    assert_eq!(iface_default_tables(ROUTES_ALL, "jafn0"), vec![51820]);
    assert!(iface_default_tables(ROUTES_ALL, "wg0").is_empty());
}

#[test]
fn the_rule_state_is_read_from_unprivileged_ip_output() {
    use jafnify_linux::route::iface_rule_installed;
    assert_eq!(
        iface_rule_installed(RULES_UP, ROUTES_ALL, "jafn0"),
        Some(true)
    );
    assert_eq!(
        iface_rule_installed(RULES_DOWN, ROUTES_ALL, "jafn0"),
        Some(false)
    );
    // No wg-quick table routes into jafn0: unknown, not "missing".
    assert_eq!(
        iface_rule_installed(RULES_UP, "default via 10.0.0.1 dev wlo1\n", "jafn0"),
        None
    );
}

#[test]
fn another_tunnels_rule_is_not_taken_for_jafn0s() {
    use jafnify_linux::route::iface_rule_installed;
    // wg0 owns 51820; wg-quick gave jafn0 the next table, whose rule is gone.
    let rules = "32765:\tnot from all fwmark 0xca6c lookup 51820\n";
    let routes =
        "default dev wg0 table 51820 scope link \ndefault dev jafn0 table 51821 scope link \n";
    assert_eq!(iface_rule_installed(rules, routes, "jafn0"), Some(false));
}
