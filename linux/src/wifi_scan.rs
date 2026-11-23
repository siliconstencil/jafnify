//! Ask NetworkManager to scan for Wi-Fi while the joiner has no network.
//!
//! NetworkManager scans on its own while a Wi-Fi device is disconnected, but
//! it backs off to one scan every 120 s. When the laptop walks out of hotspot
//! range and back, the hotspot is then often missed for many minutes even
//! though it is in range again. A scan request every [`SCAN_INTERVAL`] while
//! the joiner waits for the network lets NetworkManager's own autoconnect
//! rejoin the hotspot within seconds.
//!
//! Only a scan is requested, never an activation: autoconnect keeps every
//! rule of its own (a Wi-Fi network the user disconnected by hand stays
//! disconnected). A missing `nmcli` or a refused request is not an error;
//! the joiner just waits as before.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Time between scan requests while the network is gone.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// The Wi-Fi devices that are disconnected, from
/// `nmcli -t -f DEVICE,TYPE,STATE device` output. A device that is
/// unavailable (radio off), connecting or connected is left alone.
pub fn disconnected_wifi_devices(nmcli_terse: &str) -> Vec<String> {
    nmcli_terse
        .lines()
        .filter_map(|line| {
            // Terse output escapes ':' inside a field as "\:"; device names
            // never contain one, so a plain split from the right is enough.
            let mut fields = line.rsplitn(3, ':');
            let state = fields.next()?.trim();
            let kind = fields.next()?.trim();
            let device = fields.next()?.trim();
            (kind == "wifi" && state == "disconnected" && !device.is_empty())
                .then(|| device.to_string())
        })
        .collect()
}

/// Paces scan requests and runs them off the caller's thread.
#[derive(Debug)]
pub struct WifiScanNudge {
    nmcli: PathBuf,
    last: Option<Instant>,
    busy: Arc<AtomicBool>,
}

impl WifiScanNudge {
    pub fn new(nmcli: PathBuf) -> Self {
        Self {
            nmcli,
            last: None,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Forget the pacing so the next wait starts with an immediate request.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Whether a request is due at `now`.
    pub fn due(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|at| now.saturating_duration_since(at) >= SCAN_INTERVAL)
    }

    /// Request a scan on every disconnected Wi-Fi device when due. Returns
    /// true when a request was started. Never blocks the caller: the
    /// commands run on a short-lived thread, one at a time.
    pub fn nudge_if_due(&mut self, now: Instant) -> bool {
        if !self.due(now) {
            return false;
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.last = Some(now);
        let nmcli = self.nmcli.clone();
        let busy = Arc::clone(&self.busy);
        let spawned = std::thread::Builder::new()
            .name("wifi-scan".into())
            .spawn(move || {
                request_scans(&nmcli);
                busy.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Wait until the request in flight (if any) is done. For tests.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while self.busy.load(Ordering::Acquire) {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

/// One round: list the devices, then request a scan on each disconnected
/// Wi-Fi device. Errors are ignored on purpose (see the module docs).
pub fn request_scans(nmcli: &Path) {
    let listed = crate::exec::output({
        let mut cmd = Command::new(nmcli);
        cmd.args(["-t", "-f", "DEVICE,TYPE,STATE", "device"])
            .stdin(Stdio::null());
        cmd
    });
    let Ok(out) = listed else {
        return;
    };
    if !out.status.success() {
        return;
    }
    for device in disconnected_wifi_devices(&String::from_utf8_lossy(&out.stdout)) {
        let _ = crate::exec::output({
            let mut cmd = Command::new(nmcli);
            cmd.args(["device", "wifi", "rescan", "ifname", &device])
                .stdin(Stdio::null());
            cmd
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_disconnected_wifi_devices_are_listed() {
        let out = "wlo1:wifi:disconnected\n\
                   wlan1:wifi:connected\n\
                   wlx0:wifi:unavailable\n\
                   p2p-dev-wlo1:wifi-p2p:disconnected\n\
                   enp56s0:ethernet:disconnected\n\
                   jafn0:wireguard:connected (externally)\n\
                   \n\
                   garbage\n";
        assert_eq!(disconnected_wifi_devices(out), vec!["wlo1".to_string()]);
    }

    #[test]
    fn requests_are_paced() {
        let mut nudge = WifiScanNudge::new(PathBuf::from("/nonexistent/nmcli"));
        let t0 = Instant::now();
        assert!(nudge.nudge_if_due(t0));
        assert!(nudge.wait_idle(Duration::from_secs(5)));
        assert!(!nudge.nudge_if_due(t0 + Duration::from_secs(9)));
        assert!(nudge.nudge_if_due(t0 + SCAN_INTERVAL));
        assert!(nudge.wait_idle(Duration::from_secs(5)));
        nudge.reset();
        assert!(nudge.due(t0 + SCAN_INTERVAL));
    }
}
