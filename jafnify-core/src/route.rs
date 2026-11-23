//! Default IPv4 gateways of the main routing table, from `/proc/net/route`
//! text. A laptop on a hotspot unicasts discovery frames to the gateway, which
//! is the phone. It also holds the pure choice of the addresses a joiner
//! searches on: the network that leads to the saved peer comes first, and the
//! other networks stay as fallbacks.

use std::net::Ipv4Addr;

use crate::probe::{network, LocalIface};

/// The gateway of each listed interface's default route. Routes in other
/// tables (wg-quick's 51820) are not in `/proc/net/route`, and a default with
/// no gateway (a tunnel) gives no target.
pub fn gateways_for(table: &str, ifaces: &[&str]) -> Vec<Ipv4Addr> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 || f[1] != "00000000" || f[7] != "00000000" || f[2] == "00000000" {
                return None;
            }
            if !ifaces.contains(&f[0]) {
                return None;
            }
            let raw = u32::from_str_radix(f[2], 16).ok()?;
            Some(Ipv4Addr::from(raw.to_ne_bytes()))
        })
        .collect()
}

/// Container bridges and veth pairs are not a LAN the phone can be on.
pub fn is_container_iface(name: &str) -> bool {
    name == "docker0"
        || name.starts_with("br-")
        || name.starts_with("veth")
        || name.starts_with("virbr")
}

/// Whether `iface`'s own subnet holds `addr`. The subnet comes from the
/// interface's prefix, the same rule the probe uses to name a source.
pub fn subnet_holds(iface: &LocalIface, addr: Ipv4Addr) -> bool {
    network(iface.ip, iface.prefix) == network(addr, iface.prefix)
}

/// The addresses a joiner searches on, in the order to try them: every up
/// private LAN address that is not a container bridge, with the address whose
/// subnet holds the saved peer's last address first. The order is a
/// preference, never a filter: a stale peer hint must not keep a real network
/// out of the list, and the other addresses stay reachable as fallbacks.
///
/// A host with Ethernet, docker0, a second VPN or a VM management NIC has more
/// than one private address. Searching only the one that exists while the
/// hotspot is away binds the search to a network the phone is not on; ordering
/// by the peer's subnet makes the hotspot address the first to be tried when
/// it returns.
pub fn joiner_search_ifaces(ifaces: &[LocalIface], peer: Option<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let usable: Vec<&LocalIface> = ifaces
        .iter()
        .filter(|i| i.ip.is_private() && !is_container_iface(&i.name))
        .collect();
    let preferred = |i: &LocalIface| peer.is_some_and(|p| subnet_holds(i, p));
    let mut out: Vec<Ipv4Addr> = Vec::with_capacity(usable.len());
    for i in usable.iter().filter(|i| preferred(i)) {
        if !out.contains(&i.ip) {
            out.push(i.ip);
        }
    }
    for i in usable.iter().filter(|i| !preferred(i)) {
        if !out.contains(&i.ip) {
            out.push(i.ip);
        }
    }
    out
}
