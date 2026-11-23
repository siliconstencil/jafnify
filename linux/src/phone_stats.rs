//! Linux client for the phone measurements.
//!
//! The transport is one UDP summary request bound to `jafn0`; the worker
//! keeps the last value for the window. Tests use a fake transport, so no
//! test opens a socket.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jafnify_core::phone_exit::{self, ExitReason};
use jafnify_core::phone_stats::{self, PhoneStats};
use jafnify_core::wifi_leg::{WifiLeg, WifiWindow};

/// How often a connected phone is polled while the window is closed.
pub const INTERVAL_IDLE: Duration = Duration::from_secs(5);
/// How often a connected phone is polled while the window is open.
pub const INTERVAL_WINDOW: Duration = Duration::from_secs(1);
/// This many failed requests without a value mark the phone unavailable.
pub const UNAVAILABLE_AFTER: u32 = 3;
const GATE_INTERVAL: Duration = Duration::from_millis(250);
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Why one summary request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Malformed,
    Io(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Timeout => f.write_str("no reply"),
            TransportError::Malformed => f.write_str("malformed summary"),
            TransportError::Io(e) => write!(f, "transport error: {e}"),
        }
    }
}

/// One reply: the summary and the exit reason it carried, when it carried one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneSummary {
    pub stats: PhoneStats,
    /// Why the phone app's previous process ended, sent once per session.
    pub exit: Option<ExitReason>,
}

/// One summary request.
pub trait PhoneStatsTransport: Send {
    /// One summary request. `range` is the laptop's link report, sent only
    /// to a phone that said it takes one (`caps=link`).
    fn request_summary(
        &mut self,
        range: Option<&jafnify_core::wifi_range::RangeReport>,
    ) -> Result<Option<PhoneSummary>, TransportError>;
}

/// A UDP request bound to `jafn0`, aimed at the gateway stats responder.
pub struct UdpTransport {
    socket: Option<socket2::Socket>,
    target: SocketAddr,
}

impl UdpTransport {
    pub fn new() -> Self {
        let ip = jafnify_core::GATEWAY_DNS_IP
            .parse::<Ipv4Addr>()
            .unwrap_or(Ipv4Addr::new(10, 233, 0, 1));
        Self {
            socket: None,
            target: SocketAddr::from((ip, phone_stats::PORT)),
        }
    }

    fn socket(&mut self) -> Result<&socket2::Socket, TransportError> {
        if self.socket.is_none() {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(|e| TransportError::Io(e.to_string()))?;
            socket
                .bind_device(Some(crate::app::IFACE.as_bytes()))
                .map_err(|e| TransportError::Io(e.to_string()))?;
            socket
                .set_read_timeout(Some(READ_TIMEOUT))
                .map_err(|e| TransportError::Io(e.to_string()))?;
            self.socket = Some(socket);
        }
        Ok(self.socket.as_ref().expect("the socket was just set"))
    }
}

impl Default for UdpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl PhoneStatsTransport for UdpTransport {
    fn request_summary(
        &mut self,
        range: Option<&jafnify_core::wifi_range::RangeReport>,
    ) -> Result<Option<PhoneSummary>, TransportError> {
        let result = self.request_once(range);
        if should_rebind(&result) {
            // The socket is bound to jafn0 by interface index. A session
            // restart recreates jafn0 with a new index, and the old socket
            // then fails for good (field 0.2.1: "stale" until a restart).
            // Drop it; the next request binds to the current jafn0.
            self.socket = None;
        }
        result
    }
}

/// True when the socket must be opened again before the next request: any
/// error except a malformed reply from the right address.
pub fn should_rebind(result: &Result<Option<PhoneSummary>, TransportError>) -> bool {
    matches!(
        result,
        Err(TransportError::Io(_)) | Err(TransportError::Timeout)
    )
}

impl UdpTransport {
    fn request_once(
        &mut self,
        range: Option<&jafnify_core::wifi_range::RangeReport>,
    ) -> Result<Option<PhoneSummary>, TransportError> {
        let target: socket2::SockAddr = self.target.into();
        let request = match range {
            Some(report) => phone_stats::request_summary_with_range(report).into_bytes(),
            None => phone_stats::request_summary().to_vec(),
        };
        let socket = self.socket()?;
        socket
            .send_to(&request, &target)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); phone_stats::SUMMARY_MAX];
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let from: SocketAddr = from.as_socket().ok_or(TransportError::Malformed)?;
                if from != self.target {
                    return Err(TransportError::Malformed);
                }
                // SAFETY: `recv_from` wrote exactly `len` bytes into `buf`.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len) };
                let stats =
                    PhoneStats::parse_response(bytes).map_err(|_| TransportError::Malformed)?;
                Ok(Some(PhoneSummary {
                    stats,
                    exit: phone_exit::from_summary(bytes),
                }))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Err(TransportError::Timeout)
            }
            Err(e) => Err(TransportError::Io(e.to_string())),
        }
    }
}

