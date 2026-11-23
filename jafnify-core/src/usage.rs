//! Pure usage statistics: one bucket per UTC hour, a sampler that only uses
//! injected timestamps and cumulative counters, summaries for the current day,
//! seven days and thirty days, and the one-line bucket format. Nothing here
//! reads the clock, touches a file or opens a socket.

use crate::phone_stats::NetworkKind;

/// Milliseconds in one hour.
pub const HOUR_MS: i64 = 3_600_000;
/// Milliseconds in one day.
pub const DAY_MS: i64 = 86_400_000;
/// Thirty days of hourly buckets, oldest dropped.
pub const MAX_BUCKETS: usize = 720;

/// One UTC hour of usage counters. Rates are bytes per second; 0 on a minimum
/// or peak means "no sample".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageBucket {
    pub hour_start_ms: i64,
    pub up_bytes: u64,
    pub down_bytes: u64,
    pub min_up_bps: u64,
    pub min_down_bps: u64,
    pub peak_up_bps: u64,
    pub peak_down_bps: u64,
    pub connected_ms: u64,
    pub samples: u32,
    pub network: NetworkKind,
}

impl UsageBucket {
    /// The stable one-line format. Unknown fields are not written.
    pub fn line(&self) -> String {
        format!(
            "JFU1 1 hour={} up={} down={} min_up={} min_down={} peak_up={} peak_down={} connected_ms={} samples={} net={}",
            self.hour_start_ms,
            self.up_bytes,
            self.down_bytes,
            self.min_up_bps,
            self.min_down_bps,
            self.peak_up_bps,
            self.peak_down_bps,
            self.connected_ms,
            self.samples,
            self.network.as_str()
        )
    }

    /// Parse one line. Unknown fields are ignored; a line from before the
    /// minimum fields loads them as 0. Bad prefixes, versions and values are
    /// rejected.
    pub fn parse_line(line: &str) -> Result<Self, UsageParseError> {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("JFU1") {
            return Err(UsageParseError::BadPrefix);
        }
        match tokens.next() {
            Some("1") => {}
            Some(_) => return Err(UsageParseError::BadVersion),
            None => return Err(UsageParseError::BadVersion),
        }

        let mut hour = None;
        let mut up = None;
        let mut down = None;
        let mut min_up = None;
        let mut min_down = None;
        let mut peak_up = None;
        let mut peak_down = None;
        let mut connected_ms = None;
        let mut samples = None;
        let mut network = None;

        for token in tokens {
            let Some((key, value)) = token.split_once('=') else {
                return Err(UsageParseError::Malformed);
            };
            if key.is_empty() || value.is_empty() {
                return Err(UsageParseError::Malformed);
            }
            match key {
                "hour" => set(
                    &mut hour,
                    value
                        .parse::<i64>()
                        .map_err(|_| UsageParseError::Malformed)?,
                )?,
                "up" => set(&mut up, parse_u64(value)?)?,
                "down" => set(&mut down, parse_u64(value)?)?,
                "min_up" => set(&mut min_up, parse_u64(value)?)?,
                "min_down" => set(&mut min_down, parse_u64(value)?)?,
                "peak_up" => set(&mut peak_up, parse_u64(value)?)?,
                "peak_down" => set(&mut peak_down, parse_u64(value)?)?,
                "connected_ms" => set(&mut connected_ms, parse_u64(value)?)?,
                "samples" => set(
                    &mut samples,
                    value
                        .parse::<u32>()
                        .map_err(|_| UsageParseError::Malformed)?,
                )?,
                "net" => set(&mut network, NetworkKind::parse(value))?,
                _ => {}
            }
        }

        Ok(UsageBucket {
            hour_start_ms: hour.ok_or(UsageParseError::Malformed)?,
            up_bytes: up.ok_or(UsageParseError::Malformed)?,
            down_bytes: down.ok_or(UsageParseError::Malformed)?,
            min_up_bps: min_up.unwrap_or(0),
            min_down_bps: min_down.unwrap_or(0),
            peak_up_bps: peak_up.ok_or(UsageParseError::Malformed)?,
            peak_down_bps: peak_down.ok_or(UsageParseError::Malformed)?,
            connected_ms: connected_ms.ok_or(UsageParseError::Malformed)?,
            samples: samples.ok_or(UsageParseError::Malformed)?,
            network: network.ok_or(UsageParseError::Malformed)?,
        })
    }
}

