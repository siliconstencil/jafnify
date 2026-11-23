//! A small ring of signal levels, so the phone can say how the signal was
//! over the last day without asking for anything new.

use std::collections::VecDeque;

/// How many samples are kept: 288 is 24 hours at one sample every 5 minutes.
pub const MAX_SAMPLES: usize = 288;

/// The gap between two samples.
pub const INTERVAL_MS: i64 = 5 * 60_000;

/// One signal level: 0 is the weakest, 4 the strongest, 255 is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalSample {
    pub at_ms: i64,
    pub level: u8,
}

impl SignalSample {
    /// The stored line: `JSG1 1 at=... level=...`.
    pub fn line(&self) -> String {
        format!("JSG1 1 at={} level={}", self.at_ms, self.level)
    }

    /// Read one stored line; a bad or out-of-range line is none.
    pub fn parse_line(line: &str) -> Option<SignalSample> {
        let rest = line.strip_prefix("JSG1 1 ")?;
        let mut at = None;
        let mut level = None;
        for part in rest.split(' ') {
            let (key, value) = part.split_once('=')?;
            match key {
                "at" => at = value.parse::<i64>().ok(),
                "level" => level = value.parse::<u8>().ok(),
                _ => return None,
            }
        }
        let level = level?;
        if level > 4 {
            return None;
        }
        Some(SignalSample { at_ms: at?, level })
    }
}

/// The ring of samples.
#[derive(Debug, Clone, Default)]
pub struct SignalHistory {
    samples: VecDeque<SignalSample>,
}

impl SignalHistory {
    pub fn new() -> Self {
        SignalHistory {
            samples: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Add one sample; the oldest one is dropped when the ring is full.
    pub fn add(&mut self, sample: SignalSample) {
        self.samples.push_back(sample);
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
    }

    /// The time of the oldest sample.
    pub fn oldest_at_ms(&self) -> Option<i64> {
        self.samples.front().map(|sample| sample.at_ms)
    }

    /// True when it is time for the next sample.
    pub fn should_sample(&self, now_ms: i64) -> bool {
        match self.samples.back() {
            None => true,
            Some(last) => now_ms.saturating_sub(last.at_ms) >= INTERVAL_MS,
        }
    }

    /// The one line the phone and the laptop show.
    pub fn summary_line(&self, _now_ms: i64, offset_minutes: i32) -> String {
        let known: Vec<&SignalSample> = self.samples.iter().filter(|s| s.level <= 4).collect();
        if known.is_empty() {
            return "Signal: unavailable.".to_string();
        }
        let current = known.last().expect("a known sample");
        let weakest = known
            .iter()
            .min_by_key(|sample| sample.level)
            .expect("a known sample");
        format!(
            "Signal: {}/4 now; weakest {}/4 at {}.",
            current.level,
            weakest.level,
            clock_hhmm(weakest.at_ms, offset_minutes)
        )
    }
}

fn clock_hhmm(at_ms: i64, offset_minutes: i32) -> String {
    let minutes = (at_ms + offset_minutes as i64 * 60_000).div_euclid(60_000);
    let minutes = minutes.rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}
