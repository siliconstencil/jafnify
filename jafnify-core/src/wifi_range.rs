//! How far the laptop is from the phone, from the Wi-Fi signal the laptop
//! hears from the hotspot. The platforms only read the level in dBm; the
//! thresholds, the words and the tile live here, so every platform says the
//! same thing about the same number.
//!
//! The signal is a hint of the distance, never a measure of it: walls, a
//! pocket or the angle of the phone weaken it too, so every word says
//! "likely".

use std::collections::VecDeque;

use crate::tiles::{line_trend, window_fact, BigValue, Facts, Tile, Tone, TrendBuffer};

/// The id of the range tile.
pub const ID_RANGE: &str = "range";

/// At or above this level the laptop is likely close to the phone.
pub const CLOSE_DBM: i32 = -60;
/// At or above this level the distance is likely fine.
pub const FAIR_DBM: i32 = -70;
/// At or below this level the laptop is likely at the edge of the range of
/// the hotspot: a drop that follows such a level is likely the distance.
pub const OUT_OF_RANGE_DBM: i32 = -78;
/// How far back before a drop the weakest level is looked for. A drop is
/// noticed seconds after the link went; by then the laptop may have lost the
/// hotspot and have no level at all.
pub const LOOKBACK_MS: i64 = 30_000;

/// The words of the range under a level.
pub const WORD_CLOSE: &str = "Likely close to the phone";
pub const WORD_FAIR: &str = "Likely a fair distance";
pub const WORD_FAR: &str = "Likely far from the phone";
pub const WORD_EDGE: &str = "Likely at the edge of range";

/// The level of `iface` in the text of `/proc/net/wireless`, in dBm. None
/// when the interface is not listed (not associated, or no wireless
/// extensions), when the driver reports no level, or when the number makes
/// no sense.
pub fn parse_proc_wireless(text: &str, iface: &str) -> Option<i32> {
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != iface {
            continue;
        }
        // status, link quality, level, noise, ...
        let level = rest.split_whitespace().nth(2)?;
        let level: f64 = level.trim_end_matches('.').parse().ok()?;
        let mut dbm = level.round() as i32;
        // Some drivers report the level as an unsigned byte.
        if dbm > 0 {
            dbm -= 256;
        }
        return (-120..0).contains(&dbm).then_some(dbm);
    }
    None
}

/// The word of the range under `dbm`.
pub fn range_word(dbm: i32) -> &'static str {
    if dbm >= CLOSE_DBM {
        WORD_CLOSE
    } else if dbm >= FAIR_DBM {
        WORD_FAIR
    } else if dbm > OUT_OF_RANGE_DBM {
        WORD_FAR
    } else {
        WORD_EDGE
    }
}

/// The weakest level of the ring from the lookback before `at_ms` (the
/// moment the link went) up to `detected_ms` (the moment the drop was seen).
pub fn weakest_before(trend: &TrendBuffer, at_ms: i64, detected_ms: i64) -> Option<i32> {
    trend
        .min_between(at_ms - LOOKBACK_MS, detected_ms.max(at_ms))
        .map(|value| value as i32)
}

/// What the range tile shows, on the laptop and on the phone alike: the
/// laptop measures it, the phone gets it over the stats channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeReport {
    /// The level that stands for the link, in dBm.
    pub level_dbm: i32,
    /// The rate the laptop sends at, in units of 100 kbit/s.
    pub rate_100k: Option<u32>,
    /// Whether beacons of the hotspot were missed in the last seconds.
    pub missed_beacons: bool,
}

impl RangeReport {
    /// The report of the newest reading and the evidence of the seconds
    /// before it. None when the reading has no level.
    pub fn of(sample: &StationSample, recent: &LinkEvidence) -> Option<Self> {
        Some(Self {
            level_dbm: sample.level()?,
            rate_100k: sample.tx_rate_100k,
            missed_beacons: recent.beacon_loss.is_some_and(|n| n > 0),
        })
    }

    /// The words of the report on the stats channel, ` key=value` each.
    pub fn wire_words(&self) -> String {
        let mut out = format!(" lvl={}", self.level_dbm);
        if let Some(rate) = self.rate_100k {
            out.push_str(&format!(" rate={rate}"));
        }
        out.push_str(&format!(" mb={}", u8::from(self.missed_beacons)));
        out
    }

    /// The report from the words of the stats channel. None when the level
    /// is missing or a value does not parse.
    pub fn from_wire_words<'a>(words: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut level = None;
        let mut rate = None;
        let mut missed = false;
        for word in words {
            let (key, value) = word.split_once('=')?;
            match key {
                "lvl" => level = Some(value.parse::<i32>().ok()?),
                "rate" => rate = Some(value.parse::<u32>().ok()?),
                "mb" => missed = value == "1",
                _ => {}
            }
        }
        let level_dbm = level.filter(|dbm| (-120..0).contains(dbm))?;
        Some(Self {
            level_dbm,
            rate_100k: rate,
            missed_beacons: missed,
        })
    }
}

