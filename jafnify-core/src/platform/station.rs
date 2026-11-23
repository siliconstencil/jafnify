//! The laptop's Wi-Fi station reading: the source trait and the pure mapping.
//!
//! A station reading is numbers only. The Windows reader never stores or
//! returns the BSSID/MAC it also receives. The Linux reader lives in the
//! frontend and implements the same [`StationSource`] trait.

use crate::wifi_range::StationSample;

/// A source of station readings, injected for tests.
pub trait StationSource {
    /// The reading of `iface` now, or None when it is not a Wi-Fi link.
    fn read(&mut self, iface: &str) -> Option<StationSample>;
}

/// Windows reports signal as a 0-100 quality, not in dBm. The documented
/// mapping is `dBm = quality / 2 - 100`; values above 100 are clamped.
pub fn rssi_dbm_from_quality(quality: u32) -> i32 {
    (quality.min(100) / 2) as i32 - 100
}

/// `WLAN_ASSOCIATION_ATTRIBUTES.ulRxRate`/`ulTxRate` are in kbit/s; the core
/// station sample stores 100 kbit/s units.
pub fn rate_100k_from_kbps(kbps: u32) -> u32 {
    kbps / 100
}

/// Map the WLAN connection attributes to the core sample. Signal quality and
/// both rates come from `WLAN_ASSOCIATION_ATTRIBUTES`; the sample has one rate
/// field, so the transmit rate is the one kept.
pub fn sample_from_link(quality: u32, rx_kbps: u32, tx_kbps: u32) -> StationSample {
    let _ = rx_kbps;
    StationSample {
        signal_dbm: Some(rssi_dbm_from_quality(quality)),
        signal_avg_dbm: None,
        tx_rate_100k: Some(rate_100k_from_kbps(tx_kbps)),
        tx_retries: None,
        tx_failed: None,
        beacon_loss: None,
    }
}

/// Format a Windows GUID as the usual `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
pub fn guid_string(data1: u32, data2: u16, data3: u16, data4: &[u8; 8]) -> String {
    format!(
        "{{{data1:08X}-{data2:04X}-{data3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        data4[0], data4[1], data4[2], data4[3], data4[4], data4[5], data4[6], data4[7]
    )
}

/// True when `requested` is the interface description or its GUID, with or
/// without the braces. Case is ignored, like the uplink adapter names.
pub fn interface_matches(
    requested: &str,
    description: &str,
    data1: u32,
    data2: u16,
    data3: u16,
    data4: &[u8; 8],
) -> bool {
    if requested.eq_ignore_ascii_case(description) {
        return true;
    }
    let guid = guid_string(data1, data2, data3, data4);
    let requested = requested.trim_matches(|c| c == '{' || c == '}');
    let guid = guid.trim_matches(|c| c == '{' || c == '}');
    requested.eq_ignore_ascii_case(guid)
}

/// The Windows WLAN reader. It opens one handle and asks the WLAN API for the
/// current connection of the interface whose description or GUID matches
/// `iface`. Only the signal quality and the two rates are read; the BSSID/MAC
/// in the same struct is never copied, stored or returned.
#[cfg(windows)]
pub struct WlanStation {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WlanStation {
    /// Open the WLAN client handle. `None` when the WLAN service is absent or
    /// the API rejects the version.
    pub fn new() -> Option<Self> {
        use windows_sys::Win32::NetworkManagement::WiFi::{WlanOpenHandle, WLAN_API_VERSION_2_0};

        let mut negotiated = 0u32;
        let mut handle = 0;
        // SAFETY: both out pointers are live locals; the reserved argument is
        // documented as NULL.
        let result = unsafe {
            WlanOpenHandle(
                WLAN_API_VERSION_2_0,
                std::ptr::null(),
                &mut negotiated,
                &mut handle,
            )
        };
        (result == 0 && handle != 0).then_some(Self { handle })
    }

