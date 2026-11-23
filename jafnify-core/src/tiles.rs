//! The tile home: the instrument tiles of the laptop's home screen.
//!
//! Pure: the caller hands over what the laptop already knows and gets back the
//! tiles to draw. The words live here, so every platform shows the same
//! sentences. The trend buffer is a ring of the last five minutes that lives
//! in memory only; it is never written to disk.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::drop_history::Event;
use crate::good_hours::{verdict_for, Summary, Verdict};
use crate::phone_heat;
use crate::phone_stats::PhoneStats;
use crate::scope::{in_scope, ScopeSession, ScopeTotals, TileScope};
use crate::usage::{self, UsageSummary};
use crate::wifi_leg::WifiLeg;

/// How long the trend keeps its samples: the last five minutes.
pub const TREND_WINDOW_MS: i64 = 5 * 60 * 1000;

/// How many samples the trend keeps at most. It is a bound, not the window:
/// the window is the time, the bound is the memory.
pub const TREND_SAMPLES: usize = 900;

/// How many points a drawn line may have.
pub const MAX_TREND_POINTS: usize = 60;

/// How many cells an hour strip has.
pub const STRIP_CELLS: usize = 24;

/// The id of the DNS answer tile.
pub const ID_DNS: &str = "dns";
/// The id of the signal tile.
pub const ID_SIGNAL: &str = "signal";
/// The phone's fact while the network permission is missing.
pub const SIGNAL_PERMISSION_PHONE: &str = "Turn on Network details in Settings";
/// The laptop's fact while the phone says it has no network permission.
pub const SIGNAL_PERMISSION_LAPTOP: &str = "Turn on Network details on the phone";
/// The id of the phone tile.
pub const ID_PHONE: &str = "phone";
/// The id of the today tile.
pub const ID_TODAY: &str = "today";
/// The id of the Wi-Fi leg tile.
pub const ID_WIFI: &str = "wifi";
/// The id of the drops tile.
pub const ID_DROPS: &str = "drops";
/// The id of the heat tile of the phone.
pub const ID_HEAT: &str = "heat";
/// The id of the battery tile of the phone.
pub const ID_BATTERY: &str = "battery";
/// The id of the speed tile of the phone.
pub const ID_SPEED: &str = "speed";
/// The id of the average tile: the average and peak rates and the data moved
/// over the scope, from the usage statistics.
pub const ID_AVERAGE: &str = "avg";

/// The tone of a number or a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    /// All is well.
    Ok,
    /// Something is worth waiting for.
    Wait,
    /// Something is wrong.
    Err,
    /// A plain number.
    Muted,
}

/// The small word at the top of a tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    /// The word, one word.
    pub text: String,
    /// The tone of the word.
    pub tone: Tone,
}

/// The number a tile leads with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigValue {
    /// The number as text.
    pub value: String,
    /// The unit of the number, when it has one.
    pub unit: Option<String>,
    /// The tone of the number.
    pub tone: Tone,
}

/// The shape of the drawing under the number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrendKind {
    /// One point per sample, drawn as a line.
    Line {
        /// The points of the line, oldest first.
        points: Vec<i64>,
    },
    /// The two-part bar of today: down and up.
    Split {
        /// The bytes that came down.
        down: u64,
        /// The bytes that went up.
        up: u64,
    },
    /// The 24-cell hour strip.
    Hours {
        /// The cells of the strip, hour 0 to 23.
        cells: Vec<Cell>,
    },
    /// The fill bar of the battery: how much of it is full.
    Fill {
        /// How full it is.
        value: u8,
        /// How full it can get.
        max: u8,
    },
}

/// One cell of an hour strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    /// The local hour of the cell.
    pub hour: u8,
    /// What the hour says.
    pub mark: CellMark,
    /// Whether the hour lies inside the window of the tile. A cell outside
    /// the window is drawn muted and is never counted, so the strip cannot
    /// show a red hour the number does not count. A cell from an older
    /// window of the daemon has no field: it is inside.
    #[serde(default = "inside_window")]
    pub in_scope: bool,
}

/// The default of the scope flag of a cell: a cell that does not say is
/// inside the window of its tile.
fn inside_window() -> bool {
    true
}

/// What an hour of a strip says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellMark {
    /// The hour is usually quick.
    Quick,
    /// The hour is usually busy.
    Busy,
    /// The hour says nothing.
    Unknown,
    /// The link went down in this hour.
    Drop,
    /// The phone was away or stopped in this hour. Not a fault of the link.
    Away,
}

/// The signal bars of the signal tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Levels {
    /// The level the phone reports, of `max`.
    pub level: u8,
    /// How many levels there are.
    pub max: u8,
}

/// The drawing under the number of a tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trend {
    /// The shape of the drawing.
    pub kind: TrendKind,
    /// A dashed line to compare against, for example the heat threshold.
    pub reference: Option<i64>,
    /// The signal bars, when the tile has them.
    pub levels: Option<Levels>,
}

/// The short lines under the drawing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// The lines, at most two.
    pub lines: Vec<String>,
}

impl Facts {
    /// The lines of a tile. A tile carries at most two of them.
    pub fn new(mut lines: Vec<String>) -> Self {
        lines.truncate(2);
        Facts { lines }
    }
}

/// One tile of the home screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    /// The id of the tile.
    pub id: String,
    /// The label of the tile, in caps.
    pub label: String,
    /// The small word at the top, when the tile has one.
    pub badge: Option<Badge>,
    /// The number the tile leads with.
    pub big: Option<BigValue>,
    /// The drawing under the number.
    pub trend: Option<Trend>,
    /// The short lines under the drawing.
    pub facts: Option<Facts>,
    /// One line under the facts, for the tile that has something to add: the
    /// phone tile shows what the hot hours taught us.
    pub note: Option<String>,
}

impl Tile {
    /// A tile without a slot filled is not shown at all.
    pub fn is_empty(&self) -> bool {
        self.big.is_none() && self.trend.is_none() && self.facts.is_none()
    }
}

/// The ring of the last five minutes of one measured series.
#[derive(Debug, Clone, Default)]
pub struct TrendBuffer {
    samples: VecDeque<(i64, i64)>,
}

