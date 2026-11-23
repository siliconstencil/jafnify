//! Kernel WireGuard backend: parse `wg show <iface> all` output and wrap
//! status queries. State comes from the kernel, never from an in-memory
//! copy.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum WgError {
    #[error("io error running wg: {0}")]
    Io(#[from] std::io::Error),
    #[error("unparseable wg output: {0}")]
    Parse(String),
    #[error("interface not found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WgPeerStatus {
    pub public_key: String,
    pub endpoint: Option<String>,
    pub latest_handshake: Option<String>,
    pub transfer_rx: Option<String>,
    pub transfer_tx: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WgStatus {
    pub interface_up: bool,
    pub public_key: Option<String>,
    pub listen_port: Option<u16>,
    pub peers: Vec<WgPeerStatus>,
}

pub struct WgBackend {
    pub wg_bin: PathBuf,
}

impl Default for WgBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WgBackend {
    pub fn new() -> Self {
        Self {
            wg_bin: PathBuf::from("wg"),
        }
    }

    pub fn status(&self, iface: &str) -> Result<WgStatus, WgError> {
        let mut cmd = Command::new(&self.wg_bin);
        cmd.args(["show", iface, "all"]);
        let out = crate::exec::output(cmd)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("Unable to access interface") {
                return Err(WgError::NotFound);
            }
            return Err(WgError::Parse(stderr.into_owned()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(parse_wg_show_all(&text))
    }
}

/// The CLI's `interface:` verdict. `wg show` needs root, so a failed status
/// read is not evidence that the interface is down; the sysfs entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceState {
    Up,
    Down,
}

pub fn interface_state(wg_reported_up: bool, present: bool) -> InterfaceState {
    if wg_reported_up || present {
        InterfaceState::Up
    } else {
        InterfaceState::Down
    }
}

/// Whether the interface exists, read from sysfs without root.
pub fn interface_present_at(root: &Path, iface: &str) -> bool {
    root.join(iface).exists()
}

/// Parse `wg show <iface> all` output into a `WgStatus`.
pub fn parse_wg_show_all(output: &str) -> WgStatus {
    let mut status = WgStatus::default();
    let mut current_peer: Option<WgPeerStatus> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(_rest) = line.strip_prefix("interface:") {
            if let Some(p) = current_peer.take() {
                status.peers.push(p);
            }
            status.interface_up = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("public key:") {
            status.public_key = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("listening port:") {
            status.listen_port = rest.trim().parse().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix("peer:") {
            if let Some(p) = current_peer.take() {
                status.peers.push(p);
            }
            let peer = WgPeerStatus {
                public_key: rest.trim().to_string(),
                ..Default::default()
            };
            current_peer = Some(peer);
            continue;
        }
        if let Some(peer) = current_peer.as_mut() {
            if let Some(rest) = line.strip_prefix("endpoint:") {
                peer.endpoint = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("latest handshake:") {
                peer.latest_handshake = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("transfer:") {
                // "1.23 KiB received, 4.56 KiB sent"
                let parts: Vec<&str> = rest.split(',').collect();
                if parts.len() == 2 {
                    peer.transfer_rx = Some(parts[0].replace("received", "").trim().to_string());
                    peer.transfer_tx = Some(parts[1].replace("sent", "").trim().to_string());
                }
            }
        }
        // allowed ips, and any other peer lines, are ignored for v1
    }
    if let Some(p) = current_peer.take() {
        status.peers.push(p);
    }
    status
}
