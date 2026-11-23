//! The phone-to-laptop measurement frames.
//!
//! One ASCII datagram asks for a summary; the phone answers with one fixed
//! line. Every field is a number or a small enum, so no free text, key, name
//! or address can ride on the channel. Unknown fields are ignored and a
//! missing field means "unknown". Nothing here touches the disk or a socket.

use serde::{Deserialize, Serialize};

/// Frame magic.
pub const MAGIC: &str = "JFS1";
/// Frame version.
pub const VERSION: u32 = 1;
/// Largest accepted request.
pub const REQUEST_MAX: usize = 64;
/// Largest accepted summary.
pub const SUMMARY_MAX: usize = 512;
/// Largest accepted history frame (parsed but not served in 0.2).
pub const HISTORY_MAX: usize = 2048;
/// UDP port of the gateway stats responder.
pub const PORT: u16 = 9798;
/// Answer to a history request in this task.
pub const UNSUPPORTED_LINE: &str = "JFS1 1 error unsupported";
/// Shown when the phone never answered, usually because it is older.
pub const UNAVAILABLE_LINE: &str = "Phone measurements unavailable (older phone?).";
/// Shown when the tunnel is not connected yet.
pub const CONNECT_FIRST_LINE: &str = "Connect first.";
/// Heading of the read-only Linux block.
pub const HEADING: &str = "Phone measurements";

/// The kind of network the phone is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkKind {
    None,
    Wifi,
    Ethernet,
    Mobile,
    Mobile2g,
    Mobile3g,
    Mobile4g,
    Mobile4gCa,
    Mobile4gPlus,
    Mobile5g,
    Mobile5gNsa,
    Mobile5gMmwave,
    Mobile5gAdv,
    Other,
    #[default]
    Unknown,
}

impl NetworkKind {
    /// True for every mobile kind: the generic word and every generation.
    pub fn is_mobile(self) -> bool {
        self.generation().is_some() || self == NetworkKind::Mobile
    }

    /// The mobile generation as a number and a short word ("5G"), for every
    /// variant of that generation; None for the generic word and non-mobile.
    pub fn generation(self) -> Option<(u8, &'static str)> {
        match self {
            NetworkKind::Mobile2g => Some((2, "2G")),
            NetworkKind::Mobile3g => Some((3, "3G")),
            NetworkKind::Mobile4g | NetworkKind::Mobile4gCa | NetworkKind::Mobile4gPlus => {
                Some((4, "4G"))
            }
            NetworkKind::Mobile5g
            | NetworkKind::Mobile5gNsa
            | NetworkKind::Mobile5gMmwave
            | NetworkKind::Mobile5gAdv => Some((5, "5G")),
            _ => None,
        }
    }

    /// Parse the short wire word; an unknown word is `Unknown`, never text.
    pub fn parse(word: &str) -> Self {
        match word {
            "none" => NetworkKind::None,
            "wifi" => NetworkKind::Wifi,
            "eth" => NetworkKind::Ethernet,
            "mobile" | "cellular" => NetworkKind::Mobile,
            "2g" => NetworkKind::Mobile2g,
            "3g" => NetworkKind::Mobile3g,
            "4g" => NetworkKind::Mobile4g,
            "4g-ca" => NetworkKind::Mobile4gCa,
            "4g+" => NetworkKind::Mobile4gPlus,
            "5g" => NetworkKind::Mobile5g,
            "5g-nsa" => NetworkKind::Mobile5gNsa,
            "5g-mmwave" => NetworkKind::Mobile5gMmwave,
            "5g-adv" => NetworkKind::Mobile5gAdv,
            "other" => NetworkKind::Other,
            _ => NetworkKind::Unknown,
        }
    }

    /// The stable wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            NetworkKind::None => "none",
            NetworkKind::Wifi => "wifi",
            NetworkKind::Ethernet => "eth",
            NetworkKind::Mobile => "mobile",
            NetworkKind::Mobile2g => "2g",
            NetworkKind::Mobile3g => "3g",
            NetworkKind::Mobile4g => "4g",
            NetworkKind::Mobile4gCa => "4g-ca",
            NetworkKind::Mobile4gPlus => "4g+",
            NetworkKind::Mobile5g => "5g",
            NetworkKind::Mobile5gNsa => "5g-nsa",
            NetworkKind::Mobile5gMmwave => "5g-mmwave",
            NetworkKind::Mobile5gAdv => "5g-adv",
            NetworkKind::Other => "other",
            NetworkKind::Unknown => "unknown",
        }
    }

    /// The user-visible name.
    pub fn display(self) -> &'static str {
        match self {
            NetworkKind::None => "No network",
            NetworkKind::Wifi => "Wi-Fi",
            NetworkKind::Ethernet => "Ethernet",
            NetworkKind::Mobile => "Mobile network",
            NetworkKind::Mobile2g => "2G",
            NetworkKind::Mobile3g => "3G",
            NetworkKind::Mobile4g => "4G",
            NetworkKind::Mobile4gCa => "4G (aggregated)",
            NetworkKind::Mobile4gPlus => "4G+",
            NetworkKind::Mobile5g => "5G",
            NetworkKind::Mobile5gNsa => "5G (non-standalone)",
            NetworkKind::Mobile5gMmwave => "5G (mmWave)",
            NetworkKind::Mobile5gAdv => "5G (advanced)",
            NetworkKind::Other => "Other",
            NetworkKind::Unknown => "Unknown",
        }
    }
}