impl TrendBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// One more sample, with the moment it was taken. The sample that falls
    /// out of the five-minute window leaves, and the ring never grows over
    /// its bound.
    pub fn push(&mut self, at_ms: i64, value: i64) {
        while self.samples.len() >= TREND_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back((at_ms, value));
        self.forget_the_old(at_ms);
    }

    /// The samples that are older than the window leave the ring.
    fn forget_the_old(&mut self, newest_ms: i64) {
        let limit = newest_ms.saturating_sub(TREND_WINDOW_MS);
        while let Some((at_ms, _)) = self.samples.front() {
            if *at_ms > limit {
                break;
            }
            self.samples.pop_front();
        }
    }

    /// The values of the ring, oldest first.
    fn values(&self) -> Vec<i64> {
        self.samples.iter().map(|(_, value)| *value).collect()
    }

    /// Forget the samples: the tunnel went down, the numbers are old.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// How many samples the buffer holds.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the buffer holds no sample.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The points to draw: at most [`MAX_TREND_POINTS`], one mean per bucket.
    pub fn points(&self) -> Vec<i64> {
        downsample(&self.values())
    }

    /// The newest sample when it was taken at or after `from_ms`.
    pub fn latest_since(&self, from_ms: i64) -> Option<i64> {
        self.samples
            .back()
            .filter(|(at, _)| *at >= from_ms)
            .map(|(_, value)| *value)
    }

    /// The smallest sample taken between `from_ms` and `to_ms`, both
    /// included, if any.
    pub fn min_between(&self, from_ms: i64, to_ms: i64) -> Option<i64> {
        self.samples
            .iter()
            .filter(|(at, _)| (from_ms..=to_ms).contains(at))
            .map(|(_, value)| *value)
            .min()
    }

    /// The largest sample taken at or after `from_ms`, if any.
    pub fn max_since(&self, from_ms: i64) -> Option<i64> {
        self.samples
            .iter()
            .filter(|(at, _)| *at >= from_ms)
            .map(|(_, value)| *value)
            .max()
    }

    /// The smallest, the middle and the largest sample.
    pub fn stats(&self) -> Option<TrendStats> {
        let mut sorted = self.values();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        let median = if sorted.len() % 2 == 1 {
            sorted[middle]
        } else {
            // The same rule as the good hours: the average of the two middle
            // samples, rounded down.
            sorted[middle - 1].saturating_add(sorted[middle].saturating_sub(sorted[middle - 1]) / 2)
        };
        Some(TrendStats {
            min: sorted[0],
            median,
            max: sorted[sorted.len() - 1],
        })
    }
}

/// The smallest, the middle and the largest sample of a trend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrendStats {
    /// The smallest sample.
    pub min: i64,
    /// The middle sample.
    pub median: i64,
    /// The largest sample.
    pub max: i64,
}

/// The points of a trend, at most [`MAX_TREND_POINTS`] of them.
fn downsample(values: &[i64]) -> Vec<i64> {
    if values.len() <= MAX_TREND_POINTS {
        return values.to_vec();
    }
    let bucket = values.len() as f64 / MAX_TREND_POINTS as f64;
    let mut points = Vec::with_capacity(MAX_TREND_POINTS);
    for index in 0..MAX_TREND_POINTS {
        let start = (index as f64 * bucket).floor() as usize;
        let end = (((index + 1) as f64 * bucket).floor() as usize)
            .max(start + 1)
            .min(values.len());
        let mut sum = 0i128;
        let mut count = 0i64;
        for value in values.iter().skip(start).take(end - start) {
            sum += i128::from(*value);
            count += 1;
        }
        if count == 0 {
            continue;
        }
        points.push((sum / i128::from(count)) as i64);
    }
    points
}

/// The five trend rings of the home screen: the DNS answer time, the signal,
/// the temperature of the phone, the leg to the phone and the rate of the
/// phone. They live in memory only and are cleared when the tunnel goes down.
#[derive(Debug, Clone, Default)]
pub struct Trends {
    dns: TrendBuffer,
    signal: TrendBuffer,
    phone: TrendBuffer,
    wifi: TrendBuffer,
    /// The down rates of the phone: how fast data comes down.
    speed: TrendBuffer,
}

impl Trends {
    /// Empty rings.
    pub fn new() -> Self {
        Self::default()
    }

    /// One more sample of every measurement the phone sent, with the moment it
    /// was taken. A measurement the phone did not report is not a sample.
    pub fn push(&mut self, at_ms: i64, stats: &PhoneStats, wifi_ms: Option<u64>) {
        if let Some(rtt) = stats.rtt_ms {
            self.dns.push(at_ms, i64::from(rtt));
        }
        if let Some(dbm) = stats.dbm {
            self.signal.push(at_ms, i64::from(dbm));
        }
        if let Some(temp) = stats.temp_c_x10 {
            self.phone.push(at_ms, i64::from(temp));
        }
        if let Some(ms) = wifi_ms {
            self.wifi.push(at_ms, i64::try_from(ms).unwrap_or(i64::MAX));
        }
        // A rate of zero is a measurement: nothing came down. It is not a
        // missing value like an absent signal strength.
        self.speed
            .push(at_ms, i64::try_from(stats.down_bps).unwrap_or(i64::MAX));
    }

    /// Forget the samples: the tunnel went down and the numbers are old.
    pub fn clear(&mut self) {
        self.dns.clear();
        self.signal.clear();
        self.phone.clear();
        self.wifi.clear();
        self.speed.clear();
    }

    /// The ring of the DNS answer times.
    pub fn dns(&self) -> &TrendBuffer {
        &self.dns
    }

    /// The ring of the signal strengths.
    pub fn signal(&self) -> &TrendBuffer {
        &self.signal
    }

    /// The ring of the temperatures of the phone.
    pub fn phone(&self) -> &TrendBuffer {
        &self.phone
    }

    /// The ring of the legs to the phone.
    pub fn wifi(&self) -> &TrendBuffer {
        &self.wifi
    }

    /// The ring of the down rates of the phone.
    pub fn speed(&self) -> &TrendBuffer {
        &self.speed
    }
}

/// The DNS answer time tile.
pub fn dns_tile(
    rtt_ms: Option<u32>,
    trend: &TrendBuffer,
    baseline_ms: Option<u32>,
) -> Option<Tile> {
    let rtt = rtt_ms?;
    // A median of zero means the good hours do not know the overall speed
    // yet: a badge would say something nobody measured.
    let badge =
        baseline_ms
            .filter(|baseline| *baseline > 0)
            .and_then(
                |baseline| match verdict_for(u64::from(rtt), u64::from(baseline)) {
                    Verdict::Quick => Some(Badge {
                        text: "Quick".to_string(),
                        tone: Tone::Ok,
                    }),
                    Verdict::Busy => Some(Badge {
                        text: "Busy".to_string(),
                        tone: Tone::Wait,
                    }),
                    Verdict::Neither => None,
                },
            );
    let facts = window_fact(trend, "ms", "-").map(|fact| Facts::new(vec![fact]));
    Some(Tile {
        id: ID_DNS.to_string(),
        label: "DNS ANSWER".to_string(),
        badge,
        big: Some(BigValue {
            value: rtt.to_string(),
            unit: Some("ms".to_string()),
            tone: Tone::Muted,
        }),
        trend: line_trend(trend),
        facts,
        note: None,
    })
}

/// The signal tile.
pub fn signal_tile(stats: &PhoneStats, trend: &TrendBuffer) -> Option<Tile> {
    let big = match (stats.dbm, stats.signal) {
        (Some(dbm), _) => BigValue {
            value: dbm.to_string(),
            unit: Some("dBm".to_string()),
            tone: Tone::Muted,
        },
        (None, Some(level)) => BigValue {
            value: format!("{level}/4"),
            unit: None,
            tone: Tone::Muted,
        },
        (None, None) => return None,
    };
    let mut lines = Vec::new();
    // The window of the line comes first: it is the caption of the drawing
    // right above it, the way the DNS tile says it.
    if let Some(fact) = window_fact(trend, "dBm", " to ") {
        lines.push(fact);
    }
    lines.push(stats.net.display().to_string());
    if let Some(level) = stats.signal {
        lines.push(format!("Signal {level}/4"));
    }
    let mut draw = match line_trend(trend) {
        Some(draw) => Some(draw),
        None if stats.signal.is_some() => Some(Trend {
            kind: TrendKind::Line { points: Vec::new() },
            reference: None,
            levels: None,
        }),
        None => None,
    };
    if let Some(draw) = draw.as_mut() {
        draw.levels = stats.signal.map(|level| Levels { level, max: 4 });
    }
    Some(Tile {
        id: ID_SIGNAL.to_string(),
        label: "SIGNAL".to_string(),
        badge: None,
        big: Some(big),
        trend: draw,
        facts: Some(Facts::new(lines)),
        note: None,
    })
}

