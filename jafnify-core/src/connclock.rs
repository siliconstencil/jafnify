//! The connected clock both sides count from.
//!
//! The phone (the sharer) is the clock authority: its native clock starts at
//! the first authenticated exchange of the current tunnel session. The laptop
//! adopts the phone's elapsed value from every summary reply and advances it
//! with its own clock, so the two timers hold the same value without trusting
//! the two devices' wall clocks. A phone session reset rewinds the laptop.
//!
//! A display-only neutral (no authenticated peer activity for a moment) does
//! not end the session: the synced value is kept across the blip and resumes,
//! so a short gap does not restart the phone's clock on this side.

/// A synced value older than this is no longer trusted: without a fresh reply
/// the tunnel is likely down, and the local clock is the honest fallback.
pub const SYNC_STALE_MS: i64 = 20_000;

/// A phone elapsed value this far behind the current one is a new session,
/// not sampler jitter: the phone's app was updated or it restarted.
pub const REWIND_MS: u64 = 2_000;

/// The connected clock, in epoch milliseconds.
#[derive(Debug, Clone, Default)]
pub struct ConnectedClock {
    /// A Connected episode is open. It survives a display-only neutral.
    session: bool,
    /// The local first-data instant of the current visible episode.
    local_start_ms: Option<i64>,
    /// The phone's elapsed and the instant it was received.
    synced: Option<(u64, i64)>,
}

impl ConnectedClock {
    /// This side became Connected at `now_ms`. A fresh episode starts from
    /// zero locally until the phone's value arrives.
    pub fn connected(&mut self, now_ms: i64) {
        self.session = true;
        self.local_start_ms = Some(now_ms);
        self.synced = None;
    }

    /// This side left Connected for real: there is no connected time.
    pub fn disconnected(&mut self) {
        self.session = false;
        self.local_start_ms = None;
        self.synced = None;
    }

    /// The display turned neutral (a short gap) but the session continues.
    /// The value is hidden until [`resume`](Self::resume), and the phone's
    /// last value is kept.
    pub fn pause(&mut self) {
        self.local_start_ms = None;
    }

    /// The display is Connected again. A kept phone value carries on across
    /// the blip; without one the local clock restarts here.
    pub fn resume(&mut self, now_ms: i64) {
        if self.session {
            self.local_start_ms = Some(now_ms);
        } else {
            self.connected(now_ms);
        }
    }

    /// A new session started at `now_ms` (peer restart, a reset or a gap):
    /// drop the old value and count from the new exchange.
    pub fn restart(&mut self, now_ms: i64) {
        self.connected(now_ms);
    }

    /// Whether `elapsed_ms` belongs to a new phone session: it is behind the
    /// value this side already advanced. Only a value this side has adopted
    /// can rewind, so the first reply is never a new session.
    pub fn peer_rewound(&self, elapsed_ms: u64, at_ms: i64) -> bool {
        if !self.session {
            return false;
        }
        let Some((base, at)) = self.synced else {
            return false;
        };
        let current = base.saturating_add(at_ms.saturating_sub(at).max(0) as u64);
        elapsed_ms.saturating_add(REWIND_MS) < current
    }

    /// Adopt the phone's elapsed time as received at `at_ms`. Returns whether
    /// the value was taken; a side with no session ignores it.
    pub fn observe_peer_elapsed(&mut self, elapsed_ms: u64, at_ms: i64) -> bool {
        if !self.session {
            return false;
        }
        self.synced = Some((elapsed_ms, at_ms));
        true
    }

    /// The connected time at `now_ms`: the phone's value while it is fresh,
    /// else the local time since this side's own first data. `None` while the
    /// display is neutral.
    pub fn elapsed_ms(&self, now_ms: i64) -> Option<u64> {
        if !self.session {
            return None;
        }
        let start = self.local_start_ms?;
        if let Some((base, at)) = self.synced {
            let age = now_ms.saturating_sub(at);
            if age <= SYNC_STALE_MS {
                return Some(base.saturating_add(age as u64));
            }
        }
        Some(now_ms.saturating_sub(start) as u64)
    }

    /// [`elapsed_ms`](Self::elapsed_ms) in whole seconds.
    pub fn elapsed_secs(&self, now_ms: i64) -> Option<u64> {
        self.elapsed_ms(now_ms).map(|ms| ms / 1000)
    }

    /// The local clock alone, ignoring the phone. Used by diagnostics and by
    /// the fallback path.
    pub fn local_elapsed_ms(&self, now_ms: i64) -> Option<u64> {
        self.local_start_ms
            .map(|start| now_ms.saturating_sub(start) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_starts_the_local_clock() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        assert_eq!(clock.elapsed_ms(4_000), Some(3_000));
        assert_eq!(clock.elapsed_secs(4_000), Some(3));
    }

    #[test]
    fn a_peer_elapsed_wins_over_the_local_clock() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        assert!(clock.observe_peer_elapsed(20_000, 2_000));
        // At 2_000 the phone says 20_000 ms; 3 s later it is 23_000 ms.
        assert_eq!(clock.elapsed_ms(5_000), Some(23_000));
    }

    #[test]
    fn a_peer_reset_rewinds_the_clock() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(14_000, 2_000);
        clock.observe_peer_elapsed(0, 3_000);
        assert_eq!(clock.elapsed_ms(6_000), Some(3_000));
    }

    #[test]
    fn a_stale_sync_falls_back_to_the_local_clock() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(20_000, 2_000);
        assert_eq!(clock.elapsed_ms(2_000 + SYNC_STALE_MS + 1), Some(21_001));
    }

    #[test]
    fn disconnected_has_no_elapsed() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(9_000, 2_000);
        clock.disconnected();
        assert_eq!(clock.elapsed_ms(5_000), None);
        assert!(!clock.observe_peer_elapsed(9_000, 5_000));
    }

    #[test]
    fn restart_clears_the_old_sync() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(9_000, 2_000);
        clock.restart(5_000);
        assert_eq!(clock.elapsed_ms(7_000), Some(2_000));
    }

    #[test]
    fn a_rewound_peer_value_means_a_new_session() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(30_000, 2_000);
        assert!(clock.peer_rewound(1_000, 40_000));
        // A normal poll advances the value: not a new session.
        assert!(!clock.peer_rewound(35_000, 7_000));
    }

    #[test]
    fn the_first_reply_is_never_a_rewind() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        assert!(!clock.peer_rewound(0, 2_000));
    }

    #[test]
    fn a_paused_clock_still_detects_a_rewind() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(30_000, 2_000);
        clock.pause();
        assert!(clock.peer_rewound(1_000, 40_000));
    }

    #[test]
    fn the_local_elapsed_ignores_the_peer() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(9_000, 2_000);
        assert_eq!(clock.local_elapsed_ms(4_000), Some(3_000));
    }

    #[test]
    fn a_pause_hides_the_value_and_keeps_the_phone_value() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.observe_peer_elapsed(10_000, 1_000);
        clock.pause();
        assert_eq!(clock.elapsed_ms(12_000), None);
        // A reply still arrives while the display is neutral.
        assert!(clock.observe_peer_elapsed(24_000, 12_000));
        clock.resume(16_000);
        assert_eq!(clock.elapsed_ms(16_000), Some(28_000));
    }

    #[test]
    fn a_resume_without_a_phone_value_restarts_the_local_clock() {
        let mut clock = ConnectedClock::default();
        clock.connected(1_000);
        clock.pause();
        clock.resume(9_000);
        assert_eq!(clock.elapsed_ms(9_000), Some(0));
    }
}
