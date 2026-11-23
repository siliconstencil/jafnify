//! The uplink interface carrying the peer route, for an immediate link-loss
//! indication while Connected.
//!
//! [`LinkWatch`] turns a stream of carrier/IP samples into edge events.
//! [`read_uplink_state_with`] samples the carrier through an injected sysfs
//! root and checks addresses with `getifaddrs`, so no `ip` process runs.

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

/// A transition of the uplink state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkChange {
    Lost,
    Back,
}

/// Edge detector around one interface. The first observation of down reports
/// [`LinkChange::Lost`]; the first observation of up reports nothing.
#[derive(Debug, Clone)]
pub struct LinkWatch {
    iface: String,
    up: Option<bool>,
}

impl LinkWatch {
    pub fn new(iface: String) -> Self {
        Self { iface, up: None }
    }

    /// Feed one sample. Repeated equal states report nothing.
    pub fn observe(&mut self, up: bool) -> Option<LinkChange> {
        let previous = self.up;
        self.up = Some(up);
        match (previous, up) {
            (None, false) | (Some(true), false) => Some(LinkChange::Lost),
            (Some(false), true) => Some(LinkChange::Back),
            _ => None,
        }
    }

    /// The last observed state was down.
    pub fn lost(&self) -> bool {
        self.up == Some(false)
    }

    pub fn iface(&self) -> &str {
        &self.iface
    }
}

/// The interface after the `dev` token in `ip -4 route get` output.
pub fn parse_route_dev(route_get_output: &str) -> Option<String> {
    let words: Vec<&str> = route_get_output.split_whitespace().collect();
    let index = words.iter().position(|word| *word == "dev")?;
    words.get(index + 1).map(|dev| dev.to_string())
}

/// The source address after the `src` token in `ip -4 route get` output.
pub fn parse_route_src(route_get_output: &str) -> Option<IpAddr> {
    let words: Vec<&str> = route_get_output.split_whitespace().collect();
    let index = words.iter().position(|word| *word == "src")?;
    words.get(index + 1)?.parse().ok()
}

/// The first word of a sysfs `operstate` file ("up", "down", ...), trimmed.
/// An empty or whitespace-only file has no state.
pub fn parse_operstate(text: &str) -> Option<&str> {
    text.split_whitespace().next()
}

/// The uplink device from `ip -4 route get`, rejecting loopback and tunnel
/// devices. An unmarked route can otherwise fall through to `jafn0` under
/// wg-quick policy routing.
pub fn parse_uplink_route(output: &str) -> Option<String> {
    let dev = parse_route_dev(output)?;
    let tunnel = dev.starts_with("wg") || dev.starts_with("jafn") || dev.starts_with("tun");
    (!tunnel && dev != "lo").then_some(dev)
}

pub use jafnify_core::platform::uplink::{state_from, usable_ipv4};

/// Sample `iface` through `sysfs_root` (normally `/sys/class/net`). `Some(true)`
/// only when the carrier file is `1` and a non-loopback IPv4 address exists;
/// `Some(false)` for a down carrier or no address; `None` when the carrier
/// cannot be read at all.
pub fn read_uplink_state_with(iface: &str, sysfs_root: &Path) -> Option<bool> {
    let carrier = std::fs::read_to_string(sysfs_root.join(iface).join("carrier")).ok()?;
    let up = carrier.trim() == "1";
    let has_ipv4 = up && has_non_loopback_ipv4(iface);
    state_from(Some(up), has_ipv4)
}

/// Whether `iface` has an IPv4 address other than loopback, from the kernel's
/// interface list. This is the same source `ip addr` reads, with no process.
fn has_non_loopback_ipv4(iface: &str) -> bool {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return false;
    }
    let mut found = false;
    let mut cursor = list;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null() {
            let family = unsafe { (*entry.ifa_addr).sa_family as i32 };
            if family == libc::AF_INET {
                if entry.ifa_name.is_null() {
                    cursor = entry.ifa_next;
                    continue;
                }
                let name = unsafe { CStr::from_ptr(entry.ifa_name) };
                if name.to_bytes() == iface.as_bytes() {
                    let addr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                    if usable_ipv4(ip) {
                        found = true;
                        break;
                    }
                }
            }
        }
        cursor = entry.ifa_next;
    }
    unsafe { libc::freeifaddrs(list) };
    found
}
