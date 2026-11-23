//! The drop history: what the phone saw when the link went down.
//!
//! A drop is written the moment it happens, with the phone state and the
//! Wi-Fi leg of that moment; the reconnect is a second line that says how
//! long the link was down. The core pairs the two lines, sums the last day
//! and gives every drop a likely cause. Pure arithmetic and wording: no
//! clock, no socket, no storage.
//!
//! The lines hold no endpoint, no address, no key and no peer id: the reason
//! is a fixed word, never the free text of the connection log.

use std::time::Duration;

use crate::battery;
use crate::bottleneck::{PC_AVG_MS, PC_LOSS_PCT};
use crate::phone_exit::ExitReason;
use crate::phone_stats::NetworkKind;
use crate::wifi_leg::WifiLeg;

/// Milliseconds in one day.
pub const DAY_MS: i64 = 86_400_000;
/// The prefix of every line in the drop file.
pub const PREFIX: &str = "JFD1";
/// The version of the line format.
pub const VERSION: &str = "1";
/// A signal of this many bars or fewer is weak.
pub const WEAK_SIGNAL: u8 = 1;
/// A phone at or above this temperature is hot.
pub const HOT_C_X10: i16 = 420;
/// How old a phone summary may be and still be the context of a drop.
pub const CONTEXT_MAX_AGE_SECS: u64 = 30;

/// The hint for a weak signal.
pub const HINT_WEAK_SIGNAL: &str = "likely weak signal";
/// The hint for an old mobile network.
pub const HINT_SLOW_NETWORK: &str = "likely slow mobile network";
/// The hint for a missing mobile network.
pub const HINT_NO_NETWORK: &str = "likely no mobile network";
/// The hint for a hot phone.
pub const HINT_HOT_PHONE: &str = "likely hot phone";
/// The hint for a slow Wi-Fi leg.
pub const HINT_SLOW_WIFI: &str = "likely slow Wi-Fi";
/// The hint for a hotspot of the phone that went away.
pub const HINT_HOTSPOT_GONE: &str = "likely the phone's hotspot went away";
/// The hint for a laptop that was at the edge of the hotspot's range just
/// before the drop: the distance, not the phone.
pub const HINT_OUT_OF_RANGE: &str = "likely out of Wi-Fi range of the phone";
/// The hint for a peer session reset: the phone app restarted. It is never a
/// Wi-Fi problem: the reset comes from the app, not from the link.
pub const HINT_RESET: &str = "likely the phone app restarted";
/// The hint when nothing in the context points at a cause.
pub const HINT_NO_CAUSE: &str = "no clear cause";
/// The context words when there was no phone summary.
pub const NO_PHONE_DATA: &str = "phone data: none";
/// The words for a drop that never came back.
pub const NOT_BACK: &str = "down, not back before restart";
/// The words for an event that has not come back yet.
pub const NOT_BACK_YET: &str = "not back yet";

/// How long the phone may be unreachable before the outage stops being a
/// drop. Two minutes: a shorter loss is a fault, a longer one is an absence.
pub const AWAY_AFTER: Duration = Duration::from_secs(120);
/// The words of an absence of the phone.
pub const AWAY_LABEL: &str = "Phone away";
/// The words of a stop on the phone.
pub const STOPPED_LABEL: &str = "Stopped on the phone";
/// The words of an app update: neutral, like an absence.
pub const UPDATED_LABEL: &str = crate::phone_exit::UPDATED_LABEL;
/// The words that say the event is not a drop.
pub const NOT_A_DROP: &str = "not a drop";

/// Whether an absence of this length is an absence and not a drop.
pub fn is_away(down_ms: u64) -> bool {
    down_ms >= AWAY_AFTER.as_millis() as u64
}

/// The words of an absence of the phone: the label and how long it was.
pub fn away_words(down_ms: u64) -> String {
    format!(
        "{AWAY_LABEL} {}",
        crate::tiles::format_uptime(down_ms / 1000)
    )
}

/// The fixed reason words.
pub const REASON_PEER_LEFT: &str = "peer-left";
pub const REASON_NO_DATA: &str = "no-data";
pub const REASON_NO_NETWORK: &str = "no-network";
pub const REASON_RESET: &str = "reset";
pub const REASON_UNKNOWN: &str = "unknown";