/// Why a usage line could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageParseError {
    BadPrefix,
    BadVersion,
    Malformed,
}

impl std::fmt::Display for UsageParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsageParseError::BadPrefix => {
                formatter.write_str("usage line does not start with JFU1")
            }
            UsageParseError::BadVersion => formatter.write_str("unsupported usage line version"),
            UsageParseError::Malformed => formatter.write_str("malformed usage line"),
        }
    }
}

impl std::error::Error for UsageParseError {}

fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), UsageParseError> {
    if slot.is_some() {
        return Err(UsageParseError::Malformed);
    }
    *slot = Some(value);
    Ok(())
}

fn parse_u64(value: &str) -> Result<u64, UsageParseError> {
    value.parse::<u64>().map_err(|_| UsageParseError::Malformed)
}

/// One sampler input: the timestamp, the cumulative counters, whether the
/// link is connected, and the network kind at this instant.
#[derive(Debug, Clone, Copy)]
pub struct UsageSample {
    pub at_ms: i64,
    pub up_bytes: u64,
    pub down_bytes: u64,
    pub connected: bool,
    pub network: NetworkKind,
}

#[derive(Debug, Clone, Copy)]
struct Baseline {
    at_ms: i64,
    up_bytes: u64,
    down_bytes: u64,
}

/// Turns a feed of samples into hourly buckets. The first sample is only a
/// baseline; a decreasing counter starts a new baseline; only connected
/// intervals add bytes, rates and connected time.
#[derive(Debug, Clone, Default)]
pub struct UsageSampler {
    current: Option<UsageBucket>,
    baseline: Option<Baseline>,
}

impl UsageSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bucket for the current hour, if the sampler has seen a sample.
    pub fn current(&self) -> Option<&UsageBucket> {
        self.current.as_ref()
    }

    /// Continue a stored hour after a restart: keep its totals, then take the
    /// first new sample as the baseline so no byte is counted twice.
    pub fn seed(&mut self, bucket: UsageBucket) {
        self.current = Some(bucket);
        self.baseline = None;
    }

    /// Advance with one sample. Returns the previous bucket when the hour
    /// rolls over; the caller appends it to the store.
    pub fn step(&mut self, sample: UsageSample) -> Option<UsageBucket> {
        let hour = hour_start_ms(sample.at_ms);
        let mut completed = None;
        if self.current.as_ref().map(|bucket| bucket.hour_start_ms) != Some(hour) {
            if let Some(previous) = self.current.take() {
                completed = Some(previous);
            }
            self.current = Some(UsageBucket {
                hour_start_ms: hour,
                network: sample.network,
                ..UsageBucket::default()
            });
        }
        let Some(bucket) = self.current.as_mut() else {
            return completed;
        };

        let Some(base) = self.baseline else {
            self.baseline = Some(Baseline {
                at_ms: sample.at_ms,
                up_bytes: sample.up_bytes,
                down_bytes: sample.down_bytes,
            });
            return completed;
        };

        let elapsed = sample.at_ms.saturating_sub(base.at_ms);
        if elapsed < 0 {
            self.baseline = Some(Baseline {
                at_ms: sample.at_ms,
                up_bytes: sample.up_bytes,
                down_bytes: sample.down_bytes,
            });
            return completed;
        }
        if elapsed == 0 {
            return completed;
        }
        if !sample.connected {
            self.baseline = Some(Baseline {
                at_ms: sample.at_ms,
                up_bytes: sample.up_bytes,
                down_bytes: sample.down_bytes,
            });
            return completed;
        }

        bucket.samples = bucket.samples.saturating_add(1);
        bucket.connected_ms = bucket.connected_ms.saturating_add(elapsed as u64);
        bucket.network = sample.network;

        if sample.up_bytes < base.up_bytes || sample.down_bytes < base.down_bytes {
            self.baseline = Some(Baseline {
                at_ms: sample.at_ms,
                up_bytes: sample.up_bytes,
                down_bytes: sample.down_bytes,
            });
            return completed;
        }

        let up_delta = sample.up_bytes - base.up_bytes;
        let down_delta = sample.down_bytes - base.down_bytes;
        bucket.up_bytes = bucket.up_bytes.saturating_add(up_delta);
        bucket.down_bytes = bucket.down_bytes.saturating_add(down_delta);

        if up_delta > 0 {
            let rate = per_second(up_delta, elapsed as u64);
            if rate > 0 {
                bucket.peak_up_bps = bucket.peak_up_bps.max(rate);
                bucket.min_up_bps = if bucket.min_up_bps == 0 {
                    rate
                } else {
                    bucket.min_up_bps.min(rate)
                };
            }
        }
        if down_delta > 0 {
            let rate = per_second(down_delta, elapsed as u64);
            if rate > 0 {
                bucket.peak_down_bps = bucket.peak_down_bps.max(rate);
                bucket.min_down_bps = if bucket.min_down_bps == 0 {
                    rate
                } else {
                    bucket.min_down_bps.min(rate)
                };
            }
        }

        self.baseline = Some(Baseline {
            at_ms: sample.at_ms,
            up_bytes: sample.up_bytes,
            down_bytes: sample.down_bytes,
        });
        completed
    }
}