/// One phone summary. An absent value is `None` (or zero for a counter).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneStats {
    pub net: NetworkKind,
    pub signal: Option<u8>,
    pub rtt_ms: Option<u32>,
    pub up_bps: u64,
    pub down_bps: u64,
    pub battery: Option<u8>,
    /// Tenths of a degree Celsius; `i16::MIN` means unknown.
    pub temp_c_x10: Option<i16>,
    pub used_bytes: u64,
    pub allow_bytes: u64,
    pub reset_day: u8,
    /// Year times 12 plus month; 0 means unset.
    pub month: u32,
    /// Network switches counted this hour; `None` on older phones.
    pub switches: Option<u32>,
    /// The signal strength in dBm, when the phone knows it.
    pub dbm: Option<i16>,
    /// Android's thermal status, 0 to 6. `None` when the phone does not
    /// report it.
    pub thr: Option<u8>,
    /// Android's thermal headroom forecast for the next 30 seconds, in
    /// percent (100 = severe throttling). `None` on phones below API 30 or
    /// before Android has a reading.
    #[serde(default)]
    pub hr30: Option<u8>,
    /// The share of the laptop's packets the phone's WireGuard missed, in
    /// percent (boringtun's estimate). `None` on older phones or while the
    /// tunnel carried too little to tell.
    #[serde(default)]
    pub wg_loss_pct: Option<u8>,
    /// The round trip of the last WireGuard handshake, in milliseconds.
    #[serde(default)]
    pub wg_rtt_ms: Option<u32>,
    /// The phone takes the laptop's link report with the summary request
    /// (`caps=link`). Older phones reject extra words in a request, so the
    /// laptop sends them only to a phone that said so.
    #[serde(default)]
    pub caps_link: bool,
    /// The phone may read the network generation and the signal
    /// (`caps=net`). The phone sets this only while its network permission is
    /// granted; an older line without `caps` parses as false.
    #[serde(default)]
    pub caps_net: bool,
    /// The sharer's own connected time in milliseconds of the current tunnel
    /// session. `None` on older phones. The laptop counts from this value.
    #[serde(default)]
    pub sconn_ms: Option<u64>,
}

/// A parsed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneStatsRequest {
    Summary,
    History,
}

/// Why a frame was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    BadMagic,
    BadVersion,
    TooLarge,
    Malformed,
    Unsupported,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FrameError::BadMagic => "bad magic",
            FrameError::BadVersion => "bad version",
            FrameError::TooLarge => "frame too large",
            FrameError::Malformed => "malformed frame",
            FrameError::Unsupported => "unsupported request",
        })
    }
}

/// The summary request bytes.
pub fn request_summary() -> &'static [u8] {
    b"JFS1 1 summary"
}

/// The history request bytes (answered `unsupported` in 0.2).
pub fn request_history() -> &'static [u8] {
    b"JFS1 1 history"
}

/// The summary request with the laptop's link report, for a phone that said
/// `caps=link`.
pub fn request_summary_with_range(report: &crate::wifi_range::RangeReport) -> String {
    format!("{MAGIC} 1 summary{}", report.wire_words())
}

/// The laptop's link report a summary request carries, if any.
pub fn request_range(bytes: &[u8]) -> Option<crate::wifi_range::RangeReport> {
    if parse_request(bytes).ok()? != PhoneStatsRequest::Summary {
        return None;
    }
    let body = text(bytes).ok()?;
    crate::wifi_range::RangeReport::from_wire_words(body.split_whitespace().skip(3))
}

