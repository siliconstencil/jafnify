//! The laptop's view of its Wi-Fi link to the hotspot: nl80211 station data
//! (no root), with `/proc/net/wireless` as the fallback when netlink gives
//! nothing. Only numbers leave this module; the address of the hotspot that
//! nl80211 also returns is dropped here and never stored.

use std::path::PathBuf;

use jafnify_core::wifi_range::{parse_proc_wireless, StationSample};

pub use jafnify_core::platform::station::StationSource;

/// The production source: one nl80211 socket, kept open and reopened after
/// an error, and the proc file as the fallback.
pub struct NlStation {
    socket: Option<neli_wifi::Socket>,
    proc_path: PathBuf,
}

impl NlStation {
    pub fn new(proc_path: PathBuf) -> Self {
        Self {
            socket: None,
            proc_path,
        }
    }

    fn read_netlink(&mut self, iface: &str) -> Option<StationSample> {
        let name = std::ffi::CString::new(iface).ok()?;
        // SAFETY: `name` is a valid NUL-terminated string that outlives the call.
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return None;
        }
        if self.socket.is_none() {
            self.socket = neli_wifi::Socket::connect().ok();
        }
        let socket = self.socket.as_mut()?;
        let stations = match socket.get_station_info(index as i32) {
            Ok(stations) => stations,
            Err(_) => {
                // A broken socket is reopened on the next reading.
                self.socket = None;
                return None;
            }
        };
        // In client mode the only station is the hotspot.
        let station = stations.into_iter().next()?;
        Some(StationSample {
            signal_dbm: station.signal.map(i32::from),
            signal_avg_dbm: station.average_signal.map(i32::from),
            tx_rate_100k: station.tx_bitrate,
            tx_retries: station.tx_retries,
            tx_failed: station.tx_failed,
            beacon_loss: station.beacon_loss,
        })
        .filter(|sample| sample.level().is_some())
    }

    fn read_proc(&self, iface: &str) -> Option<StationSample> {
        let text = std::fs::read_to_string(&self.proc_path).ok()?;
        parse_proc_wireless(&text, iface).map(StationSample::level_only)
    }
}

impl StationSource for NlStation {
    fn read(&mut self, iface: &str) -> Option<StationSample> {
        self.read_netlink(iface).or_else(|| self.read_proc(iface))
    }
}

/// A fixed reading per call, for tests.
pub struct FakeStation {
    pub iface: String,
    pub next: std::sync::Arc<std::sync::Mutex<Option<StationSample>>>,
}

impl StationSource for FakeStation {
    fn read(&mut self, iface: &str) -> Option<StationSample> {
        if iface != self.iface {
            return None;
        }
        *self.next.lock().ok()?
    }
}

/// The local link log never grows past this size; the older half goes.
pub const LINK_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Append one line to the bounded link log (0600). When the file is past
/// `max_bytes`, only its newer half is kept.
pub fn append_bounded(path: &std::path::Path, line: &str, max_bytes: u64) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max_bytes) {
        let text = std::fs::read_to_string(path)?;
        let cut = text.len() / 2;
        let start = text[cut..].find('\n').map_or(text.len(), |i| cut + i + 1);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &text[start..])?;
        std::fs::rename(&tmp, path)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{line}")
}