/// A rate in units of 100 kbit/s as words: "433.3 Mbit/s".
pub fn rate_words(rate_100k: u32) -> String {
    format!("{}.{} Mbit/s", rate_100k / 10, rate_100k % 10)
}

/// The tile of the range: the level now, its line over the window, the
/// window in words, the likely distance, and the rate of the link as the
/// note. Muted while the distance is fine; the waiting tone when the laptop
/// is far or beacons of the hotspot go missing, so a coming drop can be seen
/// before it happens. It is never red: a level is not a fault.
pub fn range_tile(report: &RangeReport, trend: &TrendBuffer) -> Tile {
    let dbm = report.level_dbm;
    let mut lines = Vec::new();
    if let Some(fact) = window_fact(trend, "dBm", " to ") {
        lines.push(fact);
    }
    lines.push(range_word(dbm).to_string());
    let tone = if dbm < FAIR_DBM || report.missed_beacons {
        Tone::Wait
    } else {
        Tone::Muted
    };
    let note = match (report.rate_100k, report.missed_beacons) {
        (Some(rate), true) => Some(format!("{}, beacons missed", rate_words(rate))),
        (Some(rate), false) => Some(format!("Link {}", rate_words(rate))),
        (None, true) => Some("Beacons missed".to_string()),
        (None, false) => None,
    };
    Tile {
        id: ID_RANGE.to_string(),
        label: "RANGE".to_string(),
        badge: None,
        big: Some(BigValue {
            value: dbm.to_string(),
            unit: Some("dBm".to_string()),
            tone,
        }),
        trend: line_trend(trend),
        facts: Some(Facts::new(lines)),
        note,
    }
}

/// One reading of the laptop's Wi-Fi link to the hotspot. There is no field
/// for the address of the hotspot on purpose: the BSSID identifies the phone
/// and must never reach a log, a record or an export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StationSample {
    /// The level of the last frame, in dBm.
    pub signal_dbm: Option<i32>,
    /// The level averaged by the driver, in dBm.
    pub signal_avg_dbm: Option<i32>,
    /// The rate the laptop sends at, in units of 100 kbit/s.
    pub tx_rate_100k: Option<u32>,
    /// Frames sent again since the association (a counter).
    pub tx_retries: Option<u32>,
    /// Frames that never got through since the association (a counter).
    pub tx_failed: Option<u32>,
    /// Beacons of the hotspot that were missed (a counter).
    pub beacon_loss: Option<u32>,
}

impl StationSample {
    /// A sample with only a level, as the fallback reader gives it.
    pub fn level_only(dbm: i32) -> Self {
        Self {
            signal_dbm: Some(dbm),
            ..Self::default()
        }
    }

    /// The level that stands for the sample: the average when the driver
    /// keeps one, the last frame otherwise.
    pub fn level(&self) -> Option<i32> {
        self.signal_avg_dbm.or(self.signal_dbm)
    }
}

/// The readings of the last minutes, oldest first.
#[derive(Debug, Clone, Default)]
pub struct StationRing {
    samples: VecDeque<(i64, StationSample)>,
}

/// How long the ring keeps its readings.
pub const RING_WINDOW_MS: i64 = 300_000;

impl StationRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// One more reading; readings older than the window are forgotten.
    pub fn push(&mut self, at_ms: i64, sample: StationSample) {
        self.samples.push_back((at_ms, sample));
        let limit = at_ms - RING_WINDOW_MS;
        while self.samples.front().is_some_and(|(at, _)| *at < limit) {
            self.samples.pop_front();
        }
    }

    /// The newest reading when it was taken at or after `from_ms`.
    pub fn latest_since(&self, from_ms: i64) -> Option<StationSample> {
        self.samples
            .back()
            .filter(|(at, _)| *at >= from_ms)
            .map(|(_, sample)| *sample)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// What the link looked like from the lookback before `at_ms` (the
    /// moment the link went) up to `detected_ms` (the moment the drop was
    /// seen): the weakest level, the lowest rate and how much each counter
    /// grew. A counter that went down (a new association) tells nothing.
    pub fn evidence(&self, at_ms: i64, detected_ms: i64) -> LinkEvidence {
        let from = at_ms - LOOKBACK_MS;
        let to = detected_ms.max(at_ms);
        let window: Vec<&StationSample> = self
            .samples
            .iter()
            .filter(|(at, _)| (from..=to).contains(at))
            .map(|(_, sample)| sample)
            .collect();
        let growth = |get: fn(&StationSample) -> Option<u32>| {
            let values: Vec<u32> = window.iter().filter_map(|s| get(s)).collect();
            match (values.first(), values.last()) {
                (Some(first), Some(last)) if last >= first && values.len() > 1 => {
                    Some(last - first)
                }
                _ => None,
            }
        };
        LinkEvidence {
            weakest_dbm: window.iter().filter_map(|s| s.level()).min(),
            lowest_rate_100k: window.iter().filter_map(|s| s.tx_rate_100k).min(),
            retries: growth(|s| s.tx_retries),
            failed: growth(|s| s.tx_failed),
            beacon_loss: growth(|s| s.beacon_loss),
            phone_loss_pct: None,
            phone_rtt_ms: None,
        }
    }
}

