//! Link liveness shared by every frontend. The frontends own the sockets and
//! the tunnel; this module owns the rule for "the peer's data stopped".

use std::time::{Duration, Instant};

use crate::RX_STALL_TIMEOUT;

/// Watches a tunnel's received-bytes counter. The counter is a plain
/// `Option<u64>`: `None` = unreadable (interface gone), which is never
/// progress.
#[derive(Debug, Clone)]
pub struct RxWatch {
    last: Option<u64>,
    since: Instant,
}

impl RxWatch {
    pub fn new(now: Instant, rx: Option<u64>) -> Self {
        Self {
            last: rx,
            since: now,
        }
    }

    /// True when the counter advanced since the previous observation. An
    /// unreadable counter (`None`) never advances.
    pub fn advanced(&mut self, now: Instant, rx: Option<u64>) -> bool {
        if rx.is_some() && rx != self.last {
            self.last = rx;
            self.since = now;
            true
        } else {
            false
        }
    }

    /// How long the counter has been unchanged as of `now`. An unreadable
    /// counter is not progress, so it keeps the current flat time.
    pub fn unchanged_for(&mut self, now: Instant, rx: Option<u64>) -> Duration {
        self.advanced(now, rx);
        now.saturating_duration_since(self.since)
    }

    /// How long ago the counter last advanced, without reading it again.
    pub fn silent_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.since)
    }

    /// True once the counter has not advanced for `RX_STALL_TIMEOUT`.
    pub fn stalled(&mut self, now: Instant, rx: Option<u64>) -> bool {
        self.unchanged_for(now, rx) >= RX_STALL_TIMEOUT
    }
}