/// Parse one request datagram.
pub fn parse_request(bytes: &[u8]) -> Result<PhoneStatsRequest, FrameError> {
    if bytes.len() > REQUEST_MAX {
        return Err(FrameError::TooLarge);
    }
    let body = text(bytes)?;
    let mut parts = body.split_whitespace();
    let Some(magic) = parts.next() else {
        return Err(FrameError::Malformed);
    };
    if magic != MAGIC {
        return Err(FrameError::BadMagic);
    }
    match parts.next() {
        Some("1") => {}
        Some(_) => return Err(FrameError::BadVersion),
        None => return Err(FrameError::Malformed),
    }
    let request = match parts.next() {
        Some("summary") => PhoneStatsRequest::Summary,
        Some("history") => PhoneStatsRequest::History,
        _ => return Err(FrameError::Malformed),
    };
    // A summary request may carry the laptop's link report as key=value
    // words; anything else after the request word is malformed.
    for word in parts {
        let well_formed = word
            .split_once('=')
            .is_some_and(|(key, value)| !key.is_empty() && !value.is_empty());
        if request != PhoneStatsRequest::Summary || !well_formed {
            return Err(FrameError::Malformed);
        }
    }
    Ok(request)
}

impl PhoneStats {
    /// Parse one summary response. Unknown keys are dropped, missing values
    /// stay unknown, and any malformed known value rejects the frame.
    pub fn parse_response(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() > SUMMARY_MAX {
            return Err(FrameError::TooLarge);
        }
        let body = text(bytes)?;
        let mut parts = body.split_whitespace();
        let Some(magic) = parts.next() else {
            return Err(FrameError::Malformed);
        };
        if magic != MAGIC {
            return Err(FrameError::BadMagic);
        }
        match parts.next() {
            Some("1") => {}
            Some(_) => return Err(FrameError::BadVersion),
            None => return Err(FrameError::Malformed),
        }
        match parts.next() {
            Some("summary") => {}
            Some("history") => return Err(FrameError::Unsupported),
            _ => return Err(FrameError::Malformed),
        }

        let mut stats = PhoneStats::default();
        let mut seen: Vec<&str> = Vec::new();
        for token in parts {
            let (key, value) = token.split_once('=').ok_or(FrameError::Malformed)?;
            if key.is_empty() {
                return Err(FrameError::Malformed);
            }
            if !KNOWN_KEYS.contains(&key) {
                continue;
            }
            if seen.contains(&key) {
                return Err(FrameError::Malformed);
            }
            seen.push(key);
            match key {
                "v" => {
                    if value != "1" {
                        return Err(FrameError::Malformed);
                    }
                }
                "net" => stats.net = NetworkKind::parse(value),
                "sig" => stats.signal = parse_signal(value)?,
                "rtt" => stats.rtt_ms = parse_rtt(value)?,
                "up" => stats.up_bps = parse_u64(value)?,
                "down" => stats.down_bps = parse_u64(value)?,
                "batt" => stats.battery = parse_battery(value)?,
                "temp" => stats.temp_c_x10 = parse_temp(value)?,
                "used" => stats.used_bytes = parse_u64(value)?,
                "allow" => stats.allow_bytes = parse_u64(value)?,
                "reset" => stats.reset_day = parse_reset(value)?,
                "month" => stats.month = parse_u32(value)?,
                "switch" => stats.switches = Some(parse_u32(value)?),
                "dbm" => stats.dbm = Some(parse_dbm(value)?),
                "thr" => stats.thr = Some(parse_thr(value)?),
                "hr30" => stats.hr30 = parse_hr30(value)?,
                "loss" => stats.wg_loss_pct = Some(parse_loss(value)?),
                "wgrtt" => stats.wg_rtt_ms = Some(parse_u32(value)?),
                "caps" => {
                    stats.caps_link = value.split(',').any(|cap| cap == "link");
                    stats.caps_net = value.split(',').any(|cap| cap == "net");
                }
                "sconn" => stats.sconn_ms = Some(parse_u64(value)?),
                // `KNOWN_KEYS` is checked above, so this only runs when a key
                // was added to that list without an arm: reject the frame
                // instead of panicking in the wire parser.
                _ => return Err(FrameError::Malformed),
            }
        }
        Ok(stats)
    }

