//! Pure firewall wording and rule text, shared by every platform.
//!
//! Only the words and the rule construction live here; the platform detects
//! its own firewall and applies a rule (Linux `ufw`/nft, Windows Defender
//! later). Moving this half here lets `view` build the help text without
//! knowing which operating system is drawing it.

use std::net::Ipv4Addr;

use crate::DISCOVERY_ADDR;

/// One line for the status card when discovery is blocked; the help text goes
/// under it.
pub const BLOCKED_STATUS: &str = "Your firewall is blocking the search";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallKind {
    /// ufw is enabled: a narrow rule can be added with consent.
    Ufw,
    /// Something else (plain nftables, iptables): the rule is only shown.
    Other,
}

pub fn group_and_port() -> (&'static str, &'static str) {
    DISCOVERY_ADDR
        .rsplit_once(':')
        .expect("DISCOVERY_ADDR is host:port")
}

/// The LAN network of `ip/prefix` as "a.b.c.0/24". Only LAN-sized networks
/// (/16 to /30): a rule is never wider than that, and a single host has no LAN.
pub fn lan_subnet(ip: Ipv4Addr, prefix: u8) -> Option<String> {
    if !(16..=30).contains(&prefix) {
        return None;
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    Some(format!("{}/{prefix}", Ipv4Addr::from(u32::from(ip) & mask)))
}

/// Arguments for `ufw` that allow the discovery frames on `iface`: UDP on the
/// discovery port, whatever the destination (the multicast group or a unicast
/// copy to this host).
pub fn ufw_allow_args(iface: &str) -> Vec<String> {
    let (_group, port) = group_and_port();
    [
        "allow",
        "in",
        "on",
        iface,
        "to",
        "any",
        "port",
        port,
        "proto",
        "udp",
        "comment",
        "Jafnify discovery",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The same scope as an nftables rule (for the user to add to their input chain).
pub fn nft_rule(iface: &str) -> String {
    let (_group, port) = group_and_port();
    format!("iifname \"{iface}\" udp dport {port} accept")
}

/// The rule for `iface`, in both forms, one per line (`jafnify firewall rule`):
/// "ufw: <command>" and "nft: <rule expression>".
pub fn rule_lines(iface: &str) -> Vec<String> {
    let args = ufw_allow_args(iface);
    let (comment, rest) = args.split_last().expect("the rule has arguments");
    vec![
        format!("ufw: ufw {} '{comment}'", rest.join(" ")),
        format!("nft: {}", nft_rule(iface)),
    ]
}

/// What to tell the user, and what they can do, in plain English.
pub fn blocked_help(kind: FirewallKind, iface: Option<&str>) -> String {
    let (_group, port) = group_and_port();
    let what = format!(
        "The firewall on this computer is dropping the discovery messages the other device \
         sends (UDP {port}), so the two devices cannot find each other."
    );
    match (kind, iface) {
        (FirewallKind::Ufw, Some(iface)) => format!(
            "{what} Allow UDP {port} in on your Wi-Fi interface ({iface}) with the button below, \
             or run: ufw {}. To take the rule back out later, run: ufw delete allow in on {iface} \
             to any port {port} proto udp",
            ufw_allow_args(iface).join(" ")
        ),
        (FirewallKind::Other, Some(iface)) => format!(
            "{what} Allow UDP {port} in on your Wi-Fi interface ({iface}), for example with \
             nftables: {}",
            nft_rule(iface)
        ),
        (_, None) => {
            format!("{what} Allow UDP {port} in on your Wi-Fi interface in your firewall.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_lines_are_unchanged() {
        assert_eq!(
            rule_lines("wlo1"),
            vec![
                "ufw: ufw allow in on wlo1 to any port 9799 proto udp comment 'Jafnify discovery'"
                    .to_string(),
                "nft: iifname \"wlo1\" udp dport 9799 accept".to_string(),
            ]
        );
        assert_eq!(
            lan_subnet(Ipv4Addr::new(10, 20, 89, 70), 24).as_deref(),
            Some("10.20.89.0/24")
        );
    }
}