fn per_second(bytes: u64, elapsed_ms: u64) -> u64 {
    let value = (bytes as u128 * 1_000) / elapsed_ms as u128;
    value.min(u64::MAX as u128) as u64
}

/// The summary window: the current local day, the last seven days, or the
/// last thirty days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageWindow {
    Today,
    SevenDays,
    ThirtyDays,
}

impl UsageWindow {
    pub const ALL: [UsageWindow; 3] = [
        UsageWindow::Today,
        UsageWindow::SevenDays,
        UsageWindow::ThirtyDays,
    ];

    pub fn days(self) -> u32 {
        match self {
            UsageWindow::Today => 1,
            UsageWindow::SevenDays => 7,
            UsageWindow::ThirtyDays => 30,
        }
    }

    fn label(self) -> &'static str {
        match self {
            UsageWindow::Today => "today",
            UsageWindow::SevenDays => "this week",
            UsageWindow::ThirtyDays => "this month",
        }
    }
}

/// One of the three busiest local hours in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusyHour {
    pub hour: u8,
    pub up_bytes: u64,
    pub down_bytes: u64,
}

/// The aggregate for one window. A minimum or peak of 0 means "no sample";
/// `network` is `None` when the connected time has no clear majority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummary {
    pub window: UsageWindow,
    pub up_bytes: u64,
    pub down_bytes: u64,
    pub connected_ms: u64,
    pub average_up_bps: u64,
    pub average_down_bps: u64,
    pub peak_up_bps: u64,
    pub peak_down_bps: u64,
    pub lowest_up_bps: u64,
    pub lowest_down_bps: u64,
    pub busiest: Vec<BusyHour>,
    pub network: Option<NetworkKind>,
}