/// The muted signal tile with the one fact that explains a missing network
/// permission: a "--" and the hint, never an empty hole and never "No data".
fn muted_signal_tile(hint: &str) -> Tile {
    Tile {
        id: ID_SIGNAL.to_string(),
        label: "SIGNAL".to_string(),
        badge: None,
        big: Some(BigValue {
            value: "--".to_string(),
            unit: None,
            tone: Tone::Muted,
        }),
        trend: None,
        facts: Some(Facts::new(vec![hint.to_string()])),
        note: None,
    }
}

/// The phone's signal slot while its network permission is missing.
pub fn signal_permission_tile() -> Tile {
    muted_signal_tile(SIGNAL_PERMISSION_PHONE)
}

/// The laptop's signal slot while the phone says it may not read the network.
pub fn laptop_signal_permission_tile() -> Tile {
    muted_signal_tile(SIGNAL_PERMISSION_LAPTOP)
}

/// The phone tile: its temperature, what the phone says about the heat and its
/// battery.
pub fn phone_tile(
    temp_c_x10: Option<i16>,
    thr: Option<u8>,
    hr30: Option<u8>,
    battery_pct: Option<u8>,
    trend: &TrendBuffer,
    note: Option<&str>,
) -> Option<Tile> {
    let thermal = phone_heat::tile_line(temp_c_x10, thr);
    let headroom = phone_heat::forecast_line(hr30, thr);
    let big = temp_c_x10.map(|temp| BigValue {
        value: temp_value(temp),
        unit: Some("C".to_string()),
        tone: if temp >= phone_heat::HOT_C_X10 {
            Tone::Wait
        } else {
            Tone::Muted
        },
    });
    let mut lines = Vec::new();
    if let Some(fact) = window_fact_of(trend, "C", "-", window_temp) {
        lines.push(fact);
    }
    // The forecast takes the second line when it has something to say: the UI
    // has two facts, and the window stays first.
    if let Some(headroom) = headroom {
        lines.push(headroom);
    }
    if let Some(thermal) = thermal {
        lines.push(thermal);
    }
    if let Some(battery) = battery_pct {
        lines.push(format!("Battery {battery}%"));
    }
    let mut draw = match line_trend(trend) {
        Some(draw) => Some(draw),
        None if temp_c_x10.is_some() => Some(Trend {
            kind: TrendKind::Line { points: Vec::new() },
            reference: None,
            levels: None,
        }),
        None => None,
    };
    if let Some(draw) = draw.as_mut() {
        draw.reference = Some(i64::from(phone_heat::HOT_C_X10));
    }
    if big.is_none() && lines.is_empty() {
        return None;
    }
    Some(Tile {
        id: ID_PHONE.to_string(),
        label: "PHONE".to_string(),
        badge: None,
        big,
        trend: draw,
        facts: if lines.is_empty() {
            None
        } else {
            Some(Facts::new(lines))
        },
        note: note.map(str::to_string),
    })
}

/// Where the bytes of a session were counted. Two screens of one session
/// show different numbers because they count different things: the fact says
/// which, so the two never seem to contradict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// The tunnel device of the laptop: the payload the laptop carried.
    Laptop,
    /// The WireGuard counters of the phone: the same traffic, encrypted, so
    /// the overhead of the encryption is in the number.
    Phone,
}

impl Measure {
    /// The note of the tile: what the numbers count.
    pub fn note(self) -> &'static str {
        match self {
            Measure::Laptop => "Counted in the tunnel",
            Measure::Phone => "Counted with encryption",
        }
    }
}

/// The tile of the shared data, over its scope.
pub fn today_tile(totals: &ScopeTotals, scope: TileScope, measure: Measure) -> Option<Tile> {
    let down = totals.down_bytes;
    let up = totals.up_bytes;
    let total = down.saturating_add(up);
    if total == 0 {
        return None;
    }
    Some(Tile {
        id: ID_TODAY.to_string(),
        label: scope.word().to_string(),
        badge: None,
        big: Some(BigValue {
            value: usage::format_bytes(total),
            unit: None,
            tone: Tone::Muted,
        }),
        trend: Some(Trend {
            kind: TrendKind::Split { down, up },
            reference: None,
            levels: None,
        }),
        facts: Some(Facts::new(vec![
            format!(
                "Down {}, up {}",
                usage::format_bytes(down),
                usage::format_bytes(up)
            ),
            format!("Peak {} down", usage::format_rate(totals.peak_down_bps)),
        ])),
        // What was counted is a note of its own: appended to the numbers it
        // was cut off on the narrow tiles of the phone.
        note: Some(measure.note().to_string()),
    })
}

/// The today tile of the phone: what was shared today, and the plan of the
/// phone when it knows one. The numbers are the phone's own budget.
pub fn phone_today_tile(
    totals: &ScopeTotals,
    used_bytes: u64,
    allow_bytes: u64,
    scope: TileScope,
) -> Option<Tile> {
    let mut tile = today_tile(totals, scope, Measure::Phone)?;
    if allow_bytes > 0 {
        if let Some(facts) = tile.facts.as_mut() {
            // The peak is the line the plan replaces; find it by its word so a
            // reordering of the facts cannot silently drop the plan.
            if let Some(peak) = facts
                .lines
                .iter_mut()
                .find(|line| line.starts_with("Peak "))
            {
                *peak = format!(
                    "Plan: {} of {} this month",
                    usage::format_bytes(used_bytes),
                    usage::format_bytes(allow_bytes)
                );
            }
        }
    }
    Some(tile)
}

/// The tile of the leg between the laptop and the phone.
pub fn wifi_tile(leg: &WifiLeg, trend: &TrendBuffer, phone_loss_pct: Option<u8>) -> Option<Tile> {
    Some(Tile {
        id: ID_WIFI.to_string(),
        label: "WI-FI LEG".to_string(),
        badge: None,
        big: Some(BigValue {
            value: leg.median_ms.to_string(),
            unit: Some("ms".to_string()),
            tone: Tone::Muted,
        }),
        trend: line_trend(trend),
        facts: Some(Facts::new({
            let mut lines = Vec::new();
            if let Some(fact) = window_fact(trend, "ms", "-") {
                lines.push(fact);
            }
            lines.push(format!("Loss {}% of {}", leg.loss_pct, leg.samples));
            lines
        })),
        note: wireguard_loss_note(phone_loss_pct),
    })
}

/// The phone's own WireGuard loss as the note of the Wi-Fi leg. The share is
/// the laptop's packets the phone missed; `None` while the tunnel carried too
/// little to tell.
pub fn wireguard_loss_note(phone_loss_pct: Option<u8>) -> Option<String> {
    phone_loss_pct.map(|pct| format!("WireGuard loss {pct}%"))
}

/// The DNS tile of the phone: how long the carrier of the phone answers.
pub fn phone_dns_tile(rtt_ms: Option<u32>, trend: &TrendBuffer) -> Option<Tile> {
    let rtt = rtt_ms?;
    let facts = window_fact(trend, "ms", "-").map(|fact| Facts::new(vec![fact]));
    Some(Tile {
        id: ID_DNS.to_string(),
        label: "DNS".to_string(),
        badge: None,
        big: Some(BigValue {
            value: rtt.to_string(),
            unit: Some("ms".to_string()),
            tone: Tone::Muted,
        }),
        trend: line_trend(trend),
        facts,
        note: None,
    })
}

