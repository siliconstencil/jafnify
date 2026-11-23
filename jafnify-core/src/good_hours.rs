//! Which hours of the day the phone's network is usually quick or busy.
//!
//! Pure: the caller hands over the phone's summary numbers with the time and
//! gets back hourly records, a summary by local hour and the finished lines.
//! Nothing here reads the clock, touches a file or opens a socket.

use std::collections::BTreeSet;

use crate::phone_stats::NetworkKind;
use crate::usage::{hour_start_ms, local_day, local_hour, window_bounds};

/// The summary looks at the last 14 local days.
pub const SUMMARY_DAYS: u32 = 14;
/// A local hour needs data from this many distinct days before it gets a
/// verdict.
pub const MIN_DAYS: u32 = 3;
/// At most this many good ranges and this many busy ranges are shown.
pub const MAX_RANGES: usize = 2;
/// An hour is quick when its median is at most this share of the overall
/// median.
pub const QUICK_NUM: u64 = 80;
pub const QUICK_DEN: u64 = 100;
/// An hour is busy when its median is at least this share of the overall
/// median.
pub const BUSY_NUM: u64 = 125;
pub const BUSY_DEN: u64 = 100;
/// The dim line about the places: the van moves.
pub const PLACE_NOTE: &str =
    "Places change the network too; this learns from where the laptop has been.";
/// The heat needs this many hot hours before it is compared.
pub const MIN_HOT_HOURS: usize = 3;
/// The hot hours must come from this many distinct days.
pub const MIN_HOT_DAYS: usize = 2;
/// The other hours need this many before the comparison is worth it.
pub const MIN_OTHER_HOURS: usize = 3;
/// The heading of the strip of the usual hours.
pub const HOURS_TITLE: &str = "USUAL HOURS";
/// The line of a strip that has no verdict at all.
pub const NO_VERDICT_YET: &str = "No quick or busy hours yet";

/// One sample of the phone's summary.
///
/// The signal strength in dBm is taken but not kept: the speed verdict uses
/// the DNS answer time, and the hour record has no dBm key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub at_ms: i64,
    pub net: NetworkKind,
    pub signal: Option<u8>,
    /// Reserved: the signal strength in dBm is taken but not kept, the hour
    /// record has no dBm key.
    pub dbm: Option<i16>,
    /// The phone's DNS answer time. Only the samples with one count for the
    /// speed verdict.
    pub dns_ms: Option<u32>,
    /// The phone's temperature in tenths of a degree Celsius.
    pub temp_c_x10: Option<i16>,
    /// Android's thermal status, 0 to 6.
    pub thr: Option<u8>,
}

/// One UTC hour of samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HourRecord {
    pub hour_start_ms: i64,
    /// How many samples the hour holds.
    pub samples: u32,
    /// The median DNS answer time of the hour.
    pub dns_ms: Option<u32>,
    /// The share of the samples that were on 5G, in percent.
    pub five_g_pct: u8,
    /// The median signal level of the hour.
    pub signal: Option<u8>,
    /// The median temperature of the hour in tenths of a degree Celsius.
    pub temp_c_x10: Option<i16>,
    /// The highest thermal status of the hour, 0 to 6.
    pub thr: Option<u8>,
    /// True while the hour is not over. A half hour never gives a day: it is
    /// kept in the file so the hour is not lost, but the learning waits for
    /// the hour to end.
    pub partial: bool,
}

impl HourRecord {
    /// The stable one-line format. An unknown value is not written. Only a
    /// half hour carries the extra key.
    pub fn line(&self) -> String {
        let mut line = format!("JFG1 1 hour={} n={}", self.hour_start_ms, self.samples);
        if let Some(dns) = self.dns_ms {
            line.push_str(&format!(" rtt={dns}"));
        }
        line.push_str(&format!(" g5={}", self.five_g_pct));
        if let Some(signal) = self.signal {
            line.push_str(&format!(" sig={signal}"));
        }
        if let Some(temp) = self.temp_c_x10 {
            line.push_str(&format!(" tmp={temp}"));
        }
        if let Some(thr) = self.thr {
            line.push_str(&format!(" thr={thr}"));
        }
        if self.partial {
            line.push_str(" part=1");
        }
        line
    }