impl UsageSummary {
    /// Aggregate the buckets whose hour falls in the local day window that
    /// contains `now_ms`. The caller supplies the UTC offset in minutes; the
    /// model never reads the clock or a timezone database.
    pub fn build(
        buckets: &[UsageBucket],
        window: UsageWindow,
        now_ms: i64,
        utc_offset_minutes: i32,
    ) -> Self {
        let offset_ms = utc_offset_minutes as i64 * 60_000;
        let (start, end) = window_bounds(now_ms, offset_ms, window.days());

        let mut up_bytes = 0_u64;
        let mut down_bytes = 0_u64;
        let mut connected_ms = 0_u64;
        let mut peak_up_bps = 0_u64;
        let mut peak_down_bps = 0_u64;
        let mut lowest_up_bps = 0_u64;
        let mut lowest_down_bps = 0_u64;
        let mut per_hour = [(0_u64, 0_u64); 24];
        let mut per_network = [0_u128; NETWORK_ORDER.len()];

        for bucket in buckets
            .iter()
            .filter(|bucket| bucket.hour_start_ms >= start && bucket.hour_start_ms < end)
        {
            up_bytes = up_bytes.saturating_add(bucket.up_bytes);
            down_bytes = down_bytes.saturating_add(bucket.down_bytes);
            connected_ms = connected_ms.saturating_add(bucket.connected_ms);
            peak_up_bps = peak_up_bps.max(bucket.peak_up_bps);
            peak_down_bps = peak_down_bps.max(bucket.peak_down_bps);
            if bucket.min_up_bps > 0 {
                lowest_up_bps = if lowest_up_bps == 0 {
                    bucket.min_up_bps
                } else {
                    lowest_up_bps.min(bucket.min_up_bps)
                };
            }
            if bucket.min_down_bps > 0 {
                lowest_down_bps = if lowest_down_bps == 0 {
                    bucket.min_down_bps
                } else {
                    lowest_down_bps.min(bucket.min_down_bps)
                };
            }
            let hour = local_hour(bucket.hour_start_ms, offset_ms) as usize;
            per_hour[hour].0 = per_hour[hour].0.saturating_add(bucket.up_bytes);
            per_hour[hour].1 = per_hour[hour].1.saturating_add(bucket.down_bytes);
            if bucket.connected_ms > 0 {
                let index = network_index(bucket.network);
                per_network[index] += bucket.connected_ms as u128;
            }
        }

        let mut busiest: Vec<BusyHour> = per_hour
            .iter()
            .enumerate()
            .filter(|(_, (up, down))| *up > 0 || *down > 0)
            .map(|(hour, (up, down))| BusyHour {
                hour: hour as u8,
                up_bytes: *up,
                down_bytes: *down,
            })
            .collect();
        busiest.sort_by(|left, right| {
            let left_total = left.up_bytes as u128 + left.down_bytes as u128;
            let right_total = right.up_bytes as u128 + right.down_bytes as u128;
            right_total
                .cmp(&left_total)
                .then(left.hour.cmp(&right.hour))
        });
        busiest.truncate(3);

        let total_connected: u128 = per_network.iter().sum();
        let network = if total_connected == 0 {
            None
        } else {
            let (index, best) = per_network
                .iter()
                .enumerate()
                .max_by_key(|(_, value)| **value)
                .unwrap_or((0, &0));
            if *best * 2 <= total_connected {
                None
            } else {
                Some(NETWORK_ORDER[index])
            }
        };

        UsageSummary {
            window,
            up_bytes,
            down_bytes,
            connected_ms,
            average_up_bps: average(up_bytes, connected_ms),
            average_down_bps: average(down_bytes, connected_ms),
            peak_up_bps,
            peak_down_bps,
            lowest_up_bps,
            lowest_down_bps,
            busiest,
            network,
        }
    }

    pub fn total_line(&self) -> String {
        format!(
            "Shared {}: {}",
            self.window.label(),
            format_bytes(self.up_bytes.saturating_add(self.down_bytes))
        )
    }

    pub fn average_line(&self) -> String {
        format!(
            "Average: {} down, {} up",
            format_rate(self.average_down_bps),
            format_rate(self.average_up_bps)
        )
    }

    pub fn peak_line(&self) -> String {
        format!(
            "Peak: {} down, {} up",
            format_rate(self.peak_down_bps),
            format_rate(self.peak_up_bps)
        )
    }