/// The fixed word for a free-text reason. The text itself never reaches the
/// drop file: it can hold an address or a peer id.
pub fn reason_word(reason: &str) -> &'static str {
    let text = reason.to_ascii_lowercase();
    if text.contains("peer disconnected") || text.contains("peer left") {
        REASON_PEER_LEFT
    } else if text.contains("reset") {
        // A session reset that also mentions the data stall is still a reset:
        // the stall is the symptom, not the cause.
        REASON_RESET
    } else if text.contains("no data") {
        REASON_NO_DATA
    } else if text.contains("no network") || text.contains("unreachable") {
        REASON_NO_NETWORK
    } else {
        REASON_UNKNOWN
    }
}

/// The phone state seen just before a drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhoneContext {
    pub net: Option<NetworkKind>,
    pub signal: Option<u8>,
    pub temp_c_x10: Option<i16>,
    pub battery: Option<u8>,
    pub rtt_ms: Option<u32>,
    /// What Android said about the heat, when the phone reported it.
    pub thr: Option<u8>,
}

/// One drop, with the time it came back when it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropRecord {
    /// When the link went down, in epoch milliseconds.
    pub at_ms: i64,
    /// The fixed reason word.
    pub reason: String,
    /// The phone state of that moment, if there was one.
    pub phone: Option<PhoneContext>,
    /// The Wi-Fi leg of that moment, if there was one.
    pub wifi: Option<WifiLeg>,
    /// How long the link was down, filled in at the reconnect.
    pub down_ms: Option<u64>,
    /// Why the phone app's previous process ended, when the phone said so.
    /// It only sharpens the hint of a drop that stays a drop.
    pub exit: Option<ExitReason>,
    /// The Wi-Fi link in the seconds before the drop, from both ends
    /// (`wifi_range::LOOKBACK_MS`). It tells a laptop that walked out of
    /// range from a hotspot that was turned off.
    pub link: crate::wifi_range::LinkEvidence,
}

impl DropRecord {
    /// The one-line format of the drop. The phone fields and the Wi-Fi leg
    /// are left out when there were none.
    pub fn drop_line(&self) -> String {
        let reason: Vec<&str> = self.reason.split_whitespace().collect();
        let reason = if reason.is_empty() {
            REASON_UNKNOWN.to_string()
        } else {
            reason.join("-")
        };
        let mut line = format!(
            "{PREFIX} {VERSION} drop at={} reason={}",
            self.at_ms, reason
        );
        if let Some(phone) = &self.phone {
            if let Some(net) = phone.net {
                line.push_str(&format!(" net={}", net.as_str()));
            }
            if let Some(signal) = phone.signal {
                line.push_str(&format!(" sig={signal}"));
            }
            if let Some(temp) = phone.temp_c_x10 {
                line.push_str(&format!(" temp={temp}"));
            }
            if let Some(battery) = phone.battery {
                line.push_str(&format!(" batt={battery}"));
            }
            if let Some(rtt) = phone.rtt_ms {
                line.push_str(&format!(" rtt={rtt}"));
            }
            if let Some(thr) = phone.thr {
                line.push_str(&format!(" thr={thr}"));
            }
        }
        if let Some(wifi) = &self.wifi {
            line.push_str(&format!(
                " wifi_ms={} wifi_loss={} wifi_n={}",
                wifi.median_ms, wifi.loss_pct, wifi.samples
            ));
        }
        line.push_str(&self.link.line_words());
        line
    }

    /// The one-line format of the reconnect.
    pub fn back_line(at_ms: i64, down_ms: u64) -> String {
        format!("{PREFIX} {VERSION} back at={at_ms} down={down_ms}")
    }

    /// The one-line format of an absence of the phone. The line reclassifies
    /// the drop of the outage it belongs to: one record, never both.
    pub fn away_line(at_ms: i64) -> String {
        format!("{PREFIX} {VERSION} away at={at_ms}")
    }

