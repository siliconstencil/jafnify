//! WireGuard config generation and INI-style parsing.
//! Every struct denies unknown fields so a config with an unknown key is
//! rejected loudly (project rule: new fields must be added to the wgserv
//! Config struct first).

use crate::state::Role;
use crate::{ALLOWED_IPS, GATEWAY_DNS_IP, JOINER_IP, LISTEN_PORT, MTU, SHARER_IP};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceConfig {
    pub address: String,
    pub private_key: String,
    #[serde(default)]
    pub dns: Option<String>,
    #[serde(default)]
    pub mtu: Option<u16>,
    /// wg-quick hook run when the interface comes up (root).
    #[serde(default)]
    pub post_up: Option<String>,
    /// wg-quick hook run after the interface is gone (root).
    #[serde(default)]
    pub post_down: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub public_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub allowed_ips: String,
    #[serde(default)]
    pub persistent_keepalive: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireGuardConfig {
    pub interface: InterfaceConfig,
    pub peer: PeerConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config parse error: {0}")]
    Parse(String),
    #[error("invalid config field: {0}")]
    InvalidField(String),
}

/// Laptop side: NEW inbound connections on the tunnel interface are dropped while
/// it is up. Established and related traffic (replies to our requests) passes.
/// Plain iptables, so it works whether or not ufw is active.
const INBOUND_GUARD_UP: &str = "iptables -I INPUT 1 -i %i -m conntrack --ctstate NEW -j DROP";
const INBOUND_GUARD_DOWN: &str =
    "iptables -D INPUT -i %i -m conntrack --ctstate NEW -j DROP || true";
/// The same guard with nft, in a table of its own (removed as one).
const NFT_GUARD_UP: &str = "nft delete table inet jafnify_guard 2>/dev/null || true; \
     nft add table inet jafnify_guard; \
     nft add chain inet jafnify_guard input '{ type filter hook input priority -10 ; }'; \
     nft add rule inet jafnify_guard input iifname %i ct state new drop";
const NFT_GUARD_DOWN: &str = "nft delete table inet jafnify_guard || true";
/// With systemd-resolved (chosen at config time): every domain resolves through
/// this interface, and the routing is reverted on down.
/// The catch-all routing domain `~.` already makes this link the resolver's
/// route for every name, so a separate `resolvectl default-route` call is not
/// needed here. On Ubuntu wg-quick runs its hooks under an AppArmor profile
/// that allows SetLinkDNS/SetLinkDomains but denies SetLinkDefaultRoute
/// (journal evidence: apparmor DENIED member="SetLinkDefaultRoute",
/// label="wg-quick"), so that line could only fail. The root helper applies
/// both steps again outside that profile and warns when they fail, which is
/// where a real DNS-routing failure is reported.
/// wg-quick runs hooks with `set -e`: the resolver step is optional, so a
/// missing resolver must not fail the tunnel.
const RESOLVED_UP: &str = "resolvectl domain %i ~. || true";
const RESOLVED_DOWN: &str = "resolvectl revert %i || true";

/// Laptop kill switch (opt-in): while the tunnel is up, drop every outgoing
/// packet that does not leave via the tunnel, except loopback, the hotspot LAN
/// (the WireGuard packets go there) and DHCP. `lan` is the hotspot subnet.
pub struct KillSwitch {
    pub iptables: bool,
    pub lan: String,
}

/// The kill switch as one wg-quick hook command (iptables chain or nft table).
pub fn kill_switch_up(iptables: bool, lan: &str) -> String {
    if iptables {
        // Cleanup of an earlier run may find nothing (|| true). The rules after
        // it are not optional: under wg-quick's `set -e` a failing one aborts
        // the hook, so the tunnel does not come up without its kill switch.
        format!(
            "iptables -D OUTPUT -j JAFNIFY_KS 2>/dev/null || true; \
             iptables -F JAFNIFY_KS 2>/dev/null || true; \
             iptables -X JAFNIFY_KS 2>/dev/null || true; iptables -N JAFNIFY_KS; \
             iptables -I OUTPUT 1 -j JAFNIFY_KS; \
             iptables -A JAFNIFY_KS -o lo -j ACCEPT; \
             iptables -A JAFNIFY_KS -o %i -j ACCEPT; \
             iptables -A JAFNIFY_KS -d {lan} -j ACCEPT; \
             iptables -A JAFNIFY_KS -p udp --dport 67:68 -j ACCEPT; \
             iptables -A JAFNIFY_KS -j DROP"
        )
    } else {
        format!(
            "nft delete table inet jafnify_killswitch 2>/dev/null || true; \
             nft add table inet jafnify_killswitch; \
             nft add chain inet jafnify_killswitch output '{{ type filter hook output priority -10 ; }}'; \
             nft add rule inet jafnify_killswitch output oifname lo accept; \
             nft add rule inet jafnify_killswitch output oifname %i accept; \
             nft add rule inet jafnify_killswitch output ip daddr {lan} accept; \
             nft add rule inet jafnify_killswitch output udp dport {{ 67, 68 }} accept; \
             nft add rule inet jafnify_killswitch output drop"
        )
    }
}

/// Removes the kill switch (the same names as `kill_switch_up`). Safe to run twice.
pub fn kill_switch_down(iptables: bool) -> String {
    if iptables {
        "iptables -D OUTPUT -j JAFNIFY_KS 2>/dev/null || true; \
         iptables -F JAFNIFY_KS 2>/dev/null || true; \
         iptables -X JAFNIFY_KS 2>/dev/null || true"
            .to_string()
    } else {
        "nft delete table inet jafnify_killswitch 2>/dev/null || true".to_string()
    }
}

/// Which tool makes the laptop's inbound guard, picked when the config is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundGuard {
    Iptables,
    Nft,
    /// Neither tool is installed: the tunnel comes up without the guard.
    None,
}