    pub fn lowest_line(&self) -> String {
        match (self.lowest_down_bps, self.lowest_up_bps) {
            (0, 0) => "Lowest: no sample".to_string(),
            (down, 0) => format!("Lowest: {} down", format_rate(down)),
            (0, up) => format!("Lowest: {} up", format_rate(up)),
            (down, up) => format!("Lowest: {} down, {} up", format_rate(down), format_rate(up)),
        }
    }

    pub fn busiest_line(&self) -> String {
        if self.busiest.is_empty() {
            return "Busiest: no sample".to_string();
        }
        let hours: Vec<String> = self
            .busiest
            .iter()
            .map(|hour| format!("{:02}:00-{:02}:00", hour.hour, hour.hour as u16 + 1))
            .collect();
        format!("Busiest: {}", hours.join(", "))
    }

    pub fn network_line(&self) -> String {
        if self.connected_ms == 0 {
            return "Network: no sample".to_string();
        }
        match self.network {
            Some(network) => format!("Network: {}", network.display()),
            None => "Network: mixed".to_string(),
        }
    }

    /// The fixed display lines, in the order the windows render them.
    pub fn lines(&self) -> Vec<String> {
        vec![
            self.total_line(),
            self.average_line(),
            self.peak_line(),
            self.lowest_line(),
            self.busiest_line(),
            self.network_line(),
        ]
    }
}

const NETWORK_ORDER: [NetworkKind; 15] = [
    NetworkKind::None,
    NetworkKind::Wifi,
    NetworkKind::Ethernet,
    NetworkKind::Mobile,
    NetworkKind::Mobile2g,
    NetworkKind::Mobile3g,
    NetworkKind::Mobile4g,
    NetworkKind::Mobile4gCa,
    NetworkKind::Mobile4gPlus,
    NetworkKind::Mobile5g,
    NetworkKind::Mobile5gNsa,
    NetworkKind::Mobile5gMmwave,
    NetworkKind::Mobile5gAdv,
    NetworkKind::Other,
    NetworkKind::Unknown,
];

fn network_index(network: NetworkKind) -> usize {
    match network {
        NetworkKind::None => 0,
        NetworkKind::Wifi => 1,
        NetworkKind::Ethernet => 2,
        NetworkKind::Mobile => 3,
        NetworkKind::Mobile2g => 4,
        NetworkKind::Mobile3g => 5,
        NetworkKind::Mobile4g => 6,
        NetworkKind::Mobile4gCa => 7,
        NetworkKind::Mobile4gPlus => 8,
        NetworkKind::Mobile5g => 9,
        NetworkKind::Mobile5gNsa => 10,
        NetworkKind::Mobile5gMmwave => 11,
        NetworkKind::Mobile5gAdv => 12,
        NetworkKind::Other => 13,
        NetworkKind::Unknown => 14,
    }
}

/// Floor a timestamp to its UTC hour.
pub fn hour_start_ms(at_ms: i64) -> i64 {
    at_ms.div_euclid(HOUR_MS).saturating_mul(HOUR_MS)
}

/// The start and the end of the local day window that holds `now_ms`. The
/// caller supplies the UTC offset in minutes.
pub fn window_bounds(now_ms: i64, offset_ms: i64, days: u32) -> (i64, i64) {
    let local_now = now_ms.saturating_add(offset_ms);
    let day_start_local = local_now.div_euclid(DAY_MS).saturating_mul(DAY_MS);
    let end_local = day_start_local.saturating_add(DAY_MS);
    let start_local = end_local.saturating_sub(days as i64 * DAY_MS);
    (
        start_local.saturating_sub(offset_ms),
        end_local.saturating_sub(offset_ms),
    )
}

/// The local hour of an hour start, 0 to 23. The caller supplies the UTC
/// offset in milliseconds.
pub fn local_hour(hour_start_ms: i64, offset_ms: i64) -> u8 {
    hour_start_ms
        .saturating_add(offset_ms)
        .div_euclid(HOUR_MS)
        .rem_euclid(24) as u8
}