    /// The one-line format of a stop on the phone.
    pub fn stopped_line(at_ms: i64) -> String {
        format!("{PREFIX} {VERSION} stopped at={at_ms}")
    }

    /// The one-line format of an app update. Like an absence, it reclassifies
    /// the drop of its own outage and is not counted.
    pub fn updated_line(at_ms: i64) -> String {
        format!("{PREFIX} {VERSION} updated at={at_ms}")
    }

    /// The one-line annotation of the exit reason, written after the back line
    /// when the phone says why its previous process ended.
    pub fn exit_line(at_ms: i64, reason: ExitReason) -> String {
        format!("{PREFIX} {VERSION} exit at={at_ms} word={}", reason.word())
    }

    /// The likely cause of the drop, from the recorded context. Every hint
    /// says "likely"; nothing here is a certainty.
    pub fn hint(&self) -> &'static str {
        // The phone said why its app ended: that is the cause, not a guess
        // from the context of the moment.
        if let Some(hint) = self.exit.and_then(ExitReason::drop_hint) {
            return hint;
        }
        // The laptop was at the edge of the hotspot's range just before the
        // link went: the distance is the likely cause, whatever the laptop
        // saw next. A reset comes from the phone app and stays a reset.
        if self.reason != REASON_RESET && self.link.out_of_range() {
            return HINT_OUT_OF_RANGE;
        }
        // The laptop lost its way to the phone: the hotspot of the phone went
        // away. Nothing else in the context can explain that, so this hint
        // stands on its own.
        if self.reason == REASON_NO_NETWORK {
            return HINT_HOTSPOT_GONE;
        }
        // A peer session reset comes from the phone app, never from the Wi-Fi
        // leg: the app restarted its session.
        if self.reason == REASON_RESET {
            return HINT_RESET;
        }
        if let Some(phone) = &self.phone {
            if phone.signal.map(|s| s <= WEAK_SIGNAL).unwrap_or(false) {
                return HINT_WEAK_SIGNAL;
            }
            match phone.net {
                Some(NetworkKind::Mobile2g) | Some(NetworkKind::Mobile3g) => {
                    return HINT_SLOW_NETWORK;
                }
                Some(NetworkKind::None) => return HINT_NO_NETWORK,
                _ => {}
            }
            // The phone says it is throttling: that is the phone's own word.
            // A phone that does not report it is only hot when the
            // temperature says so.
            let hot = match phone.thr {
                Some(thr) => thr >= crate::phone_heat::THROTTLE_SEVERE,
                None => phone.temp_c_x10.map(|t| t >= HOT_C_X10).unwrap_or(false),
            };
            if hot {
                return HINT_HOT_PHONE;
            }
        }
        if let Some(wifi) = &self.wifi {
            if wifi.median_ms > PC_AVG_MS || wifi.loss_pct >= PC_LOSS_PCT {
                return HINT_SLOW_WIFI;
            }
        }
        HINT_NO_CAUSE
    }

    /// The context words of the drop: the network, the signal and the
    /// temperature of the phone, or "phone data: none".
    pub fn context_words(&self) -> String {
        let Some(phone) = &self.phone else {
            return NO_PHONE_DATA.to_string();
        };
        let mut words = Vec::new();
        if let Some(net) = phone.net {
            words.push(net.display().to_string());
        }
        if let Some(signal) = phone.signal {
            words.push(format!("signal {signal}/4"));
        }
        if let Some(temp) = phone.temp_c_x10 {
            words.push(battery::temp_text(temp));
        }
        if words.is_empty() {
            return NO_PHONE_DATA.to_string();
        }
        words.join(", ")
    }

    /// The one-line reading of the drop, with the local clock.
    pub fn detail(&self, offset_minutes: i32) -> String {
        let down = match self.down_ms {
            Some(ms) => format!("down {} s", ms / 1000),
            None => NOT_BACK.to_string(),
        };
        format!(
            "{}  {down}  {} - {}",
            clock_hhmm(self.at_ms, offset_minutes),
            self.context_words(),
            self.hint()
        )
    }
}

/// What an event of the link turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// A real fault of the link.
    Drop,
    /// The phone was away for longer than the edge.
    Away,
    /// The phone stopped sharing.
    Stopped,
    /// The phone app was updated: neutral, not counted.
    Updated,
}