    /// The current station reading of `iface`, or None when it is not a
    /// connected Wi-Fi interface.
    pub fn read(&mut self, iface: &str) -> Option<StationSample> {
        use windows_sys::Win32::NetworkManagement::WiFi::{
            wlan_interface_state_connected, wlan_intf_opcode_current_connection,
            wlan_opcode_value_type_invalid, WlanEnumInterfaces, WlanFreeMemory, WlanQueryInterface,
            WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
        };

        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: `list` is a valid out pointer; the handle came from
        // WlanOpenHandle and the reserved argument is NULL.
        let result = unsafe { WlanEnumInterfaces(self.handle, std::ptr::null(), &mut list) };
        if result != 0 || list.is_null() {
            return None;
        }
        let mut found: Option<WLAN_INTERFACE_INFO> = None;
        // SAFETY: a successful WlanEnumInterfaces fills a list whose
        // InterfaceInfo array holds dwNumberOfItems entries.
        unsafe {
            let count = (*list).dwNumberOfItems as usize;
            let first = (*list).InterfaceInfo.as_ptr();
            for info in std::slice::from_raw_parts(first, count) {
                let name = crate::ui::time::wide_name_to_string(&info.strInterfaceDescription);
                let guid = info.InterfaceGuid;
                if interface_matches(
                    iface,
                    &name,
                    guid.data1,
                    guid.data2,
                    guid.data3,
                    &guid.data4,
                ) {
                    found = Some(*info);
                    break;
                }
            }
            WlanFreeMemory(list as *const std::ffi::c_void);
        }
        let info = found?;

        let mut size = 0u32;
        let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut value_type = 0;
        // SAFETY: the interface GUID and handle are live; `size`, `data` and
        // `value_type` are valid out pointers.
        let result = unsafe {
            WlanQueryInterface(
                self.handle,
                &info.InterfaceGuid,
                wlan_intf_opcode_current_connection,
                std::ptr::null(),
                &mut size,
                &mut data,
                &mut value_type,
            )
        };
        if result != 0 || data.is_null() {
            if !data.is_null() {
                // SAFETY: the API may have allocated before failing; free the
                // buffer it handed back without reading it.
                unsafe { WlanFreeMemory(data) };
            }
            return None;
        }
        if (size as usize) < std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>()
            || value_type == wlan_opcode_value_type_invalid
        {
            // SAFETY: the pointer is non-null and was allocated by the API;
            // free it without casting a short buffer to the struct.
            unsafe { WlanFreeMemory(data) };
            return None;
        }
        // SAFETY: on success the API returns a WLAN_CONNECTION_ATTRIBUTES and
        // its size was checked above; only scalar association fields are read.
        // The BSSID is deliberately never touched. The buffer is freed once.
        unsafe {
            let attributes = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
            let sample = if attributes.isState == wlan_interface_state_connected {
                let association = &attributes.wlanAssociationAttributes;
                Some(sample_from_link(
                    association.wlanSignalQuality,
                    association.ulRxRate,
                    association.ulTxRate,
                ))
            } else {
                None
            };
            WlanFreeMemory(data);
            sample
        }
    }
}

#[cfg(windows)]
impl StationSource for WlanStation {
    fn read(&mut self, iface: &str) -> Option<StationSample> {
        WlanStation::read(self, iface)
    }
}

#[cfg(windows)]
impl Drop for WlanStation {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by WlanOpenHandle and is closed once.
        unsafe {
            windows_sys::Win32::NetworkManagement::WiFi::WlanCloseHandle(
                self.handle,
                std::ptr::null(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signal_quality_maps_to_the_documented_dbm_range() {
        // Recorded from the Windows docs: quality is 0-100 and the mapping is
        // `dBm = quality / 2 - 100`.
        assert_eq!(rssi_dbm_from_quality(0), -100);
        assert_eq!(rssi_dbm_from_quality(50), -75);
        assert_eq!(rssi_dbm_from_quality(74), -63);
        assert_eq!(rssi_dbm_from_quality(100), -50);
        assert_eq!(rssi_dbm_from_quality(140), -50);
    }

    #[test]
    fn the_kbit_rates_become_hundred_kbit_units() {
        // Recorded from `netsh wlan show interfaces`: 144.4 Mbps transmit.
        assert_eq!(rate_100k_from_kbps(144_400), 1_444);
        assert_eq!(rate_100k_from_kbps(130_000), 1_300);
        assert_eq!(rate_100k_from_kbps(99), 0);
    }

    #[test]
    fn an_interface_matches_its_description_or_its_guid() {
        let data4 = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        assert!(interface_matches(
            "Intel(R) Wi-Fi 6 AX201",
            "Intel(R) Wi-Fi 6 AX201",
            0x1234_5678,
            0x9ABC,
            0xDEF0,
            &data4,
        ));
        assert!(interface_matches(
            "{12345678-9ABC-DEF0-1234-56789ABCDEF0}",
            "Intel(R) Wi-Fi 6 AX201",
            0x1234_5678,
            0x9ABC,
            0xDEF0,
            &data4,
        ));
        assert!(interface_matches(
            "12345678-9abc-def0-1234-56789abcdef0",
            "Intel(R) Wi-Fi 6 AX201",
            0x1234_5678,
            0x9ABC,
            0xDEF0,
            &data4,
        ));
        assert!(!interface_matches(
            "Ethernet",
            "Intel(R) Wi-Fi 6 AX201",
            0x1234_5678,
            0x9ABC,
            0xDEF0,
            &data4,
        ));
    }

    #[test]
    fn a_link_sample_keeps_the_level_and_the_transmit_rate() {
        // Recorded from `netsh wlan show interfaces`: 74%, rx 130 Mbps,
        // tx 144.4 Mbps. The sample has no receive-rate field.
        let sample = sample_from_link(74, 130_000, 144_400);
        assert_eq!(sample.level(), Some(-63));
        assert_eq!(sample.tx_rate_100k, Some(1_444));
        assert_eq!(sample.tx_retries, None);
        assert_eq!(sample.beacon_loss, None);
    }
}
