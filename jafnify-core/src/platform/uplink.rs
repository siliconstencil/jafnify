//! The uplink interface rules and (on Windows) the IP Helper reader.
//!
//! The edge detector itself is a pure state machine; the platform supplies a
//! carrier bit and whether the interface has a usable IPv4 address.

use std::net::Ipv4Addr;

/// An address that can carry the peer route: not loopback and not the
/// unspecified address.
pub fn usable_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified()
}

/// Map one carrier sample and the address result to the uplink state. An
/// unreadable carrier is unknown; a down carrier or no non-loopback IPv4 is
/// down; only an up carrier with a non-loopback IPv4 is up.
pub fn state_from(carrier: Option<bool>, has_ipv4: bool) -> Option<bool> {
    match carrier {
        None => None,
        Some(true) => Some(has_ipv4),
        Some(false) => Some(false),
    }
}

/// `IP_ADAPTER_ADDRESSES_LH.OperStatus` is up only for `IfOperStatusUp` (1).
pub fn carrier_from_oper_status(status: i32) -> bool {
    status == 1
}

/// True when the requested interface name matches any adapter name spelling.
/// Windows exposes a GUID adapter name, a friendly name and a description;
/// the caller may pass any of them.
pub fn name_matches(requested: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(requested))
}

/// The IPv4 address from `SOCKADDR_IN.sin_addr.S_un.S_addr`. The value keeps
/// the network-order bytes in memory, so the native byte view is the address.
pub fn ipv4_from_s_addr(raw: u32) -> Ipv4Addr {
    Ipv4Addr::from(raw.to_ne_bytes())
}

/// The Windows uplink reader: `GetAdaptersAddresses` for the carrier state
/// and a non-loopback IPv4 address. `None` when no adapter matches `iface`
/// (the caller may pass the adapter name, friendly name or description).
#[cfg(windows)]
pub fn read_uplink_state(iface: &str) -> Option<bool> {
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    let mut size = 16 * 1024u32;
    // u64 keeps the buffer aligned for the adapter structures.
    let mut buffer: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
    let mut attempts = 0;
    loop {
        if attempts == 4 {
            return None;
        }
        attempts += 1;
        let mut capacity = (buffer.len() * 8) as u32;
        // SAFETY: the buffer is writable for `capacity` bytes and the size
        // pointer is live. The reserved argument is NULL.
        let result = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
                std::ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut capacity,
            )
        };
        if result == ERROR_BUFFER_OVERFLOW {
            if capacity as usize <= buffer.len() * 8 {
                return None;
            }
            size = capacity;
            buffer.resize((size as usize).div_ceil(8), 0);
            continue;
        }
        if result != NO_ERROR {
            return None;
        }
        break;
    }

    // SAFETY: a successful call filled the buffer with a linked list of
    // IP_ADAPTER_ADDRESSES_LH values that stay inside it.
    let mut cursor = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !cursor.is_null() {
        // SAFETY: each Next pointer stays inside the buffer filled above.
        let adapter = unsafe { &*cursor };
        if adapter_name_matches(adapter, iface) {
            let carrier = carrier_from_oper_status(adapter.OperStatus);
            return state_from(Some(carrier), has_usable_ipv4(adapter));
        }
        cursor = adapter.Next;
    }
    None
}

#[cfg(windows)]
fn adapter_name_matches(
    adapter: &windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH,
    requested: &str,
) -> bool {
    let ansi = if adapter.AdapterName.is_null() {
        None
    } else {
        // SAFETY: AdapterName is a NUL-terminated ANSI string on success.
        Some(unsafe {
            std::ffi::CStr::from_ptr(adapter.AdapterName as *const i8)
                .to_string_lossy()
                .into_owned()
        })
    };
    let friendly = wide_ptr_name(adapter.FriendlyName);
    let description = wide_ptr_name(adapter.Description);
    let mut names: Vec<&str> = Vec::with_capacity(3);
    if let Some(name) = &ansi {
        names.push(name);
    }
    if let Some(name) = &friendly {
        names.push(name);
    }
    if let Some(name) = &description {
        names.push(name);
    }
    name_matches(requested, &names)
}

#[cfg(windows)]
fn wide_ptr_name(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the pointer is a NUL-terminated UTF-16 string owned by the
    // adapter buffer. The scan is capped so a damaged list cannot run away.
    unsafe {
        let mut len = 0usize;
        while len < 1024 && *ptr.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(
            ptr, len,
        )))
    }
}

#[cfg(windows)]
fn has_usable_ipv4(
    adapter: &windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH,
) -> bool {
    use windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_UNICAST_ADDRESS_LH;

    let mut cursor = adapter.FirstUnicastAddress;
    while !cursor.is_null() {
        // SAFETY: the unicast list is a linked list inside the same buffer.
        let entry: &IP_ADAPTER_UNICAST_ADDRESS_LH = unsafe { &*cursor };
        if let Some(ip) = unicast_ipv4(&entry.Address) {
            if usable_ipv4(ip) {
                return true;
            }
        }
        cursor = entry.Next;
    }
    false
}

#[cfg(windows)]
fn unicast_ipv4(
    address: &windows_sys::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Option<Ipv4Addr> {
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    if address.lpSockaddr.is_null()
        || address.iSockaddrLength < std::mem::size_of::<SOCKADDR_IN>() as i32
    {
        return None;
    }
    // SAFETY: the length check above covers a full SOCKADDR_IN, and the
    // family check below bounds the cast to an IPv4 sockaddr.
    unsafe {
        let sockaddr = &*address.lpSockaddr;
        if sockaddr.sa_family != AF_INET {
            return None;
        }
        let sin = &*(address.lpSockaddr as *const SOCKADDR_IN);
        Some(ipv4_from_s_addr(sin.sin_addr.S_un.S_addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usable_address_is_neither_loopback_nor_unspecified() {
        assert!(usable_ipv4(Ipv4Addr::new(192, 168, 1, 5)));
        assert!(!usable_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!usable_ipv4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn the_carrier_and_address_map_to_the_uplink_state() {
        assert_eq!(state_from(None, true), None);
        assert_eq!(state_from(Some(false), true), Some(false));
        assert_eq!(state_from(Some(true), false), Some(false));
        assert_eq!(state_from(Some(true), true), Some(true));
    }

    #[test]
    fn the_oper_status_is_up_only_for_if_oper_status_up() {
        // Recorded from the Windows docs: IfOperStatusUp = 1, Down = 2,
        // Testing = 3, Dormant = 5, NotPresent = 6, LowerLayerDown = 7.
        assert!(carrier_from_oper_status(1));
        for status in [2, 3, 5, 6, 7] {
            assert!(!carrier_from_oper_status(status), "status {status}");
        }
    }

    #[test]
    fn the_sockaddr_bytes_are_the_ipv4_address() {
        // Recorded from the Winsock SOCKADDR_IN contract: S_addr keeps the
        // address in network order in memory.
        let raw = u32::from_ne_bytes([192, 168, 1, 5]);
        assert_eq!(ipv4_from_s_addr(raw), Ipv4Addr::new(192, 168, 1, 5));
    }

    #[test]
    fn an_adapter_matches_any_of_its_names_case_insensitively() {
        let names = ["{GUID}", "Ethernet", "Intel(R) Wi-Fi 6 AX201"];
        assert!(name_matches("ethernet", &names));
        assert!(name_matches("Intel(R) Wi-Fi 6 AX201", &names));
        assert!(!name_matches("Wi-Fi", &names));
    }
}