/// The heat tile of the phone: its temperature and what Android says about it.
pub fn heat_tile(
    temp_c_x10: Option<i16>,
    thr: Option<u8>,
    hr30: Option<u8>,
    trend: &TrendBuffer,
) -> Option<Tile> {
    let thermal = phone_heat::phone_tile_line(temp_c_x10, thr);
    let headroom = phone_heat::forecast_line(hr30, thr);
    let big = temp_c_x10.map(|temp| BigValue {
        value: temp_value(temp),
        unit: Some("C".to_string()),
        tone: if temp >= phone_heat::HOT_C_X10 {
            Tone::Wait
        } else {
            Tone::Muted
        },
    });
    let mut draw = match line_trend(trend) {
        Some(draw) => Some(draw),
        None if temp_c_x10.is_some() => Some(Trend {
            kind: TrendKind::Line { points: Vec::new() },
            reference: None,
            levels: None,
        }),
        None => None,
    };
    if let Some(draw) = draw.as_mut() {
        draw.reference = Some(i64::from(phone_heat::HOT_C_X10));
    }
    if big.is_none() && thermal.is_none() && headroom.is_none() {
        return None;
    }
    let mut lines = Vec::new();
    // The window of the line comes first: it is the caption of the drawing
    // right above it, the way the laptop tiles say it.
    if let Some(fact) = window_fact_of(trend, "C", "-", window_temp) {
        lines.push(fact);
    }
    if let Some(headroom) = headroom {
        lines.push(headroom);
    }
    lines.extend(thermal);
    Some(Tile {
        id: ID_HEAT.to_string(),
        label: "HEAT".to_string(),
        badge: None,
        big,
        trend: draw,
        facts: Some(Facts::new(lines)),
        note: None,
    })
}

/// The battery tile of the phone: how full it is and what it is doing.
pub fn battery_tile(
    battery_pct: Option<u8>,
    charging: Option<bool>,
    battery_saver: Option<bool>,
) -> Option<Tile> {
    let pct = battery_pct?;
    let mut lines = Vec::new();
    if let Some(charging) = charging {
        lines.push(if charging {
            "Charging".to_string()
        } else {
            "Not charging".to_string()
        });
    }
    if let Some(saver) = battery_saver {
        lines.push(if saver {
            "Battery saver on".to_string()
        } else {
            "Battery saver off".to_string()
        });
    }
    Some(Tile {
        id: ID_BATTERY.to_string(),
        label: "BATTERY".to_string(),
        badge: None,
        big: Some(BigValue {
            value: pct.to_string(),
            unit: Some("%".to_string()),
            tone: Tone::Muted,
        }),
        trend: Some(Trend {
            kind: TrendKind::Fill {
                value: pct,
                max: 100,
            },
            reference: None,
            levels: None,
        }),
        facts: if lines.is_empty() {
            None
        } else {
            Some(Facts::new(lines))
        },
        note: None,
    })
}

/// The speed tile of the phone: how fast data comes down now, and the peak
/// of the scope. The phone's own summary line carries the rates.
pub fn speed_tile(
    stats: &PhoneStats,
    trend: &TrendBuffer,
    scope: TileScope,
    peak_down_bps: u64,
) -> Option<Tile> {
    let (value, unit) = usage::format_rate_parts(stats.down_bps);
    let mut lines = Vec::new();
    // The window of the line comes first: it is the caption of the drawing
    // right above it, the way the laptop tiles say it. The peak of the scope
    // is a note under the facts: the two fact lines are the window and the up
    // rate, and the session tile already names the peak of the session.
    if let Some(fact) = window_rate_fact(trend) {
        lines.push(fact);
    }
    lines.push(format!("Up {}", usage::format_rate(stats.up_bps)));
    let note = if peak_down_bps > 0 {
        Some(format!(
            "Peak {} {}",
            usage::format_rate(peak_down_bps),
            scope.fact_word()
        ))
    } else {
        None
    };
    Some(Tile {
        id: ID_SPEED.to_string(),
        label: "SPEED".to_string(),
        badge: None,
        big: Some(BigValue {
            value,
            unit: Some(unit),
            tone: Tone::Muted,
        }),
        trend: line_trend(trend),
        facts: Some(Facts::new(lines)),
        note,
    })
}

/// The average tile: the average down and up rates of the scope, the peak
/// down and up rates and the data moved, from the usage statistics. The phone
/// and the laptop use it to summarise a session or today without a new shape:
/// the transfer is the existing split bar, the numbers use the existing rate
/// words.
///
/// The session knows its peak down only (that is what the session record
/// carries), so the peak up is shown for today and left out for a session;
/// the tile never invents a number nobody measured.
pub fn average_tile(
    summary: &UsageSummary,
    session: Option<&ScopeSession>,
    now_ms: i64,
    scope: TileScope,
) -> Option<Tile> {
    let scope = crate::scope::effective(scope, ID_AVERAGE, session);
    let (down_bytes, up_bytes, average_down, average_up, peak_down, peak_up) = match scope {
        TileScope::Session => {
            let session = session?;
            let elapsed_ms = now_ms.saturating_sub(session.started_at_ms).max(0) as u64;
            (
                session.down_bytes,
                session.up_bytes,
                crate::usage::average_rate(session.down_bytes, elapsed_ms),
                crate::usage::average_rate(session.up_bytes, elapsed_ms),
                session.peak_down_bps,
                0,
            )
        }
        _ => (
            summary.down_bytes,
            summary.up_bytes,
            summary.average_down_bps,
            summary.average_up_bps,
            summary.peak_down_bps,
            summary.peak_up_bps,
        ),
    };
    let total = down_bytes.saturating_add(up_bytes);
    let (value, unit) = usage::format_rate_parts(average_down);
    // The peak names its scope the way the moved data does: a peak of today
    // and a peak of the session are different facts, and the tile must not
    // look like it contradicts the session rows.
    let peak = if peak_up > 0 {
        format!(
            "Peak {} down, {} up {}",
            usage::format_rate(peak_down),
            usage::format_rate(peak_up),
            scope.fact_word()
        )
    } else {
        format!(
            "Peak {} down {}",
            usage::format_rate(peak_down),
            scope.fact_word()
        )
    };
    if total == 0 && average_down == 0 && average_up == 0 && peak_down == 0 {
        return None;
    }
    Some(Tile {
        id: ID_AVERAGE.to_string(),
        label: "AVERAGE".to_string(),
        badge: None,
        big: Some(BigValue {
            value,
            unit: Some(unit),
            tone: Tone::Muted,
        }),
        trend: Some(Trend {
            kind: TrendKind::Split {
                down: down_bytes,
                up: up_bytes,
            },
            reference: None,
            levels: None,
        }),
        facts: Some(Facts::new(vec![
            peak,
            format!("Moved {} {}", usage::format_bytes(total), scope.fact_word()),
        ])),
        note: Some(format!(
            "Avg {} up {}",
            usage::format_rate(average_up),
            scope.fact_word()
        )),
    })
}

