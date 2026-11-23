//! Finding the best spot for the phone.
//!
//! The phone samples its own network once a second. The user marks the spot
//! the phone is in and the samples of the last few seconds say how that spot
//! behaves. Pure: the caller hands over the samples and the marks and gets
//! back the lines. No clock, no file, no socket.
//!
//! The score of a spot is the median of its window: the network kind seen
//! most often, the median dBm, the median signal level and the median DNS
//! answer time. The ranking prefers the 5G family, then the LTE family, then
//! the better dBm and then the better level. A tie keeps the earlier spot.

use crate::phone_stats::NetworkKind;

/// A session lasts two minutes.
pub const SESSION_SECS: u64 = 120;
/// The score uses the last 15 seconds.
pub const WINDOW_SECS: u64 = 15;
/// Fewer samples: hold the phone still a moment longer.
pub const MIN_SAMPLES: usize = 5;

/// One sample of the phone's network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// When the sample was taken, in milliseconds.
    pub at_ms: i64,
    /// The network the phone was on.
    pub net: NetworkKind,
    /// The signal level, of four; 255 is unknown.
    pub level: u8,
    /// The signal strength in dBm, when the phone knows it.
    pub dbm: Option<i16>,
    /// The DNS answer time in milliseconds, when the phone knows it.
    pub rtt_ms: Option<u32>,
    /// The Wi-Fi leg in milliseconds, when the laptop knows it.
    pub wifi_ms: Option<u64>,
}

/// The moment the user marked as a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// When the mark was made, in milliseconds.
    pub at_ms: i64,
    /// The number of the spot.
    pub number: usize,
}

/// What one marked spot scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    /// The number of the spot.
    pub number: usize,
    /// The network kind seen most often in the window.
    pub net: NetworkKind,
    /// The median dBm of the window, when the phone knew one.
    pub dbm: Option<i16>,
    /// The median signal level of the window.
    pub level: u8,
    /// The median DNS answer time of the window, when there was one.
    pub rtt_ms: Option<u32>,
    /// How many samples the window held.
    pub samples: usize,
}

/// The spot of one mark: the score of the last [`WINDOW_SECS`] seconds before
/// the mark. `None` when the window holds fewer than [`MIN_SAMPLES`] samples.
pub fn spot_at(samples: &[Sample], mark: Mark) -> Option<Spot> {
    let start = mark.at_ms - (WINDOW_SECS as i64) * 1000;
    let window: Vec<&Sample> = samples
        .iter()
        .filter(|sample| sample.at_ms >= start && sample.at_ms <= mark.at_ms)
        .collect();
    if window.len() < MIN_SAMPLES {
        return None;
    }

    let mut kinds: Vec<(NetworkKind, usize, i64)> = Vec::new();
    for sample in &window {
        match kinds.iter_mut().find(|(kind, _, _)| *kind == sample.net) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 = entry.2.max(sample.at_ms);
            }
            None => kinds.push((sample.net, 1, sample.at_ms)),
        }
    }
    let mut most: Option<(NetworkKind, usize, i64)> = None;
    for kind in kinds {
        let better = match most {
            None => true,
            Some(current) => (kind.1, kind.2) > (current.1, current.2),
        };
        if better {
            most = Some(kind);
        }
    }
    let (net, _, _) = most?;

    let mut dbms: Vec<i64> = window
        .iter()
        .filter_map(|sample| sample.dbm)
        .map(i64::from)
        .collect();
    dbms.sort_unstable();
    let dbm = if dbms.is_empty() {
        None
    } else {
        Some(median(&dbms) as i16)
    };

    let mut levels: Vec<i64> = window
        .iter()
        .map(|sample| i64::from(sample.level))
        .collect();
    levels.sort_unstable();
    let level = median(&levels) as u8;

    let mut rtts: Vec<i64> = window
        .iter()
        .filter_map(|sample| sample.rtt_ms)
        .map(i64::from)
        .collect();
    rtts.sort_unstable();
    let rtt_ms = if rtts.is_empty() {
        None
    } else {
        Some(median(&rtts) as u32)
    };

    Some(Spot {
        number: mark.number,
        net,
        dbm,
        level,
        rtt_ms,
        samples: window.len(),
    })
}

/// The best spot of the session: the 5G family first, then the LTE family,
/// then the better dBm when both spots have one, then the better level. A
/// tie keeps the earlier spot.
pub fn best(spots: &[Spot]) -> Option<&Spot> {
    let mut winner: Option<&Spot> = None;
    for spot in spots {
        winner = Some(match winner {
            None => spot,
            Some(current) => {
                if ranks_after(spot, current) {
                    spot
                } else {
                    current
                }
            }
        });
    }
    winner
}

/// True when the candidate beats the current spot.
fn ranks_after(candidate: &Spot, current: &Spot) -> bool {
    let (candidate_family, current_family) = (family(candidate.net), family(current.net));
    if candidate_family != current_family {
        return candidate_family > current_family;
    }
    if let (Some(candidate_dbm), Some(current_dbm)) = (candidate.dbm, current.dbm) {
        if candidate_dbm != current_dbm {
            return candidate_dbm > current_dbm;
        }
    }
    candidate.level > current.level
}

