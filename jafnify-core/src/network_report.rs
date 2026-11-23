//! The per-generation network report the phone keeps and the laptop reads.
//!
//! A generation is what the modem says it is on (4G, 4G+, 5G and the 5G
//! variants). A switch is a change between two known generations; the same
//! generation again is not a switch. The numbers are the ones Jafnify already
//! measures, so the report never claims more than it knows, and the wording
//! never says that switching caused a battery drain.

use std::fmt;

/// How many hourly buckets are kept: 720 hours, like the usage buckets.
pub const MAX_BUCKETS: usize = 720;

/// The mobile generations the modem can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    Lte,
    LteCa,
    LteAdvancedPro,
    Nr,
    NrNsa,
    NrNsaMmwave,
    NrAdvanced,
}

impl Generation {
    /// The name the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Generation::Lte => "4G",
            Generation::LteCa => "4G (aggregated)",
            Generation::LteAdvancedPro => "4G+",
            Generation::Nr => "5G",
            Generation::NrNsa => "5G (non-standalone)",
            Generation::NrNsaMmwave => "5G (mmWave)",
            Generation::NrAdvanced => "5G (advanced)",
        }
    }

    /// The stable wire word.
    pub fn wire(self) -> &'static str {
        match self {
            Generation::Lte => "4g",
            Generation::LteCa => "4g-ca",
            Generation::LteAdvancedPro => "4g+",
            Generation::Nr => "5g",
            Generation::NrNsa => "5g-nsa",
            Generation::NrNsaMmwave => "5g-mmwave",
            Generation::NrAdvanced => "5g-adv",
        }
    }

    /// The generation of a wire word; an unknown word is none.
    pub fn from_wire(word: &str) -> Option<Generation> {
        match word {
            "4g" => Some(Generation::Lte),
            "4g-ca" => Some(Generation::LteCa),
            "4g+" => Some(Generation::LteAdvancedPro),
            "5g" => Some(Generation::Nr),
            "5g-nsa" => Some(Generation::NrNsa),
            "5g-mmwave" => Some(Generation::NrNsaMmwave),
            "5g-adv" => Some(Generation::NrAdvanced),
            _ => None,
        }
    }

    /// The generation of the kind the phone reports; an unknown kind is none.
    pub fn from_kind(kind: &str) -> Option<Generation> {
        match kind {
            "lte" => Some(Generation::Lte),
            "lte_ca" => Some(Generation::LteCa),
            "lte_advanced_pro" => Some(Generation::LteAdvancedPro),
            "nr" => Some(Generation::Nr),
            "nr_nsa" => Some(Generation::NrNsa),
            "nr_nsa_mmwave" => Some(Generation::NrNsaMmwave),
            "nr_advanced" => Some(Generation::NrAdvanced),
            _ => None,
        }
    }
}

/// How stable the network was in one hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Steady,
    Changing,
    SwitchingOften,
}

impl Stability {
    /// The fixed thresholds: 0-1 switches, 2-3, 4 or more.
    pub fn of(switches: u32) -> Stability {
        match switches {
            0..=1 => Stability::Steady,
            2..=3 => Stability::Changing,
            _ => Stability::SwitchingOften,
        }
    }

    /// The word the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Stability::Steady => "steady",
            Stability::Changing => "changing",
            Stability::SwitchingOften => "switching often",
        }
    }
}

/// Counts the switches between generations.
#[derive(Debug, Clone, Default)]
pub struct GenerationTracker {
    current: Option<Generation>,
}

impl GenerationTracker {
    pub fn new() -> Self {
        GenerationTracker { current: None }
    }

    /// A change between two known generations is a switch. The first sighting,
    /// a repeat and a move to or from another kind of network are not.
    pub fn observe(&mut self, generation: Option<Generation>) -> Option<Generation> {
        let previous = self.current;
        self.current = generation;
        match (previous, generation) {
            (Some(before), Some(after)) if before != after => Some(after),
            _ => None,
        }
    }
}

/// One hour of the network report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkBucket {
    pub hour_start_ms: i64,
    /// How many times the generation changed inside the hour.
    pub switches: u32,
    /// The time spent on each generation, in the order it was seen.
    pub generation_ms: Vec<(Generation, u64)>,
    pub up_bytes: u64,
    pub down_bytes: u64,
    /// The drops of the tunnel session in that hour.
    pub drops: u32,
    /// The battery drain in mAh, when the counter was sane.
    pub drain_mah: Option<u64>,
    /// The battery drain in percent, when the counter was not usable.
    pub drain_pct: Option<u32>,
    /// True when the phone was on the charger during that hour.
    pub charging: bool,
    pub connected_ms: u64,
    pub samples: u32,
}