/// The tile of the drops, over its scope.
pub fn drops_tile(
    events: &[Event],
    now_ms: i64,
    offset_minutes: i32,
    scope: TileScope,
    session: Option<&ScopeSession>,
) -> Option<Tile> {
    // The strip covers the window of the hour strip: the last day, hour by
    // hour, ending at the current hour. The cells come oldest first.
    let hour_ms = 3_600_000;
    let current_hour = crate::usage::hour_start_ms(now_ms);
    let strip_start = current_hour - (STRIP_CELLS as i64 - 1) * hour_ms;
    // An outage cannot still be open once a newer session began: the stop
    // or the restart that began it ended the outage too. A record left open
    // by an older daemon is closed at the start of the session here, so the
    // tile never says "not back yet" while the link is up.
    let settled = settle_open_events(events, session);
    let events = settled.as_slice();
    // The count follows the scope of the tile.
    let scope = crate::scope::effective(scope, ID_DROPS, session);
    // The window of the scope: the number, the strip and the fact line all
    // count the events inside it and nothing else.
    let window_start = crate::scope::window_start(scope, now_ms, offset_minutes, session);
    let recent: Vec<&Event> = events
        .iter()
        .filter(|event| in_scope(event.at_ms(), scope, now_ms, offset_minutes, session))
        .collect();
    // The strip is the last day whatever the scope is: the hours outside the
    // scope keep their place in the strip but are marked as outside, so the
    // strip never shows a red hour the number does not count.
    let drops = recent.iter().filter(|event| event.is_drop()).count();
    let mut cells = Vec::with_capacity(STRIP_CELLS);
    let offset_ms = i64::from(offset_minutes) * 60_000;
    for index in 0..STRIP_CELLS {
        let from = strip_start + index as i64 * hour_ms;
        let to = from + hour_ms;
        let in_hour: Vec<&Event> = recent
            .iter()
            .copied()
            .filter(|event| event.at_ms() >= from && event.at_ms() < to)
            .collect();
        let mark = if in_hour.iter().any(|event| event.is_drop()) {
            CellMark::Drop
        } else if !in_hour.is_empty() {
            CellMark::Away
        } else {
            CellMark::Unknown
        };
        // The hour is inside the window of the tile when it reaches into it.
        let inside = match window_start {
            Some(start) => to > start && from <= now_ms,
            None => false,
        };
        cells.push(Cell {
            hour: crate::usage::local_hour(from, offset_ms),
            mark,
            in_scope: inside,
        });
    }
    // The fact is the last event inside the window of the tile. When the
    // window has no event, the tile says so and names the newest event it
    // does not count.
    let mut lines = Vec::new();
    match recent.iter().max_by_key(|event| event.at_ms()) {
        Some(last) => {
            lines.push(event_fact(last, now_ms, offset_minutes));
            lines.push(match last {
                Event::Drop(record) => record.hint().to_string(),
                _ => crate::drop_history::NOT_A_DROP.to_string(),
            });
        }
        None => {
            lines.push(format!("No drops {}", scope.fact_subject()));
            // The newest event the tile does not count: the window has none,
            // so the last thing that happened is worth naming.
            let older = events
                .iter()
                .filter(|event| !in_scope(event.at_ms(), scope, now_ms, offset_minutes, session))
                .max_by_key(|event| event.at_ms());
            if let Some(older) = older {
                lines.push(format!(
                    "Last: {}",
                    event_fact(older, now_ms, offset_minutes)
                ));
            }
        }
    }
    Some(Tile {
        id: ID_DROPS.to_string(),
        label: format!("DROPS {}", scope.word()),
        badge: None,
        big: Some(BigValue {
            value: drops.to_string(),
            unit: None,
            tone: if drops > 0 { Tone::Err } else { Tone::Muted },
        }),
        trend: Some(Trend {
            kind: TrendKind::Hours { cells },
            reference: None,
            levels: None,
        }),
        facts: Some(Facts::new(lines)),
        note: None,
    })
}

/// The drops of one session, from the same events the drops tile counts.
/// The tracker's own counter also sees a stop or a restart of the phone as a
/// state change, so it can differ from the drops page; the rows and the tile
/// must agree, so both read the events inside the session window.
pub fn session_drop_count(events: &[Event], session: &ScopeSession, now_ms: i64) -> u32 {
    let settled = settle_open_events(events, Some(session));
    settled
        .iter()
        .filter(|event| {
            event.is_drop()
                && event.at_ms() >= session.started_at_ms
                && event.at_ms() <= now_ms
        })
        .count()
        .min(u32::MAX as usize) as u32
}

/// The events with every outage that is still open but began before the
/// session closed at the start of the session. An open outage inside the
/// session stays open: that one is really not back yet.
pub fn settle_open_events(events: &[Event], session: Option<&ScopeSession>) -> Vec<Event> {
    let Some(start) = session.map(|session| session.started_at_ms) else {
        return events.to_vec();
    };
    events
        .iter()
        .cloned()
        .map(|mut event| {
            let at_ms = event.at_ms();
            if event.down_ms().is_none() && at_ms < start {
                let ended = Some(start.saturating_sub(at_ms).max(0) as u64);
                match &mut event {
                    Event::Drop(record) => record.down_ms = ended,
                    Event::Away { down_ms, .. }
                    | Event::Stopped { down_ms, .. }
                    | Event::Updated { down_ms, .. } => *down_ms = ended,
                }
            }
            event
        })
        .collect()
}

/// The fact of an event: the time and what happened.
fn event_fact(event: &Event, now_ms: i64, offset_minutes: i32) -> String {
    match event {
        Event::Drop(record) => format!(
            "{}, {}",
            fact_time(record.at_ms, now_ms, offset_minutes),
            back_line(record.down_ms)
        ),
        Event::Away { at_ms, down_ms } => format!(
            "{}, {}",
            fact_time(*at_ms, now_ms, offset_minutes),
            away_fact(*down_ms)
        ),
        Event::Stopped { at_ms, .. } => format!(
            "{}, {}",
            fact_time(*at_ms, now_ms, offset_minutes),
            crate::drop_history::STOPPED_LABEL
        ),
        Event::Updated { at_ms, .. } => format!(
            "{}, {}",
            fact_time(*at_ms, now_ms, offset_minutes),
            crate::drop_history::UPDATED_LABEL
        ),
    }
}

/// The month names of the fact times.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The time of a fact, with the day when the day is not today. A time of
/// today reads "17:21"; a time of yesterday reads "yesterday 17:21"; an
/// older time reads "Nov 17 17:21". The caller gives the offset of the
/// local time in minutes.
fn fact_time(at_ms: i64, now_ms: i64, offset_minutes: i32) -> String {
    let offset_ms = i64::from(offset_minutes) * 60_000;
    let day = crate::usage::local_day(at_ms, offset_ms);
    let today = crate::usage::local_day(now_ms, offset_ms);
    let clock = clock_hhmm(at_ms, offset_minutes);
    if day == today {
        return clock;
    }
    if day == today - 1 {
        return format!("yesterday {clock}");
    }
    let (month, date) = calendar_date(day);
    format!("{} {} {}", MONTHS[(month - 1) as usize], date, clock)
}

/// The month and the day of the month of a local day number. The days since
/// 1970-01-01 become a month and a day of the month, without a year: a fact
/// names the day, not the year.
fn calendar_date(days: i64) -> (u32, u32) {
    // The civil calendar from the day number, in the usual way: the days of
    // the 400-year era, then the day of the year, then the month.
    let shifted = days + 719_468;
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_of_year = (5 * day_of_year + 2) / 153;
    let date = (day_of_year - (153 * month_of_year + 2) / 5 + 1) as u32;
    let month = if month_of_year < 10 {
        month_of_year + 3
    } else {
        month_of_year - 9
    } as u32;
    (month, date)
}