    /// Parse one line. Unknown fields are ignored; a line from before a field
    /// existed loads it as unknown. Bad prefixes, versions and values are
    /// rejected.
    pub fn parse_line(line: &str) -> Result<Self, GoodHoursParseError> {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("JFG1") {
            return Err(GoodHoursParseError::BadPrefix);
        }
        match tokens.next() {
            Some("1") => {}
            Some(_) => return Err(GoodHoursParseError::BadVersion),
            None => return Err(GoodHoursParseError::BadVersion),
        }

        let mut hour = None;
        let mut samples = None;
        let mut dns = None;
        let mut five_g = None;
        let mut signal = None;
        let mut temp = None;
        let mut thr = None;
        let mut partial = None;

        for token in tokens {
            let Some((key, value)) = token.split_once('=') else {
                return Err(GoodHoursParseError::Malformed);
            };
            if key.is_empty() || value.is_empty() {
                return Err(GoodHoursParseError::Malformed);
            }
            match key {
                "hour" => set(
                    &mut hour,
                    value
                        .parse::<i64>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "n" => set(
                    &mut samples,
                    value
                        .parse::<u32>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "rtt" => set(
                    &mut dns,
                    value
                        .parse::<u32>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "g5" => set(
                    &mut five_g,
                    value
                        .parse::<u8>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "sig" => set(
                    &mut signal,
                    value
                        .parse::<u8>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "tmp" => set(
                    &mut temp,
                    value
                        .parse::<i16>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "thr" => set(
                    &mut thr,
                    value
                        .parse::<u8>()
                        .map_err(|_| GoodHoursParseError::Malformed)?,
                )?,
                "part" => set(
                    &mut partial,
                    match value
                        .parse::<u8>()
                        .map_err(|_| GoodHoursParseError::Malformed)?
                    {
                        0 => false,
                        1 => true,
                        _ => return Err(GoodHoursParseError::Malformed),
                    },
                )?,
                _ => {}
            }
        }

        Ok(HourRecord {
            hour_start_ms: hour.ok_or(GoodHoursParseError::Malformed)?,
            samples: samples.ok_or(GoodHoursParseError::Malformed)?,
            dns_ms: dns,
            five_g_pct: five_g.unwrap_or(0),
            signal,
            temp_c_x10: temp,
            thr,
            partial: partial.unwrap_or(false),
        })
    }
}

/// Why a good hours line could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoodHoursParseError {
    BadPrefix,
    BadVersion,
    Malformed,
}

impl std::fmt::Display for GoodHoursParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoodHoursParseError::BadPrefix => {
                formatter.write_str("good hours line does not start with JFG1")
            }
            GoodHoursParseError::BadVersion => {
                formatter.write_str("unsupported good hours line version")
            }
            GoodHoursParseError::Malformed => formatter.write_str("malformed good hours line"),
        }
    }
}

impl std::error::Error for GoodHoursParseError {}

fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), GoodHoursParseError> {
    if slot.is_some() {
        return Err(GoodHoursParseError::Malformed);
    }
    *slot = Some(value);
    Ok(())
}

/// Turns a feed of samples into hourly records. The time comes from the
/// caller, so the tests can use a fake clock.
#[derive(Debug, Clone, Default)]
pub struct Sampler {
    hour_start_ms: Option<i64>,
    samples: u32,
    five_g: u32,
    dns: Vec<u32>,
    signal: Vec<u8>,
    temp: Vec<i16>,
    thr: Option<u8>,
}

impl Sampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one sample. When the sample belongs to a new hour, the finished
    /// hour is returned.
    pub fn push(&mut self, sample: Sample) -> Option<HourRecord> {
        let hour = hour_start_ms(sample.at_ms);
        let current = self.hour_start_ms;
        let finished = if current.is_some() && current != Some(hour) {
            self.take()
        } else {
            None
        };
        self.hour_start_ms = Some(hour);
        self.samples = self.samples.saturating_add(1);
        if sample.net.generation().map(|(generation, _)| generation) == Some(5) {
            self.five_g = self.five_g.saturating_add(1);
        }
        // Only the mobile network says how busy the mobile network is: a
        // sample from the phone's Wi-Fi is not about the mobile hours.
        if sample.net.is_mobile() {
            if let Some(dns) = sample.dns_ms {
                self.dns.push(dns);
            }
        }
        if let Some(signal) = sample.signal {
            self.signal.push(signal);
        }
        if let Some(temp) = sample.temp_c_x10 {
            self.temp.push(temp);
        }
        if let Some(thr) = sample.thr {
            self.thr = Some(self.thr.map_or(thr, |current| current.max(thr)));
        }
        finished
    }

    /// The record of the hour being filled. The hour is not over, so the
    /// record is a half hour.
    pub fn current(&self) -> Option<HourRecord> {
        self.hour_start_ms.map(|hour| self.record(hour, true))
    }