/// The state of the Wi-Fi link around a drop, from both ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkEvidence {
    /// The weakest level the laptop heard, in dBm.
    pub weakest_dbm: Option<i32>,
    /// The lowest rate the laptop sent at, in units of 100 kbit/s.
    pub lowest_rate_100k: Option<u32>,
    /// Frames sent again in the window.
    pub retries: Option<u32>,
    /// Frames that never got through in the window.
    pub failed: Option<u32>,
    /// Beacons of the hotspot missed in the window.
    pub beacon_loss: Option<u32>,
    /// The share of the laptop's packets the phone missed, in percent.
    pub phone_loss_pct: Option<u8>,
    /// The handshake round trip the phone measured, in milliseconds.
    pub phone_rtt_ms: Option<u32>,
}

/// Below this level a missed beacon is likely the distance.
pub const BEACON_LOSS_FAR_DBM: i32 = -72;

/// A drop-line evidence value that does not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadEvidence;

impl LinkEvidence {
    /// Whether the evidence likely says the laptop left the range of the
    /// hotspot: a level at the edge, or beacons missed while already far.
    pub fn out_of_range(&self) -> bool {
        let Some(weakest) = self.weakest_dbm else {
            return false;
        };
        weakest <= OUT_OF_RANGE_DBM
            || (weakest <= BEACON_LOSS_FAR_DBM && self.beacon_loss.is_some_and(|n| n > 0))
    }

    /// The words of the evidence in a drop line, ` key=value` each.
    pub fn line_words(&self) -> String {
        let mut out = String::new();
        let mut put = |key: &str, value: Option<String>| {
            if let Some(value) = value {
                out.push_str(&format!(" {key}={value}"));
            }
        };
        put("range_dbm", self.weakest_dbm.map(|v| v.to_string()));
        put("rate_min", self.lowest_rate_100k.map(|v| v.to_string()));
        put("retry_d", self.retries.map(|v| v.to_string()));
        put("fail_d", self.failed.map(|v| v.to_string()));
        put("bloss_d", self.beacon_loss.map(|v| v.to_string()));
        put("ploss", self.phone_loss_pct.map(|v| v.to_string()));
        put("prtt", self.phone_rtt_ms.map(|v| v.to_string()));
        out
    }

    /// Take one ` key=value` of a drop line; false when the key is not one
    /// of the evidence (the caller then handles it or ignores it). A value
    /// that does not parse is an error.
    pub fn take_word(&mut self, key: &str, value: &str) -> Result<bool, BadEvidence> {
        fn num<T: std::str::FromStr>(value: &str) -> Result<Option<T>, BadEvidence> {
            value.parse().map(Some).map_err(|_| BadEvidence)
        }
        match key {
            "range_dbm" => self.weakest_dbm = num(value)?,
            "rate_min" => self.lowest_rate_100k = num(value)?,
            "retry_d" => self.retries = num(value)?,
            "fail_d" => self.failed = num(value)?,
            "bloss_d" => self.beacon_loss = num(value)?,
            "ploss" => self.phone_loss_pct = num(value)?,
            "prtt" => self.phone_rtt_ms = num(value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// How often a reading goes to the local link log.
pub const LINK_LOG_EVERY_MS: i64 = 10_000;

/// One line of the local link log: the numbers of a reading and nothing
/// that identifies the hotspot.
pub fn link_log_line(at_ms: i64, sample: &StationSample) -> String {
    let mut line = format!("JFL1 1 at={at_ms}");
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            line.push_str(&format!(" {key}={value}"));
        }
    };
    put("sig", sample.signal_dbm.map(|v| v.to_string()));
    put("avg", sample.signal_avg_dbm.map(|v| v.to_string()));
    put("rate", sample.tx_rate_100k.map(|v| v.to_string()));
    put("retry", sample.tx_retries.map(|v| v.to_string()));
    put("fail", sample.tx_failed.map(|v| v.to_string()));
    put("bloss", sample.beacon_loss.map(|v| v.to_string()));
    line
}