impl InboundGuard {
    pub fn choose(has_iptables: bool, has_nft: bool) -> Self {
        if has_iptables {
            InboundGuard::Iptables
        } else if has_nft {
            InboundGuard::Nft
        } else {
            InboundGuard::None
        }
    }

    fn hooks(self) -> (Option<String>, Option<String>) {
        let (up, down) = match self {
            InboundGuard::Iptables => (INBOUND_GUARD_UP, INBOUND_GUARD_DOWN),
            InboundGuard::Nft => (NFT_GUARD_UP, NFT_GUARD_DOWN),
            InboundGuard::None => return (None, None),
        };
        (Some(up.to_string()), Some(down.to_string()))
    }
}

/// Seconds between keepalive pings. Phone-hotspot NAT drops idle mappings, so
/// the tunnel must keep the peer endpoint alive.
const PERSISTENT_KEEPALIVE: u32 = 25;

#[derive(Debug, Clone, Copy, Default)]
pub struct ConfigGenerator;

impl ConfigGenerator {
    pub fn sharer_config(private_key: &str, joiner_public_key: &str) -> WireGuardConfig {
        WireGuardConfig {
            interface: InterfaceConfig {
                address: SHARER_IP.to_string(),
                private_key: private_key.to_string(),
                dns: None,
                mtu: Some(MTU),
                post_up: None,
                post_down: None,
            },
            peer: PeerConfig {
                public_key: joiner_public_key.to_string(),
                endpoint: None,
                allowed_ips: ALLOWED_IPS.to_string(),
                persistent_keepalive: Some(PERSISTENT_KEEPALIVE),
            },
        }
    }

    pub fn joiner_config(
        private_key: &str,
        sharer_public_key: &str,
        sharer_endpoint: &str,
    ) -> WireGuardConfig {
        Self::joiner_config_with_guard(
            private_key,
            sharer_public_key,
            sharer_endpoint,
            InboundGuard::Iptables,
        )
    }

    pub fn joiner_config_with_guard(
        private_key: &str,
        sharer_public_key: &str,
        sharer_endpoint: &str,
        guard: InboundGuard,
    ) -> WireGuardConfig {
        Self::joiner_config_with_hooks(
            private_key,
            sharer_public_key,
            sharer_endpoint,
            guard,
            false,
        )
    }