    /// Finish the hour being filled and return it.
    pub fn take(&mut self) -> Option<HourRecord> {
        let hour = self.hour_start_ms.take()?;
        let record = self.record(hour, false);
        self.samples = 0;
        self.five_g = 0;
        self.dns.clear();
        self.signal.clear();
        self.temp.clear();
        self.thr = None;
        Some(record)
    }

    fn record(&self, hour: i64, partial: bool) -> HourRecord {
        HourRecord {
            hour_start_ms: hour,
            samples: self.samples,
            dns_ms: median_u32(&self.dns),
            five_g_pct: if self.samples == 0 {
                0
            } else {
                ((self.five_g as u64 * 100 + self.samples as u64 / 2) / self.samples as u64) as u8
            },
            signal: median_u8(&self.signal),
            temp_c_x10: median_i16(&self.temp),
            thr: self.thr,
            partial,
        }
    }
}

/// The verdict of a local hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Quick,
    Busy,
    Neither,
}

/// The place of the line that holds the hour of `line`, if the store already
/// has one. A line that does not parse has no place.
pub fn line_of_hour(lines: &[String], line: &str) -> Option<usize> {
    let hour = HourRecord::parse_line(line).ok()?.hour_start_ms;
    lines.iter().position(|old| {
        HourRecord::parse_line(old)
            .map(|old| old.hour_start_ms == hour)
            .unwrap_or(false)
    })
}

/// The verdict of an hour against the median of all the hours that have one.
pub fn verdict_for(hour_ms: u64, baseline_ms: u64) -> Verdict {
    if hour_ms.saturating_mul(QUICK_DEN) <= baseline_ms.saturating_mul(QUICK_NUM) {
        Verdict::Quick
    } else if hour_ms.saturating_mul(BUSY_DEN) >= baseline_ms.saturating_mul(BUSY_NUM) {
        Verdict::Busy
    } else {
        Verdict::Neither
    }
}

/// One local hour of the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalHour {
    /// The local hour of the day, 0 to 23.
    pub hour: u8,
    /// The median of the hourly medians of this local hour.
    pub dns_ms: u32,
    /// How many distinct days contributed.
    pub days: u32,
}

/// One range of local hours with the same verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub verdict: Verdict,
    /// The first local hour of the range.
    pub start_hour: u8,
    /// The last local hour of the range.
    pub end_hour: u8,
    /// How many hours the range holds.
    pub hours: u8,
    /// The median of the hours in the range.
    pub dns_ms: u32,
}

impl Range {
    /// The line the laptop shows.
    pub fn line(&self) -> String {
        let label = match self.verdict {
            Verdict::Quick => "Usually quick",
            Verdict::Busy => "Usually busy",
            Verdict::Neither => "Neither",
        };
        let hours = if self.start_hour == self.end_hour {
            format!("{:02}:00", self.start_hour)
        } else {
            format!("{:02}:00-{:02}:00", self.start_hour, self.end_hour)
        };
        format!("{label}: {hours} (DNS about {} ms)", self.dns_ms)
    }
}

/// The good hours of the last 14 local days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// The local hours that have data.
    pub hours: Vec<LocalHour>,
    /// How many distinct local days have data.
    pub days: u32,
    /// The median of the hours that have a verdict; 0 when there is none.
    pub baseline_ms: u32,
}

impl Summary {
    /// The summary of the records of the last 14 local days. The caller
    /// supplies the UTC offset in minutes; the model never reads a timezone
    /// database.
    pub fn build(records: &[HourRecord], now_ms: i64, utc_offset_minutes: i32) -> Self {
        let offset_ms = utc_offset_minutes as i64 * 60_000;
        let (start, end) = window_bounds(now_ms, offset_ms, SUMMARY_DAYS);

        let mut per_hour: Vec<(Vec<u32>, Vec<i64>)> = vec![(Vec::new(), Vec::new()); 24];
        let mut days: Vec<i64> = Vec::new();
        for record in records
            .iter()
            .filter(|record| record.hour_start_ms >= start && record.hour_start_ms < end)
            .filter(|record| !record.partial)
        {
            // Only the hours with a DNS answer time count for the speed.
            let Some(dns) = record.dns_ms else {
                continue;
            };
            let hour = local_hour(record.hour_start_ms, offset_ms) as usize;
            let day = local_day(record.hour_start_ms, offset_ms);
            per_hour[hour].0.push(dns);
            if !per_hour[hour].1.contains(&day) {
                per_hour[hour].1.push(day);
            }
            if !days.contains(&day) {
                days.push(day);
            }
        }

        let hours: Vec<LocalHour> = (0..24)
            .filter_map(|index| {
                let (dns, day_list) = &per_hour[index];
                if dns.is_empty() {
                    return None;
                }
                Some(LocalHour {
                    hour: index as u8,
                    dns_ms: median_u32(dns).unwrap_or(0),
                    days: day_list.len() as u32,
                })
            })
            .collect();
        let candidates: Vec<u32> = hours
            .iter()
            .filter(|hour| hour.days >= MIN_DAYS)
            .map(|hour| hour.dns_ms)
            .collect();

        Summary {
            hours,
            days: days.len() as u32,
            baseline_ms: median_u32(&candidates).unwrap_or(0),
        }
    }