/// One event of the link: a real drop, an absence of the phone or a stop on
/// the phone. One outage is one event: an event that turned out to be an
/// absence is not a drop any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A real fault of the link.
    Drop(DropRecord),
    /// The phone was away for longer than the edge.
    Away {
        /// When the link went down, in epoch milliseconds.
        at_ms: i64,
        /// How long the absence lasted, filled in at the reconnect.
        down_ms: Option<u64>,
    },
    /// The phone stopped sharing.
    Stopped {
        /// When the phone stopped, in epoch milliseconds.
        at_ms: i64,
        /// How long the link stayed down, filled in at the reconnect.
        down_ms: Option<u64>,
    },
    /// The phone app was updated over the running process.
    Updated {
        /// When the outage began, in epoch milliseconds.
        at_ms: i64,
        /// How long the link stayed down, filled in at the reconnect.
        down_ms: Option<u64>,
    },
}

impl Event {
    /// What the event turned out to be.
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Drop(_) => EventKind::Drop,
            Event::Away { .. } => EventKind::Away,
            Event::Stopped { .. } => EventKind::Stopped,
            Event::Updated { .. } => EventKind::Updated,
        }
    }

    /// When the event began, in epoch milliseconds.
    pub fn at_ms(&self) -> i64 {
        match self {
            Event::Drop(record) => record.at_ms,
            Event::Away { at_ms, .. }
            | Event::Stopped { at_ms, .. }
            | Event::Updated { at_ms, .. } => *at_ms,
        }
    }

    /// How long the event lasted, when it is over.
    pub fn down_ms(&self) -> Option<u64> {
        match self {
            Event::Drop(record) => record.down_ms,
            Event::Away { down_ms, .. }
            | Event::Stopped { down_ms, .. }
            | Event::Updated { down_ms, .. } => *down_ms,
        }
    }

    /// Whether the event is a real fault of the link.
    pub fn is_drop(&self) -> bool {
        self.kind() == EventKind::Drop
    }

    /// The one-line reading of the event, with the local clock.
    pub fn detail(&self, offset_minutes: i32) -> String {
        match self {
            Event::Drop(record) => record.detail(offset_minutes),
            Event::Away { at_ms, down_ms } => format!(
                "{}  {} {}  {}",
                clock_hhmm(*at_ms, offset_minutes),
                AWAY_LABEL,
                absence_text(*down_ms),
                NOT_A_DROP
            ),
            Event::Stopped { at_ms, .. } => format!(
                "{}  {}  {}",
                clock_hhmm(*at_ms, offset_minutes),
                STOPPED_LABEL,
                NOT_A_DROP
            ),
            Event::Updated { at_ms, .. } => format!(
                "{}  {}  {}",
                clock_hhmm(*at_ms, offset_minutes),
                UPDATED_LABEL,
                NOT_A_DROP
            ),
        }
    }
}

/// How long an absence lasted, as the event shows it.
fn absence_text(down_ms: Option<u64>) -> String {
    match down_ms {
        Some(ms) => crate::tiles::format_uptime(ms / 1000),
        None => NOT_BACK_YET.to_string(),
    }
}

/// One parsed line of the drop file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Drop(DropRecord),
    Back {
        at_ms: i64,
        down_ms: u64,
    },
    Away {
        at_ms: i64,
    },
    Stopped {
        at_ms: i64,
    },
    Updated {
        at_ms: i64,
    },
    /// Why the phone app ended, attached to the outage that began at `at_ms`.
    Exit {
        at_ms: i64,
        word: ExitReason,
    },
}

/// Why one drop line could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropParseError {
    BadPrefix,
    BadVersion,
    Malformed,
}

impl std::fmt::Display for DropParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DropParseError::BadPrefix => formatter.write_str("drop line does not start with JFD1"),
            DropParseError::BadVersion => formatter.write_str("unsupported drop line version"),
            DropParseError::Malformed => formatter.write_str("malformed drop line"),
        }
    }
}

