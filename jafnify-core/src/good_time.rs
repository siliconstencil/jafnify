//! When the link is good enough for a big download.
//!
//! Pure: the caller hands over the phone's numbers and the Wi-Fi leg of the
//! laptop, together with the time, and gets back whether this is a good time
//! and whether a notice is due.

use std::time::{Duration, Instant};

use crate::phone_stats::NetworkKind;
use crate::wifi_leg::WifiLeg;

/// The good conditions have to hold this long before a notice goes out.
pub const HOLD: Duration = Duration::from_secs(2 * 60);

/// Before a notice, the conditions have to have been bad for this long.
pub const MIN_NOT_GOOD: Duration = Duration::from_secs(10 * 60);

/// At most one notice in this time.
pub const COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// The phone's DNS answer time has to be at most this.
pub const MAX_RTT_MS: u32 = 60;

/// The phone's signal has to be at least this, of four.
pub const MIN_SIGNAL: u8 = 3;

/// What the notice line starts with.
pub const NOTICE_PREFIX: &str = "Good time for big downloads";

/// True for every 5G kind.
fn is_five_g(net: NetworkKind) -> bool {
    matches!(
        net,
        NetworkKind::Mobile5g
            | NetworkKind::Mobile5gNsa
            | NetworkKind::Mobile5gMmwave
            | NetworkKind::Mobile5gAdv
    )
}

/// Whether this is a good time for a big download.
pub fn is_good(
    net: NetworkKind,
    signal: Option<u8>,
    rtt_ms: Option<u32>,
    wifi: Option<&WifiLeg>,
) -> bool {
    if !is_five_g(net) {
        return false;
    }
    if !signal.map(|value| value >= MIN_SIGNAL).unwrap_or(false) {
        return false;
    }
    if !rtt_ms.map(|value| value <= MAX_RTT_MS).unwrap_or(false) {
        return false;
    }
    match wifi {
        None => true,
        Some(leg) => {
            leg.median_ms <= crate::bottleneck::PC_AVG_MS
                && leg.loss_pct < crate::bottleneck::PC_LOSS_PCT
        }
    }
}

/// The notice: the network and the signal that made it good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoodTime {
    /// The phone's network, in words.
    pub net: &'static str,
    /// The phone's signal, of four.
    pub signal: u8,
}

impl GoodTime {
    /// The line the desktop shows.
    pub fn line(&self) -> String {
        format!("{NOTICE_PREFIX}: {}, signal {}/4", self.net, self.signal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Unknown,
    NotGood,
    Good,
}

/// The good-time episodes: when the conditions turn good and stay good, and
/// when a notice is due.
///
/// The bad time is counted up, not per stretch: ten bad minutes with a dip in
/// between still are ten bad minutes. After a notice the count starts again,
/// so the next notice needs ten more bad minutes.
#[derive(Debug, Default)]
pub struct GoodTimeTracker {
    state: State,
    good_since: Option<Instant>,
    bad_since: Option<Instant>,
    not_good_len: Duration,
    last_notice: Option<Instant>,
    net: &'static str,
    signal: u8,
}

impl GoodTimeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the conditions of this moment. `good` says whether they are good
    /// now; `net` and `signal` are what the notice will say.
    pub fn update(
        &mut self,
        good: bool,
        net: &'static str,
        signal: u8,
        now: Instant,
    ) -> Option<GoodTime> {
        self.net = net;
        self.signal = signal;
        match (self.state, good) {
            (State::Unknown, true) => {
                self.state = State::Good;
                self.good_since = Some(now);
                None
            }
            (State::Unknown, false) => {
                self.state = State::NotGood;
                self.bad_since = Some(now);
                None
            }
            (State::NotGood, false) => None,
            (State::NotGood, true) => {
                self.not_good_len += now.saturating_duration_since(self.bad_since.unwrap_or(now));
                self.bad_since = None;
                self.state = State::Good;
                self.good_since = Some(now);
                None
            }
            (State::Good, false) => {
                self.state = State::NotGood;
                self.bad_since = Some(now);
                self.good_since = None;
                None
            }
            (State::Good, true) => {
                let held = now.saturating_duration_since(self.good_since.unwrap_or(now));
                if held < HOLD || self.not_good_len < MIN_NOT_GOOD {
                    return None;
                }
                if let Some(last) = self.last_notice {
                    if now.saturating_duration_since(last) < COOLDOWN {
                        return None;
                    }
                }
                self.last_notice = Some(now);
                self.not_good_len = Duration::ZERO;
                Some(GoodTime {
                    net: self.net,
                    signal: self.signal,
                })
            }
        }
    }
}
