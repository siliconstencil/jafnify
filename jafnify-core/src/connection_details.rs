//! The connection details both apps show, in the core's own words.
//!
//! The platform fills what it knows: how the peer is reached, how DNS is
//! done here and the peer's address. The labels and the wording live here so
//! the phone and the laptop say the same thing.

use crate::snapshot::PeerPath;
use crate::MTU;

/// Which side of the pair this device is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSide {
    /// The phone; the peer is the laptop.
    Phone,
    /// The laptop; the peer is the phone.
    Laptop,
}

impl DeviceSide {
    /// The label for the peer's address on this side.
    pub fn peer_address_label(self) -> &'static str {
        match self {
            DeviceSide::Phone => "Laptop address",
            DeviceSide::Laptop => "Phone address",
        }
    }
}

/// How DNS lookups are done on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsMode {
    /// The lookups go through the tunnel.
    ThroughTunnel,
    /// The system resolver is used; Jafnify does not change it.
    SystemResolver,
}

impl DnsMode {
    /// The value shown next to the "DNS" label.
    pub fn text(self) -> &'static str {
        match self {
            DnsMode::ThroughTunnel => "through the tunnel",
            DnsMode::SystemResolver => "system resolver (not changed)",
        }
    }
}

/// The details every app shows, in a fixed order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionDetails {
    /// How the peer is reached, in the core's words; none when unknown.
    pub path: Option<String>,
    /// The link health verdict; shown only when it says more than the path.
    pub verdict: Option<String>,
    /// How DNS is done on this device.
    pub dns: DnsMode,
    /// The peer's address as "ip:port"; none when unknown.
    pub endpoint: Option<String>,
}

impl ConnectionDetails {
    /// The rows in the order the apps show them.
    pub fn rows(&self, side: DeviceSide) -> Vec<(String, String)> {
        let mut rows = vec![
            (
                "Path".to_string(),
                self.path.clone().unwrap_or_else(|| "unknown".to_string()),
            ),
            ("MTU".to_string(), MTU.to_string()),
            ("DNS".to_string(), self.dns.text().to_string()),
            (
                side.peer_address_label().to_string(),
                self.endpoint
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            ),
        ];
        if let Some(verdict) = &self.verdict {
            if Some(verdict.as_str()) != self.path.as_deref() {
                rows.push(("Link health".to_string(), verdict.clone()));
            }
        }
        rows
    }

    /// The rows as "label\tvalue" lines for the phone bridge.
    /// The rows as one readable line each: "Path: Direct over hotspot".
    pub fn text_lines(&self, side: DeviceSide) -> String {
        self.rows(side)
            .iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// The rows as "label\tvalue" lines, for callers that split the pair.
    pub fn lines(&self, side: DeviceSide) -> String {
        self.rows(side)
            .iter()
            .map(|(label, value)| format!("{label}\t{value}"))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// The path text for a known path; none when the platform cannot tell.
pub fn path_text(path: PeerPath) -> Option<String> {
    path.text().map(str::to_string)
}