/// Parse one line of the drop file.
pub fn parse_line(line: &str) -> Result<Entry, DropParseError> {
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some(PREFIX) {
        return Err(DropParseError::BadPrefix);
    }
    match tokens.next() {
        Some(VERSION) => {}
        Some(_) => return Err(DropParseError::BadVersion),
        None => return Err(DropParseError::BadVersion),
    }
    match tokens.next() {
        Some("drop") => parse_drop(&mut tokens),
        Some("back") => parse_back(&mut tokens),
        Some("away") => parse_away(&mut tokens),
        Some("stopped") => parse_stopped(&mut tokens),
        Some("updated") => parse_updated(&mut tokens),
        Some("exit") => parse_exit(&mut tokens),
        _ => Err(DropParseError::Malformed),
    }
}

fn parse_drop(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    let mut at_ms = None;
    let mut reason = None;
    let mut net = None;
    let mut signal = None;
    let mut temp = None;
    let mut battery = None;
    let mut rtt = None;
    let mut thr = None;
    let mut wifi_ms = None;
    let mut wifi_loss = None;
    let mut wifi_n = None;
    let mut link = crate::wifi_range::LinkEvidence::default();

    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(DropParseError::Malformed);
        };
        if key.is_empty() || value.is_empty() {
            return Err(DropParseError::Malformed);
        }
        match key {
            "at" => set(&mut at_ms, parse_i64(value)?)?,
            "reason" => set(&mut reason, value.to_string())?,
            "net" => set(&mut net, NetworkKind::parse(value))?,
            "sig" => set(&mut signal, parse_u8(value)?)?,
            "temp" => set(&mut temp, parse_i16(value)?)?,
            "batt" => set(&mut battery, parse_u8(value)?)?,
            "rtt" => set(&mut rtt, parse_u32(value)?)?,
            "thr" => set(&mut thr, parse_u8(value)?)?,
            "wifi_ms" => set(&mut wifi_ms, parse_u64(value)?)?,
            "wifi_loss" => set(&mut wifi_loss, parse_u64(value)?)?,
            "wifi_n" => set(&mut wifi_n, parse_usize(value)?)?,
            _ => {
                link.take_word(key, value)
                    .map_err(|_| DropParseError::Malformed)?;
            }
        }
    }

    let phone = if net.is_some()
        || signal.is_some()
        || temp.is_some()
        || battery.is_some()
        || rtt.is_some()
        || thr.is_some()
    {
        Some(PhoneContext {
            net,
            signal,
            temp_c_x10: temp,
            battery,
            rtt_ms: rtt,
            thr,
        })
    } else {
        None
    };
    let wifi = match (wifi_ms, wifi_loss, wifi_n) {
        (Some(median_ms), Some(loss_pct), Some(samples)) => {
            Some(WifiLeg::new(median_ms, loss_pct, samples))
        }
        _ => None,
    };
    Ok(Entry::Drop(DropRecord {
        at_ms: at_ms.ok_or(DropParseError::Malformed)?,
        reason: reason.ok_or(DropParseError::Malformed)?,
        phone,
        wifi,
        down_ms: None,
        exit: None,
        link,
    }))
}

fn parse_back(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    let mut at_ms = None;
    let mut down_ms = None;
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(DropParseError::Malformed);
        };
        if key.is_empty() || value.is_empty() {
            return Err(DropParseError::Malformed);
        }
        match key {
            "at" => set(&mut at_ms, parse_i64(value)?)?,
            "down" => set(&mut down_ms, parse_u64(value)?)?,
            _ => {}
        }
    }
    Ok(Entry::Back {
        at_ms: at_ms.ok_or(DropParseError::Malformed)?,
        down_ms: down_ms.ok_or(DropParseError::Malformed)?,
    })
}

fn parse_away(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    Ok(Entry::Away {
        at_ms: parse_at(tokens)?,
    })
}

fn parse_stopped(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    Ok(Entry::Stopped {
        at_ms: parse_at(tokens)?,
    })
}

fn parse_updated(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    Ok(Entry::Updated {
        at_ms: parse_at(tokens)?,
    })
}