    /// Joiner config with its laptop hooks: the inbound guard and, with
    /// systemd-resolved, the DNS routing. One PostUp / PostDown line, run
    /// in this order. The IPv6 guard is not a hook: the root helper
    /// applies it, because wg-quick's AppArmor profile denies reading
    /// /proc/sys/net/ipv6 and `ip -6 route` fails on a kernel booted with
    /// ipv6.disable=1, which under wg-quick's `set -e` would stop the tunnel.
    pub fn joiner_config_with_hooks(
        private_key: &str,
        sharer_public_key: &str,
        sharer_endpoint: &str,
        guard: InboundGuard,
        resolved: bool,
    ) -> WireGuardConfig {
        Self::joiner_config_full(
            private_key,
            sharer_public_key,
            sharer_endpoint,
            guard,
            resolved,
            None,
        )
    }

    /// As `joiner_config_with_hooks`, plus the optional kill switch.
    pub fn joiner_config_full(
        private_key: &str,
        sharer_public_key: &str,
        sharer_endpoint: &str,
        guard: InboundGuard,
        resolved: bool,
        kill: Option<&KillSwitch>,
    ) -> WireGuardConfig {
        let (guard_up, guard_down) = guard.hooks();
        let mut up: Vec<String> = guard_up.into_iter().collect();
        let mut down: Vec<String> = guard_down.into_iter().collect();
        if let Some(ks) = kill {
            up.push(kill_switch_up(ks.iptables, &ks.lan));
            down.push(kill_switch_down(ks.iptables));
        }
        if resolved {
            up.push(RESOLVED_UP.to_string());
            down.push(RESOLVED_DOWN.to_string());
        }
        WireGuardConfig {
            interface: InterfaceConfig {
                address: JOINER_IP.to_string(),
                private_key: private_key.to_string(),
                dns: Some(GATEWAY_DNS_IP.to_string()),
                mtu: Some(MTU),
                post_up: Some(up.join("; ")),
                post_down: Some(down.join("; ")),
            },
            peer: PeerConfig {
                public_key: sharer_public_key.to_string(),
                endpoint: Some(sharer_endpoint.to_string()),
                allowed_ips: ALLOWED_IPS.to_string(),
                persistent_keepalive: Some(PERSISTENT_KEEPALIVE),
            },
        }
    }

    pub fn to_wg_quick_text(&self, cfg: &WireGuardConfig) -> String {
        let mut out = String::new();
        out.push_str("[Interface]\n");
        out.push_str(&format!("Address = {}\n", cfg.interface.address));
        out.push_str(&format!("PrivateKey = {}\n", cfg.interface.private_key));
        if let Some(dns) = &cfg.interface.dns {
            out.push_str(&format!("DNS = {dns}\n"));
        }
        if let Some(mtu) = cfg.interface.mtu {
            out.push_str(&format!("MTU = {mtu}\n"));
        }
        if let Some(up) = &cfg.interface.post_up {
            out.push_str(&format!("PostUp = {up}\n"));
        }
        if let Some(down) = &cfg.interface.post_down {
            out.push_str(&format!("PostDown = {down}\n"));
        }
        out.push_str("\n[Peer]\n");
        out.push_str(&format!("PublicKey = {}\n", cfg.peer.public_key));
        if let Some(ep) = &cfg.peer.endpoint {
            out.push_str(&format!("Endpoint = {ep}\n"));
        }
        if let Some(pk) = cfg.peer.persistent_keepalive {
            out.push_str(&format!("PersistentKeepalive = {pk}\n"));
        }
        out.push_str(&format!("AllowedIPs = {}\n", cfg.peer.allowed_ips));
        out
    }