/// The fact of an absence of the phone.
fn away_fact(down_ms: Option<u64>) -> String {
    match down_ms {
        Some(ms) => format!(
            "{} {}",
            crate::drop_history::AWAY_LABEL,
            format_uptime(ms / 1000)
        ),
        None => format!(
            "{} {}",
            crate::drop_history::AWAY_LABEL,
            crate::drop_history::NOT_BACK_YET
        ),
    }
}

/// The strip of the hours of the day, with the quick and the busy hours.
pub fn hours_strip(summary: &Summary) -> Strip {
    let mut cells = Vec::with_capacity(STRIP_CELLS);
    for hour in 0..STRIP_CELLS {
        let mark = hour_mark(summary, hour as u8);
        cells.push(Cell {
            hour: hour as u8,
            mark,
            in_scope: true,
        });
    }
    Strip {
        cells,
        learning: summary.learning_line(),
        caption: summary.strip_caption(),
        title: crate::good_hours::HOURS_TITLE.to_string(),
    }
}

/// The mark of one hour of the strip.
fn hour_mark(summary: &Summary, hour: u8) -> CellMark {
    if summary.baseline_ms == 0 {
        return CellMark::Unknown;
    }
    let Some(local) = summary.hours.iter().find(|local| local.hour == hour) else {
        return CellMark::Unknown;
    };
    match verdict_for(u64::from(local.dns_ms), u64::from(summary.baseline_ms)) {
        Verdict::Quick => CellMark::Quick,
        Verdict::Busy => CellMark::Busy,
        Verdict::Neither => CellMark::Unknown,
    }
}

/// The strip of the hours of the day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strip {
    /// The cells of the strip, hour 0 to 23.
    pub cells: Vec<Cell>,
    /// The learning line, while there is no verdict yet.
    pub learning: Option<String>,
    /// The caption of the strip: the line that says why the strip looks the
    /// way it does. It is never missing when the strip has no verdict, so no
    /// strip is all grey without a word. A strip from an older window of the
    /// daemon has no caption.
    #[serde(default)]
    pub caption: Option<String>,
    /// The heading of the strip. A strip from an older window of the daemon
    /// has no heading and keeps the one it had.
    #[serde(default = "hours_title")]
    pub title: String,
}

/// The heading of a strip that comes from an older window of the daemon.
fn hours_title() -> String {
    crate::good_hours::HOURS_TITLE.to_string()
}

/// What the status card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCard {
    /// Whether the tunnel is up.
    pub connected: bool,
    /// The tone of the card: green when all is well, amber while the phone
    /// tries, grey when nothing runs.
    pub tone: Tone,
    /// The line at the top: "Connected to <name>".
    pub title: String,
    /// The line under it: the path and the time.
    pub subline: Option<String>,
    /// Whether the card offers the Stop button.
    pub stop: bool,
    /// The good-time notice, as a chip inside the card.
    pub notice: Option<String>,
    /// The word of the action button, when the platform asks for one. The
    /// laptop card leaves it empty and draws its own label.
    pub action: Option<String>,
}

/// What the caller knows about the connection.
#[derive(Debug, Clone, Default)]
pub struct StatusInput<'a> {
    /// Whether the tunnel is up.
    pub connected: bool,
    /// The name of the paired device.
    pub name: Option<&'a str>,
    /// The path of the connection, in words.
    pub path: Option<&'a str>,
    /// How long the tunnel has been up.
    pub uptime_secs: Option<u64>,
    /// The good-time notice, when there is one.
    pub notice: Option<&'a str>,
}

/// The status card of the home screen.
pub fn status_card(input: &StatusInput) -> StatusCard {
    let title = match (input.connected, input.name) {
        (true, Some(name)) if !name.is_empty() => format!("Connected to {name}"),
        (true, _) => "Connected".to_string(),
        (false, _) => "Not connected".to_string(),
    };
    // The card's time is the session's connected time: the Details page shows
    // the same field with the same words, so the two can never disagree. The
    // phone card keeps its own `Up h:mm:ss` line.
    // A card that is not connected has no running time to show.
    let uptime = input.uptime_secs.filter(|_| input.connected);
    let subline = match (input.path, uptime) {
        (Some(path), Some(secs)) => {
            Some(format!("{path} - up {}", crate::ui::ui_text::format_elapsed(secs)))
        }
        (Some(path), None) => Some(path.to_string()),
        (None, Some(secs)) => Some(format!("up {}", crate::ui::ui_text::format_elapsed(secs))),
        (None, None) => None,
    };
    StatusCard {
        connected: input.connected,
        tone: if input.connected {
            Tone::Ok
        } else {
            Tone::Muted
        },
        title,
        subline,
        stop: input.connected,
        notice: input.notice.map(str::to_string),
        action: None,
    }
}

/// What the phone is doing with the sharing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhoneShareState {
    /// The phone does not share.
    Idle,
    /// The phone is trying, but the tunnel is not up yet.
    Trying,
    /// The tunnel is up.
    Sharing,
}

impl PhoneShareState {
    /// The state of the code the phone sends over the JNI. A code the core
    /// does not know means that the phone does not share.
    pub fn from_code(code: i32) -> PhoneShareState {
        match code {
            1 => PhoneShareState::Trying,
            2 => PhoneShareState::Sharing,
            _ => PhoneShareState::Idle,
        }
    }

    /// The code the phone sends over the JNI.
    pub fn code(self) -> i32 {
        match self {
            PhoneShareState::Idle => 0,
            PhoneShareState::Trying => 1,
            PhoneShareState::Sharing => 2,
        }
    }
}

/// The whole home screen: the status card, the tiles and the hour strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeModel {
    /// The card at the top.
    pub status: StatusCard,
    /// The tiles of the grid.
    pub tiles: Vec<Tile>,
    /// The strip of the hours, when the good hours are on.
    pub hours: Option<Strip>,
}

/// The home screen of the laptop.
pub fn home_model(status: StatusCard, tiles: Vec<Tile>, hours: Option<Strip>) -> HomeModel {
    HomeModel {
        status,
        tiles,
        hours,
    }
}

/// What the phone knows about itself.
#[derive(Debug, Clone)]
pub struct PhoneHomeInput<'a> {
    /// What the phone is doing with the sharing.
    pub state: PhoneShareState,
    /// The line of the core that says what the phone is doing, when the
    /// tunnel is not up: "Retrying in 12 s" and the like.
    pub state_line: Option<&'a str>,
    /// The verdict of the link, as a chip inside the card.
    pub notice: Option<&'a str>,
    /// How long the phone has been sharing.
    pub uptime_secs: Option<u64>,
    /// What was shared today.
    pub today: Option<&'a UsageSummary>,
    /// The last summary line of the phone.
    pub stats: Option<&'a PhoneStats>,
    /// Whether the phone may read the network generation and the signal.
    pub network_permission: bool,
    /// The rings of the phone's own series.
    pub trends: &'a Trends,
    /// Whether the phone is charging.
    pub charging: Option<bool>,
    /// Whether the battery saver is on.
    pub battery_saver: Option<bool>,
    /// Whether the DNS answer time may be shown.
    pub show_dns: bool,
    /// Whether the battery may be shown.
    pub show_battery: bool,
    /// The scope of the today tile.
    pub today_scope: TileScope,
    /// The scope of the speed tile.
    pub speed_scope: TileScope,
    /// The scope of the average tile.
    pub average_scope: TileScope,
    /// The moment the caller built the input, in epoch milliseconds. The
    /// average tile needs it to divide a session's bytes by its length.
    pub now_ms: i64,
    /// The numbers of the sharing session.
    pub session: Option<&'a ScopeSession>,
    /// The laptop's report of the Wi-Fi link and the line of its levels,
    /// while the laptop sends them.
    pub range: Option<(crate::wifi_range::RangeReport, &'a TrendBuffer)>,
}