    /// The fixed summary line. Always shorter than [`SUMMARY_MAX`].
    pub fn to_response(&self) -> String {
        let mut line = format!(
            "{MAGIC} {VERSION} summary v=1 net={} sig={} rtt={} up={} down={} batt={} temp={} used={} allow={} reset={} month={}",
            self.net.as_str(),
            self.signal.unwrap_or(255),
            self.rtt_ms.unwrap_or(0),
            self.up_bps,
            self.down_bps,
            self.battery.unwrap_or(255),
            self.temp_c_x10.unwrap_or(i16::MIN),
            self.used_bytes,
            self.allow_bytes,
            self.reset_day,
            self.month
        );
        if let Some(switches) = self.switches {
            line.push_str(&format!(" switch={switches}"));
        }
        if let Some(dbm) = self.dbm {
            line.push_str(&format!(" dbm={dbm}"));
        }
        if let Some(thr) = self.thr {
            line.push_str(&format!(" thr={thr}"));
        }
        if let Some(hr30) = self.hr30 {
            line.push_str(&format!(" hr30={hr30}"));
        }
        if let Some(loss) = self.wg_loss_pct {
            line.push_str(&format!(" loss={loss}"));
        }
        if let Some(rtt) = self.wg_rtt_ms {
            line.push_str(&format!(" wgrtt={rtt}"));
        }
        match (self.caps_net, self.caps_link) {
            (true, true) => line.push_str(" caps=net,link"),
            (true, false) => line.push_str(" caps=net"),
            (false, true) => line.push_str(" caps=link"),
            (false, false) => {}
        }
        if let Some(sconn_ms) = self.sconn_ms {
            line.push_str(&format!(" sconn={sconn_ms}"));
        }
        line
    }

    /// A value older than this many seconds is stale.
    pub fn is_stale(age_secs: u64) -> bool {
        age_secs > 15
    }
}

/// The phone section on the phone's main screen: the same core wording as
/// the laptop block, plus the hour's switch count when the report is on.
pub fn main_lines(stats: &PhoneStats, show_dns: bool, show_battery: bool) -> Vec<String> {
    let mut lines = display_lines(Some(stats), None, false, show_dns);
    // The phone has no memory: it shows the heat line while its own reading
    // is 42 C or more. The line replaces the battery temperature line. The
    // battery switch owns the temperature, so the heat line waits for it.
    if show_battery {
        if let Some(temp) = stats.temp_c_x10 {
            if crate::phone_heat::level(Some(temp)) == Some(crate::phone_heat::Heat::Hot) {
                crate::phone_heat::replace_temperature_line(&mut lines);
                lines.push(crate::phone_heat::line(temp));
            }
        }
    }
    if let Some(switches) = stats.switches {
        lines.push(crate::network_report::switches_line(switches));
    }
    lines
}

/// The read-only lines the Linux window shows. No statistics means no lines;
/// `unavailable` is only shown when there never was a value.
pub fn display_lines(
    stats: Option<&PhoneStats>,
    age_secs: Option<u64>,
    unavailable: bool,
    show_dns: bool,
) -> Vec<String> {
    let Some(stats) = stats else {
        return if unavailable {
            vec![UNAVAILABLE_LINE.to_string()]
        } else {
            Vec::new()
        };
    };
    let mut lines = vec![format!("Network: {}", stats.net.display())];
    if let Some(signal) = stats.signal {
        lines.push(format!("Signal: {signal} of 4"));
    }
    if show_dns {
        lines.push(match stats.rtt_ms {
            Some(rtt) => format!("DNS answer time: {rtt} ms"),
            None => "DNS answer time: not enough data".to_string(),
        });
    }
    if let Some(line) = crate::battery::level_line(stats.battery) {
        lines.push(line);
    }
    if let Some(line) = crate::battery::temperature_line(stats.temp_c_x10) {
        lines.push(line);
    }
    if stats.up_bps > 0 || stats.down_bps > 0 {
        lines.push(format!(
            "Speed: {} down, {} up",
            format_rate(stats.down_bps),
            format_rate(stats.up_bps)
        ));
    }
    if stats.used_bytes > 0 || stats.allow_bytes > 0 {
        lines.push(crate::budget::display_for(
            stats.used_bytes,
            stats.allow_bytes,
        ));
        if let Some(line) = crate::budget::warning_for(stats.used_bytes, stats.allow_bytes) {
            lines.push(line.to_string());
        }
    }
    if age_secs.map(PhoneStats::is_stale).unwrap_or(false) {
        lines.push("Phone measurements are stale.".to_string());
    }
    lines
}

const KNOWN_KEYS: &[&str] = &[
    "v", "net", "sig", "rtt", "up", "down", "batt", "temp", "used", "allow", "reset", "month",
    "switch", "dbm", "thr", "hr30", "loss", "wgrtt", "caps", "sconn",
];

/// Trim trailing line endings and reject anything that is not printable
/// ASCII with spaces. A mid-frame newline is therefore malformed.
fn text(bytes: &[u8]) -> Result<&str, FrameError> {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != b'\r' && *byte != b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let body = &bytes[..end];
    if !body.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        return Err(FrameError::Malformed);
    }
    std::str::from_utf8(body).map_err(|_| FrameError::Malformed)
}