    /// The learning line, while there is no hour with its three days yet. The
    /// number is the days of the closest hour, so it never says "3 of 3".
    pub fn learning_line(&self) -> Option<String> {
        let learning: u32 = self
            .hours
            .iter()
            .filter(|hour| hour.days < MIN_DAYS)
            .map(|hour| hour.days)
            .max()
            .unwrap_or(0);
        if learning == 0 && !self.hours.is_empty() {
            // Every hour has its three days: the learning is done.
            return None;
        }
        Some(format!(
            "Learning the busy hours: {} of {} days so far",
            learning, MIN_DAYS
        ))
    }

    /// The caption of the strip of the hours: the line that says why the
    /// strip looks the way it does. A strip with no verdict at all says so
    /// in one line, so no strip is ever all grey without a word.
    pub fn strip_caption(&self) -> Option<String> {
        if let Some(learning) = self.learning_line() {
            return Some(learning);
        }
        // The learning is done and no hour is quick or busy: the strip is all
        // grey and says why.
        if self.ranges().is_empty() {
            return Some(NO_VERDICT_YET.to_string());
        }
        None
    }

    /// The good and busy ranges: the good ones first, then the busy ones, the
    /// longest first in each group.
    pub fn ranges(&self) -> Vec<Range> {
        let mut quick: BTreeSet<u8> = BTreeSet::new();
        let mut busy: BTreeSet<u8> = BTreeSet::new();
        for hour in &self.hours {
            if hour.days < MIN_DAYS {
                continue;
            }
            match verdict_for(hour.dns_ms as u64, self.baseline_ms as u64) {
                Verdict::Quick => {
                    quick.insert(hour.hour);
                }
                Verdict::Busy => {
                    busy.insert(hour.hour);
                }
                Verdict::Neither => {}
            }
        }
        let mut ranges = self.merged_ranges(&quick, Verdict::Quick);
        ranges.extend(self.merged_ranges(&busy, Verdict::Busy));
        ranges
    }

    /// The lines of the ranges.
    pub fn range_lines(&self) -> Vec<String> {
        self.ranges().iter().map(|range| range.line()).collect()
    }

    /// The line about the current local hour; none when the hour has no
    /// verdict yet.
    pub fn now_line(&self, now_ms: i64, utc_offset_minutes: i32) -> Option<String> {
        let offset_ms = utc_offset_minutes as i64 * 60_000;
        let hour = local_hour(now_ms, offset_ms);
        let entry = self.hours.iter().find(|entry| entry.hour == hour)?;
        if entry.days < MIN_DAYS {
            return None;
        }
        match verdict_for(entry.dns_ms as u64, self.baseline_ms as u64) {
            Verdict::Quick => Some("Now is usually a quick hour".to_string()),
            Verdict::Busy => Some("Now is usually a busy hour".to_string()),
            Verdict::Neither => None,
        }
    }

    /// The lines of the section: the learning line, the ranges and the now
    /// line.
    pub fn lines(&self, now_ms: i64, utc_offset_minutes: i32) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(learning) = self.learning_line() {
            lines.push(learning);
        }
        lines.extend(self.range_lines());
        if let Some(now) = self.now_line(now_ms, utc_offset_minutes) {
            lines.push(now);
        }
        lines
    }

    fn merged_ranges(&self, members: &BTreeSet<u8>, verdict: Verdict) -> Vec<Range> {
        let mut ranges = Vec::new();
        for (start_hour, end_hour) in merge_hours(members) {
            let count = (end_hour as i32 - start_hour as i32).rem_euclid(24) as u32 + 1;
            let values: Vec<u32> = self
                .hours
                .iter()
                .filter(|hour| in_range(hour.hour, start_hour, end_hour))
                .map(|hour| hour.dns_ms)
                .collect();
            ranges.push(Range {
                verdict,
                start_hour,
                end_hour,
                hours: count as u8,
                dns_ms: median_u32(&values).unwrap_or(0),
            });
        }
        ranges.sort_by(|left, right| {
            right
                .hours
                .cmp(&left.hours)
                .then(left.start_hour.cmp(&right.start_hour))
        });
        ranges.truncate(MAX_RANGES);
        ranges
    }
}