impl PhoneHomeInput<'_> {
    /// The scope the today tile really shows: the asked scope when it has a
    /// window, the default of the tile when it has none.
    pub fn effective_today_scope(&self) -> TileScope {
        crate::scope::effective(self.today_scope, ID_TODAY, self.session)
    }

    /// The scope the speed tile really shows.
    pub fn effective_speed_scope(&self) -> TileScope {
        crate::scope::effective(self.speed_scope, ID_SPEED, self.session)
    }

    /// The scope the average tile really shows.
    pub fn effective_average_scope(&self) -> TileScope {
        crate::scope::effective(self.average_scope, ID_AVERAGE, self.session)
    }

    /// The numbers of the today tile, over its scope.
    pub fn totals(&self) -> Option<ScopeTotals> {
        let today = self.today.map(|today| ScopeTotals {
            up_bytes: today.up_bytes,
            down_bytes: today.down_bytes,
            peak_down_bps: today.peak_down_bps,
        })?;
        let (_, numbers) = crate::scope::totals(self.today_scope, ID_TODAY, today, self.session);
        Some(numbers)
    }
}

/// The highest rate of the speed line that lies inside `scope`. The peak and
/// the line come from two samplers of the phone; a peak that read below the
/// line drawn right above it would contradict it, so the peak is never lower.
fn line_peak(input: &PhoneHomeInput, scope: TileScope) -> u64 {
    let from_ms = match (scope, input.session) {
        (TileScope::Session, Some(session)) => session.started_at_ms,
        _ => i64::MIN,
    };
    input
        .trends
        .speed()
        .max_since(from_ms)
        .map_or(0, |value| value.max(0) as u64)
}

/// The home screen of the phone: the card and the tiles.
pub fn phone_home(input: &PhoneHomeInput) -> HomeModel {
    let mut tiles = Vec::new();
    if !input.network_permission {
        tiles.push(signal_permission_tile());
    }
    if let Some(stats) = input.stats {
        if input.network_permission {
            if let Some(tile) = signal_tile(stats, input.trends.signal()) {
                tiles.push(tile);
            }
        }
        if input.show_dns {
            if let Some(tile) = phone_dns_tile(stats.rtt_ms, input.trends.dns()) {
                tiles.push(tile);
            }
        }
        if let Some(tile) = heat_tile(
            stats.temp_c_x10,
            stats.thr,
            stats.hr30,
            input.trends.phone(),
        ) {
            tiles.push(tile);
        }
    }
    if input.show_battery {
        let battery = input.stats.and_then(|stats| stats.battery);
        if let Some(tile) = battery_tile(battery, input.charging, input.battery_saver) {
            tiles.push(tile);
        }
    }
    // How far the laptop is, as the laptop hears the hotspot: only while
    // the tunnel is up and the laptop reports it.
    if input.state == PhoneShareState::Sharing {
        if let Some((report, trend)) = &input.range {
            tiles.push(crate::wifi_range::range_tile(report, trend));
        }
    }
    // A rate is a measurement of the tunnel: with no tunnel there is nothing
    // to show, so the tile is absent rather than zero.
    if input.state == PhoneShareState::Sharing {
        if let Some(stats) = input.stats {
            let speed_scope = input.effective_speed_scope();
            let peak = match speed_scope {
                TileScope::Session => input
                    .session
                    .map(|session| session.peak_down_bps)
                    .unwrap_or(0),
                _ => input.today.map(|today| today.peak_down_bps).unwrap_or(0),
            };
            let peak = peak.max(line_peak(input, speed_scope));
            if let Some(tile) = speed_tile(stats, input.trends.speed(), speed_scope, peak) {
                tiles.push(tile);
            }
        }
    }
    if let Some(totals) = input.totals() {
        let (used, allow) = input
            .stats
            .map(|stats| (stats.used_bytes, stats.allow_bytes))
            .unwrap_or((0, 0));
        let scope = input.effective_today_scope();
        let mut totals = totals;
        if input.state == PhoneShareState::Sharing {
            totals.peak_down_bps = totals.peak_down_bps.max(line_peak(input, scope));
        }
        if let Some(tile) = phone_today_tile(&totals, used, allow, scope) {
            tiles.push(tile);
        }
    }
    if let Some(today) = input.today {
        let scope = input.effective_average_scope();
        if let Some(tile) = average_tile(today, input.session, input.now_ms, scope) {
            tiles.push(tile);
        }
    }
    HomeModel {
        status: phone_status_card(input),
        tiles,
        hours: None,
    }
}

/// The status card of the phone.
pub fn phone_status_card(input: &PhoneHomeInput) -> StatusCard {
    let (title, tone, connected) = match input.state {
        PhoneShareState::Sharing => ("Sharing with the laptop", Tone::Ok, true),
        PhoneShareState::Trying => ("Trying to share", Tone::Wait, false),
        PhoneShareState::Idle => ("Not sharing", Tone::Muted, false),
    };
    // While the tunnel is up the card carries the numbers of the sharing and
    // nothing else: a transition line under "Sharing with the laptop" would
    // contradict the title. The rest of the time it says what the phone does.
    let subline = if input.state == PhoneShareState::Sharing {
        input
            .uptime_secs
            .map(|secs| format!("Up {}", format_uptime(secs)))
    } else {
        input.state_line.map(str::to_string)
    };
    StatusCard {
        connected,
        tone,
        title: title.to_string(),
        subline,
        stop: input.state != PhoneShareState::Idle,
        notice: input.notice.map(str::to_string),
        action: Some(if input.state == PhoneShareState::Idle {
            "Share".to_string()
        } else {
            "Stop sharing".to_string()
        }),
    }
}

/// The prefix of the text the phone parses.
pub const HOME_PREFIX: &str = "JFH1";
/// The version of the text the phone parses. It stays 1 while the new lines
/// use kinds the 0.2.2 parser already reads: the parser ignores an unknown
/// key and reads `kind=split | down= | up=`, and the parser and the encoder
/// ship in the same APK.
pub const HOME_VERSION: &str = "1";