impl NetworkBucket {
    /// The line that goes into the private phone file.
    pub fn line(&self) -> String {
        let mut line = format!(
            "JFN1 1 hour={} switches={} gen={} up={} down={} drops={}",
            self.hour_start_ms,
            self.switches,
            self.generation_text(),
            self.up_bytes,
            self.down_bytes,
            self.drops,
        );
        if let Some(mah) = self.drain_mah {
            line.push_str(&format!(" batt_mah={mah}"));
        }
        if let Some(pct) = self.drain_pct {
            line.push_str(&format!(" batt_pct={pct}"));
        }
        line.push_str(&format!(
            " charging={} connected_ms={} samples={}",
            u8::from(self.charging),
            self.connected_ms,
            self.samples,
        ));
        line
    }

    fn generation_text(&self) -> String {
        if self.generation_ms.is_empty() {
            return "none".to_string();
        }
        self.generation_ms
            .iter()
            .map(|(generation, ms)| format!("{}:{}", generation.wire(), ms))
            .collect::<Vec<String>>()
            .join(",")
    }

    /// Read a line back; unknown keys are ignored, a broken line is refused.
    pub fn parse_line(line: &str) -> Result<NetworkBucket, NetworkParseError> {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("JFN1") {
            return Err(NetworkParseError::BadPrefix);
        }
        match tokens.next() {
            Some("1") => {}
            _ => return Err(NetworkParseError::BadVersion),
        }

        let mut bucket = NetworkBucket::default();
        let mut hour = None;
        let mut switches = None;
        let mut gen = None;
        let mut up = None;
        let mut down = None;
        let mut drops = None;
        let mut batt_mah = None;
        let mut batt_pct = None;
        let mut charging = None;
        let mut connected_ms = None;
        let mut samples = None;

        for token in tokens {
            let Some((key, value)) = token.split_once('=') else {
                return Err(NetworkParseError::Malformed);
            };
            if key.is_empty() || value.is_empty() {
                return Err(NetworkParseError::Malformed);
            }
            match key {
                "hour" => set(
                    &mut hour,
                    value
                        .parse::<i64>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "switches" => set(
                    &mut switches,
                    value
                        .parse::<u32>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "gen" => set(&mut gen, parse_generations(value)?)?,
                "up" => set(
                    &mut up,
                    value
                        .parse::<u64>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "down" => set(
                    &mut down,
                    value
                        .parse::<u64>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "drops" => set(
                    &mut drops,
                    value
                        .parse::<u32>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "batt_mah" => set(
                    &mut batt_mah,
                    value
                        .parse::<u64>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "batt_pct" => set(
                    &mut batt_pct,
                    value
                        .parse::<u32>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "charging" => set(
                    &mut charging,
                    match value {
                        "0" => false,
                        "1" => true,
                        _ => return Err(NetworkParseError::Malformed),
                    },
                )?,
                "connected_ms" => set(
                    &mut connected_ms,
                    value
                        .parse::<u64>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                "samples" => set(
                    &mut samples,
                    value
                        .parse::<u32>()
                        .map_err(|_| NetworkParseError::Malformed)?,
                )?,
                _ => {}
            }
        }

        bucket.hour_start_ms = hour.ok_or(NetworkParseError::Missing)?;
        bucket.switches = switches.ok_or(NetworkParseError::Missing)?;
        bucket.generation_ms = gen.unwrap_or_default();
        bucket.up_bytes = up.ok_or(NetworkParseError::Missing)?;
        bucket.down_bytes = down.ok_or(NetworkParseError::Missing)?;
        bucket.drops = drops.ok_or(NetworkParseError::Missing)?;
        bucket.drain_mah = batt_mah;
        bucket.drain_pct = batt_pct;
        bucket.charging = charging.ok_or(NetworkParseError::Missing)?;
        bucket.connected_ms = connected_ms.ok_or(NetworkParseError::Missing)?;
        bucket.samples = samples.ok_or(NetworkParseError::Missing)?;
        Ok(bucket)
    }
}

fn parse_generations(value: &str) -> Result<Vec<(Generation, u64)>, NetworkParseError> {
    if value == "none" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in value.split(',') {
        let Some((word, ms)) = part.split_once(':') else {
            return Err(NetworkParseError::Malformed);
        };
        let Some(generation) = Generation::from_wire(word) else {
            return Err(NetworkParseError::Malformed);
        };
        let ms = ms
            .parse::<u64>()
            .map_err(|_| NetworkParseError::Malformed)?;
        out.push((generation, ms));
    }
    Ok(out)
}

/// Add one bucket: the same hour merges, the oldest hour is dropped.
pub fn push_bucket(buckets: &mut Vec<NetworkBucket>, bucket: NetworkBucket) {
    if let Some(last) = buckets.last_mut() {
        if last.hour_start_ms == bucket.hour_start_ms {
            last.switches += bucket.switches;
            last.up_bytes += bucket.up_bytes;
            last.down_bytes += bucket.down_bytes;
            last.drops += bucket.drops;
            last.connected_ms += bucket.connected_ms;
            last.samples += bucket.samples;
            for (generation, ms) in bucket.generation_ms {
                match last
                    .generation_ms
                    .iter_mut()
                    .find(|(known, _)| *known == generation)
                {
                    Some((_, known_ms)) => *known_ms += ms,
                    None => last.generation_ms.push((generation, ms)),
                }
            }
            // Two parts of the same hour (for example around an app restart)
            // each measured their own drain: the hour's drain is the sum.
            last.drain_mah = add_optional(last.drain_mah, bucket.drain_mah);
            last.drain_pct = add_optional(last.drain_pct, bucket.drain_pct);
            last.charging = last.charging || bucket.charging;
            return;
        }
    }
    buckets.push(bucket);
    let overflow = buckets.len().saturating_sub(MAX_BUCKETS);
    if overflow > 0 {
        buckets.drain(..overflow);
    }
}

fn add_optional<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, None) => a,
        (None, b) => b,
    }
}

/// The charge counter of a phone can be zero, negative or garbage; only a
/// positive value is a drain figure.
pub fn sane_mah(value: i64) -> Option<u64> {
    if value <= 0 {
        None
    } else {
        Some(value as u64)
    }
}

/// The last-24-hour totals. The wording never says "this hour"; the drain is
/// the sum of the hours that had a figure.
pub fn window_lines(buckets: &[NetworkBucket], now_ms: i64) -> Vec<String> {
    let cutoff = now_ms - 86_400_000;
    let buckets: Vec<&NetworkBucket> = buckets
        .iter()
        .filter(|bucket| bucket.hour_start_ms >= cutoff)
        .collect();
    if buckets.is_empty() {
        return vec!["Not enough data yet.".to_string()];
    }
    let mut switches = 0u32;
    let mut times: Vec<(Generation, u64)> = Vec::new();
    let mut mah = 0u64;
    let mut pct = 0u32;
    for bucket in buckets {
        switches = switches.saturating_add(bucket.switches);
        for (generation, ms) in &bucket.generation_ms {
            match times.iter_mut().find(|(known, _)| known == generation) {
                Some((_, total)) => *total = total.saturating_add(*ms),
                None => times.push((*generation, *ms)),
            }
        }
        mah = mah.saturating_add(bucket.drain_mah.unwrap_or(0));
        pct = pct.saturating_add(bucket.drain_pct.unwrap_or(0));
    }
    if switches == 0 && times.is_empty() {
        return vec!["Not enough data yet.".to_string()];
    }
    let mut lines = vec![format!("Network switches: {switches}")];
    if !times.is_empty() {
        times.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        let mut parts = Vec::new();
        for (index, (generation, ms)) in times.iter().enumerate() {
            let minutes = ms / 60_000;
            if index == 0 {
                parts.push(format!("Time on {}: {minutes} min.", generation.label()));
            } else {
                parts.push(format!("On {}: {minutes} min.", generation.label()));
            }
        }
        lines.push(parts.join(" "));
    }
    if mah > 0 {
        lines.push(format!("Battery drain: {mah} mAh."));
    } else if pct > 0 {
        lines.push(format!("Battery drain: {pct}%."));
    }
    lines
}

/// The switch count line, shared by the phone and the laptop.
pub fn switches_line(switches: u32) -> String {
    format!("Network switches this hour: {switches}")
}

/// The words of one hour.
#[derive(Debug, Clone)]
pub struct NetworkSummary {
    bucket: NetworkBucket,
}

impl NetworkSummary {
    pub fn build(bucket: &NetworkBucket) -> Self {
        NetworkSummary {
            bucket: bucket.clone(),
        }
    }

    /// The lines, in the order the user reads them.
    pub fn lines(&self) -> Vec<String> {
        let bucket = &self.bucket;
        if bucket.samples == 0 {
            return vec!["Not enough data yet.".to_string()];
        }
        let mut lines = Vec::new();

        let time_line = self.time_line();
        if let Some(line) = time_line {
            lines.push(line);
        }
        // Without a reported 4G/5G generation we know nothing about switching:
        // say so instead of "0 switches, steady".
        let known = !bucket.generation_ms.is_empty() || bucket.switches > 0;
        if known {
            lines.push(format!("Network switches this hour: {}", bucket.switches));
        } else {
            lines.push("Network type not reported this hour.".to_string());
        }
        if bucket.up_bytes + bucket.down_bytes > 0 {
            lines.push(format!(
                "Speed this hour: {} down, {} up (what Jafnify shared)",
                format_rate(self.average_down_bps()),
                format_rate(self.average_up_bps()),
            ));
        }
        if bucket.drops > 0 {
            lines.push(format!("Drops this hour: {}", bucket.drops));
        }
        if known {
            lines.push(format!(
                "Network stability this hour: {}.",
                Stability::of(bucket.switches).label()
            ));
        }

        let drain = self.drain_text();
        if bucket.charging {
            lines.push("Charging this hour - no drain figure.".to_string());
        } else if let Some(drain) = &drain {
            lines.push(format!("Battery drain this hour: {drain}."));
        }
        if !bucket.charging && known {
            if let Some(drain) = &drain {
                let correlation = match bucket.switches {
                    0 => format!(
                        "No network switching this hour; battery drain {drain}."
                    ),
                    1..=3 => format!(
                        "Switching and battery drain this hour: {} switches, {drain}.",
                        bucket.switches
                    ),
                    _ => format!(
                        "More switching and a higher battery drain this hour: {} switches, {drain}.",
                        bucket.switches
                    ),
                };
                lines.push(correlation);
            }
        }
        lines
    }

    fn time_line(&self) -> Option<String> {
        let mut times: Vec<(Generation, u64)> = self.bucket.generation_ms.clone();
        if times.is_empty() {
            return None;
        }
        times.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        let mut parts = Vec::new();
        for (index, (generation, ms)) in times.iter().enumerate() {
            let minutes = ms / 60_000;
            if index == 0 {
                parts.push(format!("Time on {}: {minutes} min.", generation.label()));
            } else {
                parts.push(format!("On {}: {minutes} min.", generation.label()));
            }
        }
        Some(parts.join(" "))
    }

    fn drain_text(&self) -> Option<String> {
        if let Some(mah) = self.bucket.drain_mah {
            return Some(format!("{mah} mAh"));
        }
        self.bucket.drain_pct.map(|pct| format!("{pct}%"))
    }

    fn average_up_bps(&self) -> u64 {
        average(self.bucket.up_bytes, self.bucket.connected_ms)
    }

    fn average_down_bps(&self) -> u64 {
        average(self.bucket.down_bytes, self.bucket.connected_ms)
    }
}

fn average(bytes: u64, connected_ms: u64) -> u64 {
    if connected_ms == 0 {
        return 0;
    }
    bytes.saturating_mul(1_000) / connected_ms
}

/// Why a network line was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkParseError {
    BadPrefix,
    BadVersion,
    Malformed,
    Missing,
    Duplicate,
}

impl fmt::Display for NetworkParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkParseError::BadPrefix => {
                formatter.write_str("network line does not start with JFN1")
            }
            NetworkParseError::BadVersion => {
                formatter.write_str("unsupported network line version")
            }
            NetworkParseError::Malformed => formatter.write_str("malformed network line"),
            NetworkParseError::Missing => formatter.write_str("network line misses a field"),
            NetworkParseError::Duplicate => formatter.write_str("network line repeats a field"),
        }
    }
}

impl std::error::Error for NetworkParseError {}

fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), NetworkParseError> {
    if slot.is_some() {
        return Err(NetworkParseError::Duplicate);
    }
    *slot = Some(value);
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
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

fn format_rate(bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_second))
}
