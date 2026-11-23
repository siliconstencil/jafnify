//! Pure aggregation for the one-shot "Test the link now" measurement.
//!
//! The platform sends the echoes and feeds one sample per probe; this module
//! only counts and formats. It has no sockets, no clock and no storage.

use std::collections::BTreeSet;
use std::time::Duration;

/// Fixed 0.2 plan: 10 probes, 100 ms apart, 1 s timeout each.
pub const PROBES: u32 = 10;
pub const INTERVAL: Duration = Duration::from_millis(100);
pub const TIMEOUT: Duration = Duration::from_secs(1);

/// The plan a platform runs. Production uses [`LinkTestConfig::DEFAULT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkTestConfig {
    pub probes: u32,
    pub interval: Duration,
    pub timeout: Duration,
}

impl LinkTestConfig {
    pub const DEFAULT: Self = Self {
        probes: PROBES,
        interval: INTERVAL,
        timeout: TIMEOUT,
    };
}

/// What the measurement says, independent of how many probes replied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOutcome {
    /// At least one reply.
    Measured,
    /// Every probe timed out.
    NoReply,
    /// The host denied the ICMP socket.
    NotPermitted,
    /// The tunnel was not connected, so nothing was sent.
    ConnectFirst,
}

/// One probe result. `rtt_ms` is None when that probe timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSample {
    pub seq: u32,
    pub rtt_ms: Option<u64>,
}

/// The aggregate shown once; never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkTestResult {
    pub sent: u32,
    pub received: u32,
    pub min_ms: Option<u64>,
    pub avg_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub loss_pct: u32,
    pub outcome: LinkOutcome,
}

impl LinkTestResult {
    /// Aggregate one sample per probe. A repeated sequence number is ignored,
    /// so a duplicate or late reply cannot be counted twice.
    pub fn from_samples(samples: &[LinkSample]) -> Self {
        let mut seen = BTreeSet::new();
        let rtts: Vec<u64> = samples
            .iter()
            .filter(|sample| seen.insert(sample.seq))
            .filter_map(|sample| sample.rtt_ms)
            .collect();
        let sent = seen.len() as u32;
        let received = rtts.len() as u32;
        let (min_ms, avg_ms, max_ms) = if rtts.is_empty() {
            (None, None, None)
        } else {
            let sum: u128 = rtts.iter().map(|&rtt| u128::from(rtt)).sum();
            let avg = (sum / rtts.len() as u128) as u64;
            (
                rtts.iter().copied().min(),
                Some(avg),
                rtts.iter().copied().max(),
            )
        };
        let loss_pct = if sent == 0 {
            100
        } else {
            (((sent - received) as u64 * 100) / u64::from(sent)) as u32
        };
        let outcome = if received == 0 {
            LinkOutcome::NoReply
        } else {
            LinkOutcome::Measured
        };
        Self {
            sent,
            received,
            min_ms,
            avg_ms,
            max_ms,
            loss_pct,
            outcome,
        }
    }

    /// The host denied the unprivileged ICMP socket.
    pub fn not_permitted() -> Self {
        Self {
            sent: 0,
            received: 0,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            loss_pct: 100,
            outcome: LinkOutcome::NotPermitted,
        }
    }

    /// The daemon or CLI was not Connected, so no probe was sent.
    pub fn connect_first() -> Self {
        Self {
            sent: 0,
            received: 0,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            loss_pct: 100,
            outcome: LinkOutcome::ConnectFirst,
        }
    }

    /// The one line the window and CLI show. Wording lives in core.
    pub fn line(&self) -> String {
        match self.outcome {
            LinkOutcome::NotPermitted => "Ping is not permitted on this system.".to_string(),
            LinkOutcome::ConnectFirst => "Connect first.".to_string(),
            LinkOutcome::NoReply => "No reply.".to_string(),
            LinkOutcome::Measured => format!(
                "Round trip: min {} ms, average {} ms, max {} ms; {}% loss.",
                self.min_ms.unwrap_or(0),
                self.avg_ms.unwrap_or(0),
                self.max_ms.unwrap_or(0),
                self.loss_pct
            ),
        }
    }
}

/// The fixed "not connected" line, for a caller that has not measured.
pub fn connect_first_line() -> String {
    LinkTestResult::connect_first().line()
}