/// One request, or `None` without opening a socket when not connected.
pub fn run_once(
    transport: &mut dyn PhoneStatsTransport,
    connected: bool,
) -> Result<Option<PhoneSummary>, TransportError> {
    if !connected {
        return Ok(None);
    }
    transport.request_summary(None)
}

#[derive(Default)]
struct PhoneStatsCache {
    stats: Option<PhoneStats>,
    last_ok: Option<Instant>,
    timeouts: u32,
    unavailable: bool,
    /// The passive PC leg: the reply times of the last requests.
    wifi: WifiWindow,
    /// How many summaries arrived. The count tells a new summary from the
    /// same one, so a caller can work on every summary once.
    generations: u64,
    /// The exit reason of the newest summary, with its generation.
    exit: Option<ExitReason>,
    exit_generation: u64,
    /// The round trip of the request that produced `last_ok`.
    last_rtt: Duration,
}

/// The cache and the three gate bits the daemon updates every tick.
pub struct PhoneStatsShared {
    cache: Mutex<PhoneStatsCache>,
    enabled: AtomicBool,
    connected: AtomicBool,
    window_open: AtomicBool,
    /// Ask the worker for one summary at once (a fresh session's clock).
    force: AtomicBool,
    /// The laptop's link report for the phone.
    range: Mutex<Option<jafnify_core::wifi_range::RangeReport>>,
}

impl Default for PhoneStatsShared {
    fn default() -> Self {
        Self::new()
    }
}

impl PhoneStatsShared {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(PhoneStatsCache::default()),
            enabled: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            window_open: AtomicBool::new(false),
            force: AtomicBool::new(false),
            range: Mutex::new(None),
        }
    }

    /// The laptop's link report to pass on with the next request; None
    /// while the range is off or there is no Wi-Fi reading.
    pub fn set_range(&self, range: Option<jafnify_core::wifi_range::RangeReport>) {
        if let Ok(mut slot) = self.range.lock() {
            *slot = range;
        }
    }

    pub fn set(&self, enabled: bool, connected: bool, window_open: bool) {
        let was_enabled = self.enabled.swap(enabled, Ordering::Relaxed);
        let was_connected = self.connected.swap(connected, Ordering::Relaxed);
        self.window_open.store(window_open, Ordering::Relaxed);
        if connected && !was_connected {
            // A new Connected episode: fetch its first summary at once so the
            // laptop adopts the phone's clock without a poll interval of lag.
            self.force.store(true, Ordering::Relaxed);
        }
        if was_enabled != enabled || was_connected != connected {
            // A new session, or a switch that went off: the old samples
            // belong to the old session and must not be mixed into the new
            // one. This does not depend on the socket being rebound.
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.wifi.clear();
            cache.exit = None;
            cache.exit_generation = 0;
        }
    }

    /// Take the one-shot immediate-poll request.
    fn take_force(&self) -> bool {
        self.force.swap(false, Ordering::Relaxed)
    }

    /// The last value, its age in seconds and the unavailable flag.
    pub fn snapshot(&self, now: Instant) -> (Option<PhoneStats>, Option<u64>, bool) {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        (
            cache.stats.clone(),
            cache
                .last_ok
                .map(|last| now.saturating_duration_since(last).as_secs()),
            cache.unavailable,
        )
    }

    /// The measured PC leg of the window, if the window holds enough samples.
    pub fn wifi_leg(&self) -> Option<WifiLeg> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.wifi.leg()
    }

    /// The last summary and the generation that counts it, read under one
    /// lock: a summary that arrives between two reads cannot be half of the
    /// pair. A caller that works on every summary once needs exactly this.
    pub fn summary_with_generation(&self) -> (Option<PhoneStats>, u64) {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        (cache.stats.clone(), cache.generations)
    }

    /// The last summary, the instant its reply arrived and the generation
    /// that counts it. The laptop adopts the phone's connected time once per
    /// summary. The phone samples its clock at reply time, so anchoring at
    /// the request start would run the laptop ahead by the round trip.
    pub fn summary_for_clock(&self) -> (Option<PhoneStats>, Option<Instant>, u64) {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let at = cache.last_ok.map(|start| start + cache.last_rtt);
        (cache.stats.clone(), at, cache.generations)
    }

    /// How many samples the window holds.
    pub fn wifi_samples(&self) -> usize {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.wifi.len()
    }

    /// The exit reason of the newest summary and the generation that counts
    /// it, so the caller acts on every summary once.
    pub fn exit(&self) -> Option<(ExitReason, u64)> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.exit.map(|exit| (exit, cache.exit_generation))
    }

    /// Fill the cache directly; only the daemon's inline tests need this.
    #[cfg(test)]
    pub fn record_for_test(&self, stats: PhoneStats, now: Instant) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.stats = Some(stats);
        cache.last_ok = Some(now);
        cache.last_rtt = Duration::ZERO;
        cache.timeouts = 0;
        cache.unavailable = false;
        cache.generations = cache.generations.saturating_add(1);
        cache.exit = None;
        cache.exit_generation = 0;
    }

    /// Fill the cache with a summary and its exit reason; only tests need it.
    #[cfg(test)]
    pub fn record_exit_for_test(&self, stats: PhoneStats, exit: ExitReason, now: Instant) {
        self.record_for_test(stats, now);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.exit = Some(exit);
        cache.exit_generation = cache.generations;
    }
}