/// The exit annotation. An unknown word is kept as `Other`: a newer phone's
/// word must never reject the line, and an annotation never hides a drop.
fn parse_exit(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<Entry, DropParseError> {
    let mut at_ms = None;
    let mut word = None;
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(DropParseError::Malformed);
        };
        if key.is_empty() || value.is_empty() {
            return Err(DropParseError::Malformed);
        }
        match key {
            "at" => set(&mut at_ms, parse_i64(value)?)?,
            "word" => set(
                &mut word,
                ExitReason::parse(value).unwrap_or(ExitReason::Other),
            )?,
            _ => {}
        }
    }
    Ok(Entry::Exit {
        at_ms: at_ms.ok_or(DropParseError::Malformed)?,
        word: word.ok_or(DropParseError::Malformed)?,
    })
}

/// The one time a neutral line carries. The other keys are ignored: a line of
/// a newer version still loads.
fn parse_at(tokens: &mut std::str::SplitWhitespace<'_>) -> Result<i64, DropParseError> {
    let mut at_ms = None;
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(DropParseError::Malformed);
        };
        if key.is_empty() || value.is_empty() {
            return Err(DropParseError::Malformed);
        }
        if key == "at" {
            set(&mut at_ms, parse_i64(value)?)?;
        }
    }
    at_ms.ok_or(DropParseError::Malformed)
}

/// Take one value; a second value for the same key is malformed.
fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), DropParseError> {
    if slot.is_some() {
        return Err(DropParseError::Malformed);
    }
    *slot = Some(value);
    Ok(())
}

fn parse_i64(value: &str) -> Result<i64, DropParseError> {
    value.parse::<i64>().map_err(|_| DropParseError::Malformed)
}

fn parse_i16(value: &str) -> Result<i16, DropParseError> {
    value.parse::<i16>().map_err(|_| DropParseError::Malformed)
}

fn parse_u8(value: &str) -> Result<u8, DropParseError> {
    value.parse::<u8>().map_err(|_| DropParseError::Malformed)
}

fn parse_u32(value: &str) -> Result<u32, DropParseError> {
    value.parse::<u32>().map_err(|_| DropParseError::Malformed)
}

fn parse_u64(value: &str) -> Result<u64, DropParseError> {
    value.parse::<u64>().map_err(|_| DropParseError::Malformed)
}

fn parse_usize(value: &str) -> Result<usize, DropParseError> {
    value
        .parse::<usize>()
        .map_err(|_| DropParseError::Malformed)
}

/// Pair the lines of the drop file into the drops they describe.
pub fn pair<S: AsRef<str>>(lines: &[S]) -> Vec<DropRecord> {
    let entries: Vec<Entry> = lines
        .iter()
        .filter_map(|line| parse_line(line.as_ref()).ok())
        .collect();
    pair_entries(entries)
}

/// Pair the lines of the drop file into the events they describe.
pub fn events<S: AsRef<str>>(lines: &[S]) -> Vec<Event> {
    let entries: Vec<Entry> = lines
        .iter()
        .filter_map(|line| parse_line(line.as_ref()).ok())
        .collect();
    pair_events(entries)
}

/// Pair the parsed lines into the drops they describe: a back line fills the
/// newest drop that has no down time yet. An outage that turned out to be an
/// absence or a stop on the phone is not a drop any more.
pub fn pair_entries(entries: Vec<Entry>) -> Vec<DropRecord> {
    pair_events(entries)
        .into_iter()
        .filter_map(|event| match event {
            Event::Drop(record) => Some(record),
            _ => None,
        })
        .collect()
}

/// Pair the parsed lines into the events they describe: one record per outage.
/// A back line closes the newest open event. An away or a stopped line
/// reclassifies the newest open drop, so an outage is never both a drop and
/// an absence.
pub fn pair_events(entries: Vec<Entry>) -> Vec<Event> {
    let mut events: Vec<Event> = Vec::new();
    for entry in entries {
        match entry {
            Entry::Drop(record) => events.push(Event::Drop(record)),
            Entry::Back { down_ms, .. } => {
                if let Some(open) = events
                    .iter_mut()
                    .rev()
                    .find(|event| event.down_ms().is_none())
                {
                    match open {
                        Event::Drop(record) => record.down_ms = Some(down_ms),
                        Event::Away { down_ms: down, .. } => *down = Some(down_ms),
                        Event::Stopped { down_ms: down, .. } => *down = Some(down_ms),
                        Event::Updated { down_ms: down, .. } => *down = Some(down_ms),
                    }
                }
            }
            Entry::Away { at_ms } => mark_away(&mut events, at_ms),
            Entry::Stopped { at_ms } => mark_stopped(&mut events, at_ms),
            Entry::Updated { at_ms } => mark_updated(&mut events, at_ms),
            Entry::Exit { at_ms, word } => mark_exit(&mut events, at_ms, word),
        }
    }
    events
}

