//! The host hints, parsed from tool output and rendered in the core wording.
//!
//! Only an explicit positive counts: the tools are often missing, the
//! network is often Ethernet, and a wrong hint is worse than no hint. No
//! SSID, no BSSID and no address ever reaches the text; the interface name
//! is allowed.

use crate::firewall::{FirewallKind, BLOCKED_STATUS};

/// One thing the host can explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostHint {
    /// The active Wi-Fi connection saves power and can stall the tunnel.
    WifiPowerSaving { iface: String },
    /// The connected network name is served by more than one access point.
    SeveralAccessPoints,
    /// The host firewall drops the discovery messages.
    FirewallBlocked {
        kind: FirewallKind,
        iface: Option<String>,
    },
}

impl HostHint {
    /// The short line for the Details block.
    pub fn title(&self) -> String {
        match self {
            HostHint::WifiPowerSaving { iface } => {
                format!("Wi-Fi power saving is on for {iface}.")
            }
            HostHint::SeveralAccessPoints => {
                "This network name is seen from several access points.".to_string()
            }
            HostHint::FirewallBlocked { .. } => BLOCKED_STATUS.to_string(),
        }
    }

    /// The explanation under the line; the firewall text is the existing one.
    pub fn help(&self) -> String {
        match self {
            HostHint::WifiPowerSaving { .. } => "This can add short stalls. Jafnify does not \
                change it; turn it off in your network settings if stalls persist."
                .to_string(),
            HostHint::SeveralAccessPoints => "Roaming between them can cause short stalls. \
                Jafnify does not change your network settings."
                .to_string(),
            HostHint::FirewallBlocked { kind, iface } => {
                crate::firewall::blocked_help(*kind, iface.as_deref())
            }
        }
    }
}

/// The firewall hint for an existing verdict; only a block is a hint.
pub fn firewall_hint(kind: FirewallKind, iface: Option<&str>) -> HostHint {
    HostHint::FirewallBlocked {
        kind,
        iface: iface.map(str::to_string),
    }
}

/// `nmcli -t -f DEVICE,TYPE connection show --active` prints one line per
/// active connection, for example `wlo1:802-11-wireless`, `jafn0:wireguard`.
/// The devices of the Wi-Fi connections, in order. (Checked against a real
/// NetworkManager: the TYPE column is `802-11-wireless`; a
/// `802-11-wireless.powersave` column is rejected in this list mode.)
pub fn parse_wifi_devices(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let fields = split_terse(line.trim());
            if fields.len() < 2 {
                return None;
            }
            let kind = fields[1].trim();
            let wifi = kind == "802-11-wireless" || kind.eq_ignore_ascii_case("wifi");
            let device = fields[0].trim();
            (wifi && !device.is_empty()).then(|| device.to_string())
        })
        .collect()
}

/// `iw dev <iface> get power_save` prints `Power save: on` or
/// `Power save: off`: the driver's real state. Only an explicit `on` counts.
pub fn parse_iw_power_save(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("power save: on"))
}

/// `nmcli -t -f IN-USE,BSSID,SSID device wifi list --rescan no` prints one
/// line per network: `*:AA\:BB\:CC\:DD\:EE\:FF:Home`, with a space instead of
/// `*` for the others (checked against a real NetworkManager).
/// The connected network is the one marked `*` (older builds: `yes`); two
/// or more distinct access points on that network name are the hint. A hidden or missing network is not.
pub fn parse_access_points(output: &str) -> bool {
    let mut rows: Vec<(bool, String, String)> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields = split_terse(line);
        if fields.len() < 3 {
            continue;
        }
        rows.push((
            matches!(fields[0].trim(), "*") || fields[0].eq_ignore_ascii_case("yes"),
            fields[1].clone(),
            fields[2].clone(),
        ));
    }
    let Some((_, _, connected)) = rows.iter().find(|(in_use, _, _)| *in_use) else {
        return false;
    };
    if connected.is_empty() {
        return false;
    }
    let mut distinct: Vec<&str> = Vec::new();
    for (_, bssid, ssid) in &rows {
        if ssid != connected {
            continue;
        }
        if !distinct.contains(&bssid.as_str()) {
            distinct.push(bssid);
        }
    }
    distinct.len() >= 2
}

/// Split a terse nmcli line: fields are separated by `:`, a `:` inside a
/// value is written as `\:`.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(':') => current.push(':'),
                Some(other) => {
                    current.push('\\');
                    current.push(other);
                }
                None => current.push('\\'),
            },
            ':' => fields.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}
