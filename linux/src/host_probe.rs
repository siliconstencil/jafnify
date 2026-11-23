//! The read-only host probe.
//!
//! The optional tools (`nmcli`, `iw`) run on their own thread with a
//! timeout, so neither the daemon loop nor the window frame ever waits for
//! them. A result stays fresh for a minute; the daemon asks for a refresh
//! only while the Details panel is open.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::host_hints::{parse_access_points, parse_iw_power_save, parse_wifi_devices, HostHint};

/// How long one tool may take before it is given up on.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a result stays fresh.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// One pending job at a time is enough.
const QUEUE: usize = 1;

/// Runs one command and gives its output, or nothing.
pub trait CommandRunner: Send + Sync {
    /// The output of the command, or `None` when it failed or was too slow.
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Option<String>;
}

/// The real tools, read-only and without a shell.
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Option<String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // The output is read on its own thread so a long list cannot fill the
        // pipe and stall the child.
        let mut stdout = child.stdout.take()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stdout.read_to_end(&mut buffer);
            let _ = sender.send(String::from_utf8_lossy(&buffer).into_owned());
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break None,
            }
        };
        let output = receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        match status {
            Some(status) if status.success() => Some(output),
            _ => None,
        }
    }
}

/// What the probe was asked to look at.
struct Job {
    uplink_iface: Option<String>,
}

/// The last result and when it was taken.
struct Cached {
    at: Instant,
    hints: Vec<HostHint>,
}

/// The host probe: asks the tools on a worker thread, keeps the result for a
/// minute and hands out only what was explicitly detected.
pub struct HostProbe {
    sender: SyncSender<Job>,
    cached: Arc<Mutex<Option<Cached>>>,
}

impl HostProbe {
    /// Start the worker thread with the given runner.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        let (sender, receiver) = sync_channel(QUEUE);
        let cached: Arc<Mutex<Option<Cached>>> = Arc::new(Mutex::new(None));
        let store = cached.clone();
        let _ = std::thread::Builder::new()
            .name("jafnify-hosts".to_string())
            .spawn(move || worker(runner, receiver, store));
        HostProbe { sender, cached }
    }

    /// Ask for a refresh; the caller never waits. A fresh result is kept.
    pub fn request(&self, uplink_iface: Option<String>) {
        let now = Instant::now();
        if let Ok(guard) = self.cached.lock() {
            if guard.as_ref().is_some_and(|cached| fresh(cached, now)) {
                return;
            }
        }
        let _ = self.sender.try_send(Job { uplink_iface });
    }

    /// The hints of the last fresh result; empty when there is none.
    pub fn hints(&self, now: Instant) -> Vec<HostHint> {
        let Ok(guard) = self.cached.lock() else {
            return Vec::new();
        };
        match guard.as_ref() {
            Some(cached) if fresh(cached, now) => cached.hints.clone(),
            _ => Vec::new(),
        }
    }

    /// Forget the result; the next request runs the tools again.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.cached.lock() {
            *guard = None;
        }
    }
}

/// Is the result still good at this moment?
fn fresh(cached: &Cached, now: Instant) -> bool {
    match now.checked_duration_since(cached.at) {
        Some(age) => age <= CACHE_TTL,
        // The result is newer than the moment asked about: it is fresh.
        None => true,
    }
}

/// The worker: one job at a time, never on the caller's thread.
fn worker(
    runner: Arc<dyn CommandRunner>,
    receiver: Receiver<Job>,
    cached: Arc<Mutex<Option<Cached>>>,
) {
    while let Ok(job) = receiver.recv() {
        let hints = collect(&runner, &job);
        if let Ok(mut guard) = cached.lock() {
            *guard = Some(Cached {
                at: Instant::now(),
                hints,
            });
        }
    }
}

/// What the tools say; only an explicit positive becomes a hint.
fn collect(runner: &Arc<dyn CommandRunner>, job: &Job) -> Vec<HostHint> {
    let mut hints = Vec::new();
    for iface in wifi_devices(runner, job) {
        if let Some(output) = runner.run("iw", &["dev", &iface, "get", "power_save"], PROBE_TIMEOUT)
        {
            if parse_iw_power_save(&output) {
                hints.push(HostHint::WifiPowerSaving { iface });
                break;
            }
        }
    }
    if let Some(output) = runner.run(
        "nmcli",
        &[
            "-t",
            "-f",
            "IN-USE,BSSID,SSID",
            "device",
            "wifi",
            "list",
            "--rescan",
            "no",
        ],
        PROBE_TIMEOUT,
    ) {
        if parse_access_points(&output) {
            hints.push(HostHint::SeveralAccessPoints);
        }
    }
    hints
}

/// The Wi-Fi devices of the active connections; without nmcli, the uplink
/// the link watch knows (only while a tunnel is up).
fn wifi_devices(runner: &Arc<dyn CommandRunner>, job: &Job) -> Vec<String> {
    if let Some(output) = runner.run(
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE", "connection", "show", "--active"],
        PROBE_TIMEOUT,
    ) {
        return parse_wifi_devices(&output);
    }
    job.uplink_iface.iter().cloned().collect()
}