/// Mark the open event of this outage as an absence of the phone. The line
/// carries the start of its own outage, so it finds its own drop and never
/// an older one: a fault of an earlier outage stays a fault.
fn mark_away(events: &mut Vec<Event>, at_ms: i64) {
    match open_event_index(events, at_ms) {
        Some(index) => {
            let start = events[index].at_ms();
            events[index] = Event::Away {
                at_ms: start,
                down_ms: None,
            };
        }
        None => events.push(Event::Away {
            at_ms,
            down_ms: None,
        }),
    }
}

/// Mark the open event of this outage as a stop on the phone. The exit reason
/// arrives at the reconnect, after the back line: the event of that exact
/// outage is reclassified even when it is already closed.
fn mark_stopped(events: &mut Vec<Event>, at_ms: i64) {
    match event_index_at(events, at_ms) {
        Some(index) => {
            let down_ms = events[index].down_ms();
            events[index] = Event::Stopped {
                at_ms: events[index].at_ms(),
                down_ms,
            };
        }
        None => events.push(Event::Stopped {
            at_ms,
            down_ms: None,
        }),
    }
}

/// Mark the outage of this exact start as an app update: neutral, one record.
fn mark_updated(events: &mut Vec<Event>, at_ms: i64) {
    match event_index_at(events, at_ms) {
        Some(index) => {
            let down_ms = events[index].down_ms();
            events[index] = Event::Updated {
                at_ms: events[index].at_ms(),
                down_ms,
            };
        }
        None => events.push(Event::Updated {
            at_ms,
            down_ms: None,
        }),
    }
}

/// Attach the exit reason to the drop of its own outage. It sharpens the hint;
/// it never turns a drop into something else - that is the neutral line's job.
fn mark_exit(events: &mut [Event], at_ms: i64, word: ExitReason) {
    if let Some(index) = event_index_at(events, at_ms) {
        if let Event::Drop(record) = &mut events[index] {
            record.exit = Some(word);
        }
    }
}

/// The index of the event that starts at this moment: the event of the outage
/// the neutral line belongs to, open or already closed. The line carries its
/// own outage start, so it never finds a different one.
fn event_index_at(events: &[Event], at_ms: i64) -> Option<usize> {
    events.iter().rposition(|event| event.at_ms() == at_ms)
}

/// The index of the open event that starts at this moment: the event of the
/// outage the neutral line belongs to.
fn open_event_index(events: &[Event], at_ms: i64) -> Option<usize> {
    events
        .iter()
        .rposition(|event| event.at_ms() == at_ms && event.down_ms().is_none())
}

/// The one-line summary of the last day: how many drops and how long the
/// longest one lasted.
pub fn summary(records: &[DropRecord], now_ms: i64) -> String {
    let start = now_ms - DAY_MS;
    let recent: Vec<&DropRecord> = records
        .iter()
        .filter(|record| record.at_ms >= start)
        .collect();
    let longest = recent
        .iter()
        .filter_map(|record| record.down_ms)
        .max()
        .map(|ms| ms / 1_000)
        .unwrap_or(0);
    format!(
        "Drops in the last 24 h: {} (longest {longest} s)",
        recent.len()
    )
}

/// The local clock of an epoch time, `HH:MM`.
fn clock_hhmm(at_ms: i64, offset_minutes: i32) -> String {
    let seconds = at_ms.div_euclid(1_000) + i64::from(offset_minutes) * 60;
    let day = seconds.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3_600, (day % 3_600) / 60)
}