/// The local day of an hour start, as a day number. The caller supplies the
/// UTC offset in minutes; the model never reads a timezone database.
pub fn local_day(hour_start_ms: i64, offset_ms: i64) -> i64 {
    hour_start_ms.saturating_add(offset_ms).div_euclid(DAY_MS)
}

fn average(bytes: u64, connected_ms: u64) -> u64 {
    average_rate(bytes, connected_ms)
}

/// The average rate of `bytes` over `elapsed_ms` milliseconds, in bytes per
/// second. Zero elapsed time has no rate.
pub fn average_rate(bytes: u64, elapsed_ms: u64) -> u64 {
    if elapsed_ms == 0 {
        return 0;
    }
    let value = (bytes as u128 * 1_000) / elapsed_ms as u128;
    value.min(u64::MAX as u128) as u64
}

/// Append one bucket and drop the oldest when the cap is exceeded. The caller
/// keeps the vector in time order; the cap drops from the front. A bucket for
/// the same hour as the last one (an app or service restart inside the hour)
/// is merged into it, so the hour is neither counted twice nor lost.
pub fn push_bucket(buckets: &mut Vec<UsageBucket>, bucket: UsageBucket) {
    if let Some(last) = buckets.last_mut() {
        if last.hour_start_ms == bucket.hour_start_ms {
            *last = merge_buckets(*last, bucket);
            return;
        }
    }
    buckets.push(bucket);
    let overflow = buckets.len().saturating_sub(MAX_BUCKETS);
    if overflow > 0 {
        buckets.drain(..overflow);
    }
}

/// Parse a whole store file: bad lines are skipped, the buckets are put in
/// time order and the newest [`MAX_BUCKETS`] are kept (same-hour duplicates
/// merge).
pub fn buckets_from_lines(text: &str) -> Vec<UsageBucket> {
    let mut parsed: Vec<UsageBucket> = text
        .lines()
        .filter_map(|line| UsageBucket::parse_line(line.trim()).ok())
        .collect();
    parsed.sort_by_key(|bucket| bucket.hour_start_ms);
    let mut buckets = Vec::new();
    for bucket in parsed {
        push_bucket(&mut buckets, bucket);
    }
    buckets
}

/// Two partial buckets of the same hour as one. Totals add; the lowest keeps
/// the smaller positive value; the peak keeps the larger; the network is the
/// later one.
fn merge_buckets(a: UsageBucket, b: UsageBucket) -> UsageBucket {
    fn min_positive(x: u64, y: u64) -> u64 {
        match (x, y) {
            (0, v) | (v, 0) => v,
            (x, y) => x.min(y),
        }
    }
    UsageBucket {
        hour_start_ms: a.hour_start_ms,
        up_bytes: a.up_bytes.saturating_add(b.up_bytes),
        down_bytes: a.down_bytes.saturating_add(b.down_bytes),
        min_up_bps: min_positive(a.min_up_bps, b.min_up_bps),
        min_down_bps: min_positive(a.min_down_bps, b.min_down_bps),
        peak_up_bps: a.peak_up_bps.max(b.peak_up_bps),
        peak_down_bps: a.peak_down_bps.max(b.peak_down_bps),
        connected_ms: a.connected_ms.saturating_add(b.connected_ms),
        samples: a.samples.saturating_add(b.samples),
        network: b.network,
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else if bytes < 1_000_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    }
}

pub fn format_rate(bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_second))
}

/// The rate as the two parts a tile shows: the number and the unit. The big
/// value of a tile carries the unit in the small type next to it.
pub fn format_rate_parts(bytes_per_second: u64) -> (String, String) {
    // format_bytes always ends in a number, a space and a unit; the split is at
    // that last space, so a word without one degrades to the whole text.
    let text = format_bytes(bytes_per_second);
    let at = text.rfind(' ').unwrap_or(text.len());
    (text[..at].to_string(), format!("{}/s", text[at..].trim()))
}