/// The network family of the ranking: the 5G family, the LTE family or the
/// rest.
fn family(net: NetworkKind) -> u8 {
    match net {
        NetworkKind::Mobile5g
        | NetworkKind::Mobile5gNsa
        | NetworkKind::Mobile5gMmwave
        | NetworkKind::Mobile5gAdv => 2,
        NetworkKind::Mobile4g | NetworkKind::Mobile4gCa | NetworkKind::Mobile4gPlus => 1,
        _ => 0,
    }
}

/// The line the phone and the laptop show while the session runs.
pub fn live_line(sample: Option<&Sample>) -> String {
    let Some(sample) = sample else {
        return hold_line().to_string();
    };
    let mut parts = vec![format!("Now: {}", sample.net.display())];
    if let Some(dbm) = sample.dbm {
        parts.push(format!("{dbm} dBm"));
    }
    if sample.level <= 4 {
        parts.push(format!("signal {}/4", sample.level));
    }
    if let Some(rtt) = sample.rtt_ms {
        parts.push(format!("DNS {rtt} ms"));
    }
    if let Some(wifi) = sample.wifi_ms {
        parts.push(format!("Wi-Fi {wifi} ms"));
    }
    parts.join(", ")
}

/// The line of one marked spot.
pub fn spot_line(spot: &Spot) -> String {
    let mut parts = vec![format!("Spot {}: {}", spot.number, spot.net.display())];
    if let Some(dbm) = spot.dbm {
        parts.push(format!("{dbm} dBm"));
    }
    if spot.level <= 4 {
        parts.push(format!("signal {}/4", spot.level));
    }
    parts.join(", ")
}

/// The line that says which spot is the best so far. It never says "best
/// spot" as a certainty: one spot is not a comparison.
pub fn verdict(spots: &[Spot]) -> String {
    if spots.len() < 2 {
        return "Mark another spot to compare".to_string();
    }
    match best(spots) {
        Some(spot) => format!("Best so far: Spot {}", spot.number),
        None => "Mark another spot to compare".to_string(),
    }
}

/// The line that asks the user to hold the phone still a moment longer.
pub fn hold_line() -> &'static str {
    "Hold the phone still a few seconds longer"
}

/// True when the session has run its two minutes.
pub fn session_over(started_at_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(started_at_ms) >= (SESSION_SECS as i64) * 1000
}

/// The samples of the wire text: one sample per line, `atMs,net,level,dbm,
/// rttMs,wifiMs`. An empty field means unknown. A line that does not parse
/// is skipped.
pub fn parse_samples(text: &str) -> Vec<Sample> {
    text.lines().filter_map(parse_sample).collect()
}

/// One sample of the wire text.
fn parse_sample(line: &str) -> Option<Sample> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut fields = line.split(',');
    let at_ms = fields.next()?.trim().parse::<i64>().ok()?;
    let net = NetworkKind::parse(fields.next()?.trim());
    let level = parse_level(fields.next()?.trim())?;
    let dbm = parse_dbm(fields.next()?.trim())?;
    let rtt_ms = parse_optional::<u32>(fields.next()?.trim())?;
    let wifi_ms = parse_optional::<u64>(fields.next()?.trim())?;
    if fields.next().is_some() {
        return None;
    }
    Some(Sample {
        at_ms,
        net,
        level,
        dbm,
        rtt_ms,
        wifi_ms,
    })
}

/// The level of a sample: 0-4, or 255 for unknown.
fn parse_level(value: &str) -> Option<u8> {
    if value.is_empty() {
        return Some(255);
    }
    let level: u8 = value.parse().ok()?;
    if level <= 4 || level == 255 {
        Some(level)
    } else {
        None
    }
}

/// The dBm of a sample: an empty field is unknown, a value outside
/// -200..=0 does not parse.
fn parse_dbm(value: &str) -> Option<Option<i16>> {
    if value.is_empty() {
        return Some(None);
    }
    let dbm: i16 = value.parse().ok()?;
    if !(-200..=0).contains(&dbm) {
        return None;
    }
    Some(Some(dbm))
}

/// An optional number of a sample: an empty field is unknown.
fn parse_optional<T: std::str::FromStr>(value: &str) -> Option<Option<T>> {
    if value.is_empty() {
        return Some(None);
    }
    value.parse::<T>().ok().map(Some)
}

/// The marks of the wire text: one mark per line, `atMs,number`. A line that
/// does not parse is skipped.
pub fn parse_marks(text: &str) -> Vec<Mark> {
    text.lines().filter_map(parse_mark).collect()
}

/// One mark of the wire text.
fn parse_mark(line: &str) -> Option<Mark> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut fields = line.split(',');
    let at_ms = fields.next()?.trim().parse::<i64>().ok()?;
    let number = fields.next()?.trim().parse::<usize>().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(Mark { at_ms, number })
}

/// The median of a sorted list; an even count averages the two middle ones.
fn median(sorted: &[i64]) -> i64 {
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        let sum = sorted[middle - 1] + sorted[middle];
        if sum >= 0 {
            (sum + 1) / 2
        } else {
            (sum - 1) / 2
        }
    }
}