/// One worker decision. No request while the flag is off or the tunnel is
/// down; a success replaces the value, a failure keeps the last one.
pub fn step(
    shared: &PhoneStatsShared,
    transport: &mut dyn PhoneStatsTransport,
    now: Instant,
) -> Duration {
    if !shared.enabled.load(Ordering::Relaxed) || !shared.connected.load(Ordering::Relaxed) {
        return GATE_INTERVAL;
    }
    // The report goes only to a phone whose last summary said it takes one.
    let range = {
        let takes = shared
            .cache
            .lock()
            .map(|cache| cache.stats.as_ref().is_some_and(|stats| stats.caps_link))
            .unwrap_or(false);
        if takes {
            shared.range.lock().ok().and_then(|slot| *slot)
        } else {
            None
        }
    };
    let started = Instant::now();
    let result = transport.request_summary(range.as_ref());
    let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    {
        let mut cache = shared.cache.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(Some(summary)) => {
                cache.stats = Some(summary.stats);
                // The clock anchor is the reply time: the tick instant plus
                // the request's measured round trip. Keeping `last_ok` itself
                // at the tick keeps the snapshot's age deterministic.
                cache.last_ok = Some(now);
                cache.last_rtt = Duration::from_millis(elapsed_ms);
                cache.timeouts = 0;
                cache.unavailable = false;
                cache.generations = cache.generations.saturating_add(1);
                // The exit reason is one summary's news; a newer summary
                // without it says nothing about the old one.
                if let Some(exit) = summary.exit {
                    cache.exit = Some(exit);
                    cache.exit_generation = cache.generations;
                }
                // A good reply is one sample of the PC leg.
                cache.wifi.push(Some(elapsed_ms));
            }
            Ok(None) => note_failure(&mut cache),
            Err(TransportError::Timeout) => {
                note_failure(&mut cache);
                // A timeout is a lost sample. The other errors say nothing
                // about the link, so they are no sample at all.
                cache.wifi.push(None);
            }
            Err(_) => note_failure(&mut cache),
        }
    }
    if shared.window_open.load(Ordering::Relaxed) {
        INTERVAL_WINDOW
    } else {
        INTERVAL_IDLE
    }
}

/// One failed request: the value gets older and may become unavailable.
fn note_failure(cache: &mut PhoneStatsCache) {
    cache.timeouts = cache.timeouts.saturating_add(1);
    if cache.stats.is_none() && cache.timeouts >= UNAVAILABLE_AFTER {
        cache.unavailable = true;
    }
}

/// Start the worker thread. It is the only caller of the transport.
/// Wake interval while the switch is off or the tunnel is down.
const IDLE_INTERVAL: Duration = Duration::from_secs(2);

/// The one line the worker logs when its body ends. A panic and a return both
/// mean the laptop's phone measurements stop, so neither may stay silent.
fn worker_exit_line(outcome: &std::thread::Result<()>) -> &'static str {
    if outcome.is_err() {
        "phone stats worker panicked; measurements stop until restart"
    } else {
        "phone stats worker returned; measurements stop until restart"
    }
}

/// The worker body, kept separate so the spawn site can guard it.
fn worker_loop(shared: &PhoneStatsShared, transport: &mut dyn PhoneStatsTransport) {
    let mut next = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next || shared.take_force() {
            next = now + step(shared, transport, now);
        }
        // While the switch is off or the tunnel is down, wake rarely: an
        // idle worker must not cost the laptop four wakeups a second.
        let active =
            shared.enabled.load(Ordering::Relaxed) && shared.connected.load(Ordering::Relaxed);
        std::thread::sleep(if active { GATE_INTERVAL } else { IDLE_INTERVAL });
    }
}

pub fn spawn_worker(
    shared: Arc<PhoneStatsShared>,
    mut transport: Box<dyn PhoneStatsTransport>,
    mut log: impl FnMut(&str, &str) + Send + 'static,
) {
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            worker_loop(&shared, transport.as_mut())
        }));
        let level = if outcome.is_err() { "error" } else { "warn" };
        log(level, worker_exit_line(&outcome));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_that_panics_and_one_that_returns_both_say_so() {
        let returned: std::thread::Result<()> = Ok(());
        assert!(worker_exit_line(&returned).contains("returned"));
        let panicked = std::panic::catch_unwind(|| panic!("injected worker panic"));
        assert!(panicked.is_err());
        assert!(worker_exit_line(&panicked).contains("panicked"));
    }
}