/// The comparison of the hot hours with the other hours. It needs at least
/// three hot hours on at least two days and at least three other hours, else
/// it says nothing.
pub fn hot_hour_evidence(
    records: &[HourRecord],
    now_ms: i64,
    utc_offset_minutes: i32,
) -> Option<String> {
    let offset_ms = utc_offset_minutes as i64 * 60_000;
    let (start, end) = window_bounds(now_ms, offset_ms, SUMMARY_DAYS);

    let mut hot_dns: Vec<u32> = Vec::new();
    let mut hot_days: BTreeSet<i64> = BTreeSet::new();
    let mut other_dns: Vec<u32> = Vec::new();
    for record in records
        .iter()
        .filter(|record| record.hour_start_ms >= start && record.hour_start_ms < end)
        .filter(|record| !record.partial)
    {
        // Only the hours with a measured temperature and a DNS answer time can
        // be compared. An hour without a temperature is neither hot nor other.
        let (Some(dns), Some(temp)) = (record.dns_ms, record.temp_c_x10) else {
            continue;
        };
        if temp >= crate::phone_heat::HOT_C_X10 {
            hot_dns.push(dns);
            hot_days.insert(local_day(record.hour_start_ms, offset_ms));
        } else {
            other_dns.push(dns);
        }
    }

    if hot_dns.len() < MIN_HOT_HOURS
        || hot_days.len() < MIN_HOT_DAYS
        || other_dns.len() < MIN_OTHER_HOURS
    {
        return None;
    }
    let hot = median_u32(&hot_dns)?;
    let other = median_u32(&other_dns)?;
    if other == 0 {
        return None;
    }
    let ratio = f64::from(hot) / f64::from(other);
    let claim = if ratio >= BUSY_NUM as f64 / BUSY_DEN as f64 {
        format!("DNS was usually {ratio:.1}x slower")
    } else if ratio <= QUICK_NUM as f64 / QUICK_DEN as f64 {
        format!("DNS was usually {:.1}x faster", 1.0 / ratio)
    } else {
        "DNS was about the same".to_string()
    };
    Some(format!(
        "When the phone was {} C or more, {claim} ({} hot hours)",
        crate::phone_heat::HOT_C_X10 / 10,
        hot_dns.len()
    ))
}

/// The runs of neighbouring hours. A run may wrap past midnight.
fn merge_hours(hours: &BTreeSet<u8>) -> Vec<(u8, u8)> {
    if hours.is_empty() {
        return Vec::new();
    }
    if hours.len() == 24 {
        return vec![(0, 23)];
    }
    // The walk starts right after a gap, so a run that wraps past midnight is
    // not split in two.
    let mut start = 0;
    for hour in 0..24u8 {
        if !hours.contains(&hour) {
            start = (hour + 1) % 24;
            break;
        }
    }
    let mut ranges = Vec::new();
    let mut run: Option<(u8, u8)> = None;
    for step in 0..24usize {
        let hour = ((start as usize + step) % 24) as u8;
        if hours.contains(&hour) {
            run = Some(match run {
                None => (hour, hour),
                Some((first, _)) => (first, hour),
            });
        } else if let Some(range) = run.take() {
            ranges.push(range);
        }
    }
    if let Some(range) = run {
        ranges.push(range);
    }
    ranges
}

/// True when the hour lies in the range, counting forward from the start.
fn in_range(hour: u8, start: u8, end: u8) -> bool {
    let span = (end as i32 - start as i32).rem_euclid(24);
    let offset = (hour as i32 - start as i32).rem_euclid(24);
    offset <= span
}

/// The median of the values. An even number takes the mean of the two middle
/// values, rounded down.
fn median_u32(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some(((sorted[middle - 1] as u64 + sorted[middle] as u64) / 2) as u32)
    }
}

/// The median of the values, the same way as the DNS times.
fn median_u8(values: &[u8]) -> Option<u8> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some(((sorted[middle - 1] as u16 + sorted[middle] as u16) / 2) as u8)
    }
}

/// The median of the temperatures, the same way as the DNS times.
fn median_i16(values: &[i16]) -> Option<i16> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some(((i32::from(sorted[middle - 1]) + i32::from(sorted[middle])) / 2) as i16)
    }
}