fn parse_u64(value: &str) -> Result<u64, FrameError> {
    value.parse().map_err(|_| FrameError::Malformed)
}

fn parse_u32(value: &str) -> Result<u32, FrameError> {
    value.parse().map_err(|_| FrameError::Malformed)
}

fn parse_signal(value: &str) -> Result<Option<u8>, FrameError> {
    let signal: u8 = value.parse().map_err(|_| FrameError::Malformed)?;
    match signal {
        255 => Ok(None),
        0..=4 => Ok(Some(signal)),
        _ => Err(FrameError::Malformed),
    }
}

fn parse_rtt(value: &str) -> Result<Option<u32>, FrameError> {
    let rtt = parse_u32(value)?;
    Ok(if rtt == 0 { None } else { Some(rtt) })
}

fn parse_battery(value: &str) -> Result<Option<u8>, FrameError> {
    let battery: u8 = value.parse().map_err(|_| FrameError::Malformed)?;
    match battery {
        255 => Ok(None),
        0..=100 => Ok(Some(battery)),
        _ => Err(FrameError::Malformed),
    }
}

fn parse_temp(value: &str) -> Result<Option<i16>, FrameError> {
    let temp: i16 = value.parse().map_err(|_| FrameError::Malformed)?;
    Ok(if temp == i16::MIN { None } else { Some(temp) })
}

fn parse_reset(value: &str) -> Result<u8, FrameError> {
    let day: u8 = value.parse().map_err(|_| FrameError::Malformed)?;
    if day > 31 {
        return Err(FrameError::Malformed);
    }
    Ok(day)
}

/// A signal strength in dBm. Outside -200..=0 it is malformed.
fn parse_dbm(value: &str) -> Result<i16, FrameError> {
    let dbm: i16 = value.parse().map_err(|_| FrameError::Malformed)?;
    if !(-200..=0).contains(&dbm) {
        return Err(FrameError::Malformed);
    }
    Ok(dbm)
}

/// Android's thermal status: 0 (none) to 6 (shutdown).
fn parse_loss(value: &str) -> Result<u8, FrameError> {
    match value.parse::<u8>() {
        Ok(pct) if pct <= 100 => Ok(pct),
        _ => Err(FrameError::Malformed),
    }
}

fn parse_thr(value: &str) -> Result<u8, FrameError> {
    let thr: u8 = value.parse().map_err(|_| FrameError::Malformed)?;
    if thr > 6 {
        return Err(FrameError::Malformed);
    }
    Ok(thr)
}

/// The thermal headroom forecast in percent: 0 (cool) to 200.
fn parse_hr30(value: &str) -> Result<Option<u8>, FrameError> {
    match value.parse::<u8>() {
        Ok(pct) if pct <= 200 => Ok(Some(pct)),
        _ => Err(FrameError::Malformed),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A value every parser accepts, so a key without an arm is the only
    /// reason this can fail. A new key in `KNOWN_KEYS` without an arm used to
    /// hit `unreachable!` and panic in the wire parser.
    fn accepted_value(key: &str) -> &'static str {
        match key {
            "v" => "1",
            "net" => "wifi",
            // The dbm parser wants a negative or zero level.
            "dbm" => "0",
            "caps" => "link",
            _ => "0",
        }
    }

    #[test]
    fn every_known_key_has_a_match_arm_and_never_panics() {
        for key in KNOWN_KEYS {
            let line = format!("{MAGIC} {VERSION} summary {key}={}", accepted_value(key));
            let parsed = PhoneStats::parse_response(line.as_bytes());
            assert!(parsed.is_ok(), "key {key} was rejected: {parsed:?}");
        }
    }

    #[test]
    fn summary_round_trips_the_sharer_connected_ms() {
        let stats = PhoneStats {
            sconn_ms: Some(12_345),
            ..PhoneStats::default()
        };
        let line = stats.to_response();
        assert!(line.contains(" sconn=12345"), "line was {line}");
        let back = PhoneStats::parse_response(line.as_bytes()).expect("round trip");
        assert_eq!(back.sconn_ms, Some(12_345));
    }

    #[test]
    fn a_summary_without_sconn_parses_as_none() {
        let line = PhoneStats::default().to_response();
        assert!(!line.contains("sconn"));
        let back = PhoneStats::parse_response(line.as_bytes()).expect("parse");
        assert_eq!(back.sconn_ms, None);
    }

    #[test]
    fn a_non_numeric_sconn_is_malformed() {
        let line = format!("{MAGIC} {VERSION} summary sconn=soon");
        assert_eq!(
            PhoneStats::parse_response(line.as_bytes()),
            Err(FrameError::Malformed)
        );
    }
}