/// The home model as the text the phone parses: the header, the card and one
/// line per tile. The fields of a line are separated by " | " and each field
/// is "key=value"; the words of the core never carry a bar.
pub fn phone_home_text(model: &HomeModel) -> String {
    let mut lines = vec![format!("{HOME_PREFIX} {HOME_VERSION}")];
    let status = &model.status;
    lines.push(format!(
        "s | connected={} | tone={} | title={} | sub={} | stop={} | action={} | notice={}",
        if status.connected { 1 } else { 0 },
        tone_word(status.tone),
        status.title,
        status.subline.clone().unwrap_or_default(),
        if status.stop { 1 } else { 0 },
        status.action.clone().unwrap_or_default(),
        status.notice.clone().unwrap_or_default(),
    ));
    for tile in &model.tiles {
        let mut fields = vec![
            format!("id={}", tile.id),
            format!("label={}", tile.label),
            match &tile.badge {
                Some(badge) => format!("badge={}", badge.text),
                None => "badge=".to_string(),
            },
            match &tile.badge {
                Some(badge) => format!("btone={}", tone_word(badge.tone)),
                None => "btone=muted".to_string(),
            },
        ];
        match &tile.big {
            Some(big) => {
                fields.push(format!("value={}", big.value));
                fields.push(format!("unit={}", big.unit.clone().unwrap_or_default()));
                fields.push(format!("tone={}", tone_word(big.tone)));
            }
            None => {
                fields.push("value=".to_string());
                fields.push("unit=".to_string());
                fields.push("tone=muted".to_string());
            }
        }
        match &tile.trend {
            Some(trend) => {
                match &trend.kind {
                    TrendKind::Line { points } => {
                        fields.push("kind=line".to_string());
                        let list: Vec<String> =
                            points.iter().map(|point| point.to_string()).collect();
                        fields.push(format!("points={}", list.join(",")));
                    }
                    TrendKind::Split { down, up } => {
                        fields.push("kind=split".to_string());
                        fields.push(format!("down={down}"));
                        fields.push(format!("up={up}"));
                    }
                    TrendKind::Hours { cells } => {
                        fields.push("kind=hours".to_string());
                        let list: Vec<String> = cells
                            .iter()
                            .map(|cell| format!("{}:{}", cell.hour, cell_word(cell)))
                            .collect();
                        fields.push(format!("cells={}", list.join(",")));
                    }
                    TrendKind::Fill { value, max } => {
                        fields.push("kind=fill".to_string());
                        fields.push(format!("fill={value}"));
                        fields.push(format!("fill_max={max}"));
                    }
                }
                fields.push(match trend.reference {
                    Some(reference) => format!("ref={reference}"),
                    None => "ref=".to_string(),
                });
                match trend.levels {
                    Some(levels) => {
                        fields.push(format!("level={}", levels.level));
                        fields.push(format!("level_max={}", levels.max));
                    }
                    None => {
                        fields.push("level=".to_string());
                        fields.push("level_max=".to_string());
                    }
                }
            }
            None => {
                fields.push("kind=none".to_string());
                fields.push("points=".to_string());
                fields.push("ref=".to_string());
                fields.push("level=".to_string());
                fields.push("level_max=".to_string());
            }
        }
        for line in tile
            .facts
            .as_ref()
            .map(|facts| facts.lines.as_slice())
            .unwrap_or(&[])
        {
            fields.push(format!("fact={line}"));
        }
        fields.push(format!("note={}", tile.note.clone().unwrap_or_default()));
        lines.push(format!("t | {}", fields.join(" | ")));
    }
    lines.join("\n")
}

/// The word of a tone in the text the phone parses.
fn tone_word(tone: Tone) -> &'static str {
    match tone {
        Tone::Ok => "ok",
        Tone::Wait => "wait",
        Tone::Err => "err",
        Tone::Muted => "muted",
    }
}

/// The range of the samples of a trend over its window, as the fact of a
/// tile. Every line trend says its window the same way, the way the DNS
/// tile does. The separator reads well for positive numbers; the signal
/// needs the word, because "-97--80" is not readable.
pub(crate) fn window_fact(trend: &TrendBuffer, unit: &str, separator: &str) -> Option<String> {
    window_fact_of(trend, unit, separator, |value| value.to_string())
}

/// The same fact, with the numbers written the way the tile writes them.
fn window_fact_of(
    trend: &TrendBuffer,
    unit: &str,
    separator: &str,
    format: impl Fn(i64) -> String,
) -> Option<String> {
    let stats = trend.stats()?;
    let (low, high) = (format(stats.min), format(stats.max));
    // A flat line is one value: "34 ms", never "34-34 ms".
    let range = if low == high {
        low
    } else {
        format!("{low}{separator}{high}")
    };
    Some(format!(
        "{range} {unit} in {} min",
        TREND_WINDOW_MS / 60_000
    ))
}

/// The range of the rates of a trend over its window, as the fact of a
/// tile. A rate that grew from a few bytes a second to a megabyte a second
/// does not fit one unit, so each end carries its own.
fn window_rate_fact(trend: &TrendBuffer) -> Option<String> {
    let stats = trend.stats()?;
    let (min, min_unit) = usage::format_rate_parts(stats.min.max(0) as u64);
    let (max, max_unit) = usage::format_rate_parts(stats.max.max(0) as u64);
    let range = if min_unit == max_unit && min == max {
        format!("{max} {max_unit}")
    } else if min_unit == max_unit {
        format!("{min}-{max} {max_unit}")
    } else {
        format!("{min} {min_unit} to {max} {max_unit}")
    };
    Some(format!("{range} in {} min", TREND_WINDOW_MS / 60_000))
}

/// The word of a mark in the text the phone parses.
fn mark_word(mark: CellMark) -> &'static str {
    match mark {
        CellMark::Quick => "quick",
        CellMark::Busy => "busy",
        CellMark::Unknown => "unknown",
        CellMark::Drop => "drop",
        CellMark::Away => "away",
    }
}

/// The word of a cell in the text the phone parses. A cell outside the
/// window of its tile has its own word, so the phone draws it fainter than
/// any hour of the window and never red.
pub fn cell_word(cell: &Cell) -> &'static str {
    if cell.in_scope {
        mark_word(cell.mark)
    } else {
        "outside"
    }
}

/// The time the tunnel has been up, as the card shows it.
pub fn format_uptime(secs: u64) -> String {
    let (hours, minutes, seconds) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{hours}:{minutes:02}:{seconds:02}")
}

/// The line of a trend that has samples.
pub(crate) fn line_trend(trend: &TrendBuffer) -> Option<Trend> {
    if trend.is_empty() {
        return None;
    }
    Some(Trend {
        kind: TrendKind::Line {
            points: trend.points(),
        },
        reference: None,
        levels: None,
    })
}

/// The temperature as the tile shows it: the number without the unit.
/// A temperature of a window range: always one decimal, so a range never
/// reads "33.7-34".
fn window_temp(temp_c_x10: i64) -> String {
    format!("{:.1}", temp_c_x10 as f64 / 10.0)
}

fn temp_value(temp_c_x10: i16) -> String {
    let tenths = i32::from(temp_c_x10);
    if tenths % 10 == 0 {
        format!("{}", tenths / 10)
    } else {
        format!("{:.1}", f64::from(tenths) / 10.0)
    }
}

/// The clock of a drop, in the local time.
fn clock_hhmm(at_ms: i64, offset_minutes: i32) -> String {
    let seconds = at_ms.div_euclid(1_000) + i64::from(offset_minutes) * 60;
    let day = seconds.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3_600, (day % 3_600) / 60)
}

/// How long the link was down, as the drop tile shows it.
fn back_line(down_ms: Option<u64>) -> String {
    match down_ms {
        Some(ms) => {
            let seconds = ms / 1000;
            format!("back after {}:{:02}", seconds / 60, seconds % 60)
        }
        None => crate::drop_history::NOT_BACK_YET.to_string(),
    }
}
