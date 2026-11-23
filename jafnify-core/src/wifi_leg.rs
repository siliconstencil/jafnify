//! The passive PC leg: how long the laptop waits for the phone's summary.
//!
//! The window holds the newest samples of the request the daemon already
//! sends to the phone: the reply time of a good reply, or a lost sample when
//! the request timed out. A reply also carries the phone app's own delay, so
//! the verdict built from this leg only says "likely". Pure arithmetic and
//! wording: no clock, no socket, no storage.

use std::collections::VecDeque;

use crate::bottleneck::LinkLeg;

/// How many samples the window keeps.
pub const WINDOW: usize = 30;
/// How many samples are needed before the leg is reported.
pub const MIN_SAMPLES: usize = 10;

/// The measured PC leg of the link to the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WifiLeg {
    /// The median reply time of the window, in milliseconds.
    pub median_ms: u64,
    /// How many percent of the window was lost.
    pub loss_pct: u64,
    /// How many requests the window holds.
    pub samples: usize,
}

impl WifiLeg {
    /// A leg from parts; used when the numbers travel over the control socket.
    pub fn new(median_ms: u64, loss_pct: u64, samples: usize) -> Self {
        Self {
            median_ms,
            loss_pct,
            samples,
        }
    }

    /// The same leg in the shape the bottleneck verdict uses.
    pub fn link_leg(&self) -> LinkLeg {
        LinkLeg {
            avg_ms: self.median_ms,
            loss_pct: self.loss_pct,
        }
    }

    /// The one-line reading under the verdict.
    pub fn line(&self) -> String {
        format!(
            "Wi-Fi to phone: {} ms, {}% lost (last {} checks)",
            self.median_ms, self.loss_pct, self.samples
        )
    }
}

/// The newest samples of the PC leg.
#[derive(Debug, Clone, Default)]
pub struct WifiWindow {
    samples: VecDeque<Option<u64>>,
}

impl WifiWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// One request: the reply time in milliseconds, or `None` when the reply
    /// was lost. The oldest sample falls out of the window.
    pub fn push(&mut self, sample: Option<u64>) {
        if self.samples.len() == WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Forget every sample: a new session starts from nothing.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// How many samples the window holds.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The leg of the window, or `None` while there is too little to say.
    pub fn leg(&self) -> Option<WifiLeg> {
        if self.samples.len() < MIN_SAMPLES {
            return None;
        }
        let mut times: Vec<u64> = self.samples.iter().filter_map(|sample| *sample).collect();
        if times.is_empty() {
            return None;
        }
        times.sort_unstable();
        let lost = self.samples.len() - times.len();
        Some(WifiLeg {
            median_ms: median_of(&times),
            loss_pct: (lost as u64 * 100) / self.samples.len() as u64,
            samples: self.samples.len(),
        })
    }
}

/// The median of a sorted list; an even count averages the two middle ones.
fn median_of(sorted: &[u64]) -> u64 {
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2
    }
}