    pub fn parse(text: &str) -> Result<WireGuardConfig, ConfigError> {
        let mut interface = None;
        let mut peer = None;
        let mut section = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_string();
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| ConfigError::Parse(format!("malformed line: {line}")))?;
            let key = key.trim();
            let value = value.trim();
            // wg-quick spells keys CamelCase (PrivateKey); older Jafnify text
            // used snake_case. Accept both.
            let norm = key.replace('_', "").to_ascii_lowercase();
            match (section.as_str(), norm.as_str()) {
                ("Interface", "address") => {
                    let cfg = interface.get_or_insert_with(interface_placeholder);
                    cfg.address = value.to_string();
                }
                ("Interface", "privatekey") => {
                    interface
                        .get_or_insert_with(interface_placeholder)
                        .private_key = value.to_string();
                }
                ("Interface", "dns") => {
                    interface.get_or_insert_with(interface_placeholder).dns =
                        Some(value.to_string());
                }
                ("Interface", "postup") => {
                    interface.get_or_insert_with(interface_placeholder).post_up =
                        Some(value.to_string());
                }
                ("Interface", "postdown") => {
                    interface
                        .get_or_insert_with(interface_placeholder)
                        .post_down = Some(value.to_string());
                }
                ("Interface", "mtu") => {
                    let mtu: u16 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidField(format!("mtu: {value}")))?;
                    interface.get_or_insert_with(interface_placeholder).mtu = Some(mtu);
                }
                ("Peer", "publickey") => {
                    let cfg = peer.get_or_insert_with(peer_placeholder);
                    cfg.public_key = value.to_string();
                }
                ("Peer", "endpoint") => {
                    peer.get_or_insert_with(peer_placeholder).endpoint = Some(value.to_string());
                }
                ("Peer", "persistentkeepalive") => {
                    let keepalive: u32 = value.parse().map_err(|_| {
                        ConfigError::InvalidField(format!("persistent_keepalive: {value}"))
                    })?;
                    peer.get_or_insert_with(peer_placeholder)
                        .persistent_keepalive = Some(keepalive);
                }
                ("Peer", "allowedips") => {
                    peer.get_or_insert_with(peer_placeholder).allowed_ips = value.to_string();
                }
                _ => {
                    return Err(ConfigError::Parse(format!(
                        "unknown key {key} in [{section}]"
                    )))
                }
            }
        }
        let interface =
            interface.ok_or_else(|| ConfigError::Parse("missing [Interface]".into()))?;
        let peer = peer.ok_or_else(|| ConfigError::Parse("missing [Peer]".into()))?;
        if interface.address.is_empty() {
            return Err(ConfigError::InvalidField(
                "interface address is empty".into(),
            ));
        }
        if interface.private_key.is_empty() {
            return Err(ConfigError::InvalidField(
                "interface private_key is empty".into(),
            ));
        }
        if peer.public_key.is_empty() {
            return Err(ConfigError::InvalidField("peer public_key is empty".into()));
        }
        if peer.allowed_ips.is_empty() {
            return Err(ConfigError::InvalidField(
                "peer allowed_ips is empty".into(),
            ));
        }
        Ok(WireGuardConfig { interface, peer })
    }
}

fn interface_placeholder() -> InterfaceConfig {
    InterfaceConfig {
        address: String::new(),
        private_key: String::new(),
        dns: None,
        mtu: None,
        post_up: None,
        post_down: None,
    }
}

fn peer_placeholder() -> PeerConfig {
    PeerConfig {
        public_key: String::new(),
        endpoint: None,
        allowed_ips: String::new(),
        persistent_keepalive: None,
    }
}

/// The config the app and the root helper must agree on.
pub fn build_config(
    role: Role,
    private_key: &str,
    peer_public_key: &str,
    peer_endpoint: Option<SocketAddr>,
    guard: InboundGuard,
    resolved: bool,
    kill: Option<&KillSwitch>,
) -> WireGuardConfig {
    match role {
        Role::Sharer => ConfigGenerator::sharer_config(private_key, peer_public_key),
        Role::Joiner => {
            let endpoint = peer_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("127.0.0.1:{LISTEN_PORT}"));
            ConfigGenerator::joiner_config_full(
                private_key,
                peer_public_key,
                &endpoint,
                guard,
                resolved,
                kill,
            )
        }
    }
}
