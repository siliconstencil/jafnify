//! How hot the phone is, what the phone says about it, and the words for it.
//!
//! Pure: the caller hands over the temperature and the phone's own thermal
//! status and gets back a level and the lines to show. The hysteresis keeps
//! the warning from flapping: a phone that got hot stays hot until it cooled
//! down, and a phone that limits itself keeps the warning until it says it is
//! fine again.

/// The heat levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Heat {
    /// The phone is cool enough.
    #[default]
    Normal,
    /// The phone is warm, but not hot.
    Warm,
    /// The phone is hot: the mobile data can suffer.
    Hot,
}

/// At or above this temperature the phone counts as hot: 42.0 C.
pub const HOT_C_X10: i16 = 420;

/// How many fresh readings at or above [`HOT_C_X10`] in a row the temperature
/// warning needs. One reading can be a stale or half-ready sensor; the summary
/// cadence brings the second within seconds.
pub const HOT_READINGS: u8 = 2;

/// Below this temperature a hot phone counts as cool again: 39.0 C.
pub const COOL_C_X10: i16 = 390;

/// Android's thermal status: at this level and above the phone limits itself.
pub const THROTTLE_SEVERE: u8 = 3;

/// Android's thermal status: at this level the phone slows down a little. It
/// is shown in the tile, but it is not worth a notification.
pub const THROTTLE_MODERATE: u8 = 2;

/// Android's thermal status: at this level and below the phone is fine.
pub const THROTTLE_LIGHT: u8 = 1;

/// The thermal headroom forecast (percent, 100 = severe throttling) at which
/// the phone likely throttles within the forecast window.
pub const HEADROOM_THROTTLE_PCT: u8 = 100;

/// The thermal headroom forecast at which the phone is likely warmer soon. It
/// is a hint, not a fault.
pub const HEADROOM_WARM_PCT: u8 = 85;

/// The level of a temperature, without any memory of the past.
pub fn level(temp_c_x10: Option<i16>) -> Option<Heat> {
    temp_c_x10.map(level_of)
}

fn level_of(temp_c_x10: i16) -> Heat {
    if temp_c_x10 >= HOT_C_X10 {
        Heat::Hot
    } else if temp_c_x10 >= COOL_C_X10 {
        Heat::Warm
    } else {
        Heat::Normal
    }
}

/// The warning a reading earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    /// The phone says it is limiting itself because of the heat.
    Throttling,
    /// The phone is too hot and does not report its thermal status.
    Hot {
        /// The temperature of the phone, in tenths of a degree.
        temp_c_x10: i16,
    },
}

/// What the tracker saw in one update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeatChange {
    /// The level after the update.
    pub level: Heat,
    /// The temperature the level came from, when the phone reported one.
    pub temp_c_x10: Option<i16>,
    /// The phone just became hot by temperature.
    pub became_hot: bool,
    /// The phone just cooled down to normal.
    pub cooled: bool,
    /// The phone just stopped limiting itself.
    pub stopped_throttling: bool,
    /// The warning this reading earns, when it earns one.
    pub warning: Option<Warning>,
}

/// The heat state of the phone, with the hysteresis.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeatTracker {
    level: Heat,
    hot_temp_c_x10: Option<i16>,
    throttling: bool,
    /// Consecutive fresh readings at or above [`HOT_C_X10`].
    hot_readings: u8,
    /// The phone has reported its own thermal status in this session.
    thr_seen: bool,
    /// The temperature warning already went out; one episode, one line.
    warned_hot: bool,
}

impl HeatTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The level the phone is on now.
    pub fn level(&self) -> Heat {
        self.level
    }

    /// Whether the phone says it is limiting itself.
    pub fn throttling(&self) -> bool {
        self.throttling
    }

    /// The temperature that made the phone hot, while it is hot. The line
    /// shows this number, so it never calls a cooler phone hot.
    pub fn hot_temp_c_x10(&self) -> Option<i16> {
        self.hot_temp_c_x10
    }

    /// Whether the phone has reported its own thermal status in this session.
    pub fn thr_seen(&self) -> bool {
        self.thr_seen
    }

    /// Forget the phone: the next reading starts from cool.
    pub fn reset(&mut self) {
        self.level = Heat::Normal;
        self.hot_temp_c_x10 = None;
        self.throttling = false;
        self.hot_readings = 0;
        self.thr_seen = false;
        self.warned_hot = false;
    }

    /// Feed a reading. `temp_c_x10` is the temperature and `thr` is the
    /// phone's own thermal status (0 to 6). `None` means the phone did not
    /// report it: the state stays as it is and there is nothing to report.
    pub fn update(&mut self, temp_c_x10: Option<i16>, thr: Option<u8>) -> Option<HeatChange> {
        if temp_c_x10.is_none() && thr.is_none() {
            return None;
        }
        let was = self.level;
        let was_throttling = self.throttling;
        let mut became_hot = false;
        let mut cooled = false;
        let mut level = self.level;
        if let Some(temp) = temp_c_x10 {
            let fresh = level_of(temp);
            let next = if was == Heat::Hot {
                // A hot phone stays hot until it cooled down.
                if temp < COOL_C_X10 {
                    Heat::Normal
                } else {
                    Heat::Hot
                }
            } else {
                fresh
            };
            became_hot = was != Heat::Hot && fresh == Heat::Hot;
            cooled = was == Heat::Hot && next == Heat::Normal;
            level = next;
            // Two fresh readings in a row, never one: a single reading after a
            // reconnect can be stale.
            if temp >= HOT_C_X10 {
                self.hot_readings = self.hot_readings.saturating_add(1).min(HOT_READINGS);
            } else {
                self.hot_readings = 0;
            }
            if cooled {
                self.hot_readings = 0;
                self.warned_hot = false;
            }
            if became_hot {
                self.hot_temp_c_x10 = Some(temp);
            } else if next != Heat::Hot {
                self.hot_temp_c_x10 = None;
            }
        }
        self.level = level;
        // A phone that reported its thermal status at least once in this
        // session is never warned about by temperature alone: it speaks for
        // itself, and one missing key does not make it mute.
        if thr.is_some() {
            self.thr_seen = true;
        }
        // The phone speaks for itself: three and above it limits itself, one
        // and below it is fine again. Two keeps the warning, it is only a
        // light slowdown.
        let mut became_throttling = false;
        let mut stopped_throttling = false;
        match thr {
            Some(status) if status >= THROTTLE_SEVERE => {
                self.throttling = true;
                became_throttling = !was_throttling;
            }
            Some(status) if status <= THROTTLE_LIGHT => {
                self.throttling = false;
                stopped_throttling = was_throttling;
            }
            _ => {}
        }
        let warning = if became_throttling {
            Some(Warning::Throttling)
        } else if self.level == Heat::Hot
            && !self.thr_seen
            && thr.is_none()
            && self.hot_readings >= HOT_READINGS
            && !self.warned_hot
        {
            // A phone that does not report its thermal status is warned
            // about by its temperature, after the second fresh reading.
            self.warned_hot = true;
            self.hot_temp_c_x10
                .map(|temp| Warning::Hot { temp_c_x10: temp })
        } else {
            None
        };
        Some(HeatChange {
            level,
            temp_c_x10,
            became_hot,
            cooled,
            stopped_throttling,
            warning,
        })
    }
}

/// The line the laptop and the phone show.
pub fn line(temp_c_x10: i16) -> String {
    format!("Phone is hot ({})", crate::battery::temp_text(temp_c_x10))
}

/// The title of the desktop notification.
pub fn title(temp_c_x10: i16) -> String {
    line(temp_c_x10)
}

/// The title of the notification when the phone limits itself.
pub fn throttling_title() -> &'static str {
    "Phone is throttling because of heat"
}

/// The help under that title. The slowdown is the phone's own word.
pub fn throttling_help() -> &'static str {
    "Android says the phone is limiting itself; mobile data can slow down. Move it out of the sun; unplug the charger if it is charging."
}

/// The help of a phone that does not report its thermal status. It never
/// claims that this phone slows down.
pub fn help() -> &'static str {
    "Hot phones can slow mobile data down; this one does not report throttling."
}

/// The thermal line of the phone tile. It never claims a slowdown the phone
/// did not report. A phone that reports nothing and is at the hot threshold
/// reads "warm" here: the tile states a temperature, the notification of the
/// same phone says "hot", and both words are a product choice.
pub fn tile_line(temp_c_x10: Option<i16>, thr: Option<u8>) -> Option<String> {
    match thr {
        Some(status) if status >= THROTTLE_SEVERE => {
            Some("Android says the phone is limiting itself".to_string())
        }
        Some(status) if status >= THROTTLE_MODERATE => {
            Some("Android: light throttling".to_string())
        }
        Some(_) => None,
        None => temp_c_x10
            .filter(|temp| *temp >= HOT_C_X10)
            .map(|temp| format!("Phone is warm ({})", crate::battery::temp_text(temp))),
    }
}

/// The thermal line of the phone's own heat tile. The phone knows the thermal
/// status, so it always says what Android says - including that nothing is
/// wrong. A phone that does not report the status only says that it is warm.
pub fn phone_tile_line(temp_c_x10: Option<i16>, thr: Option<u8>) -> Option<String> {
    match thr {
        Some(status) if status >= THROTTLE_SEVERE => {
            Some("Android says the phone is limiting itself".to_string())
        }
        Some(status) if status >= THROTTLE_MODERATE => {
            Some("Android: light throttling".to_string())
        }
        Some(_) => Some("Android: no throttling".to_string()),
        None => temp_c_x10
            .filter(|temp| *temp >= HOT_C_X10)
            .map(|temp| format!("Phone is warm ({})", crate::battery::temp_text(temp))),
    }
}

/// The thermal headroom forecast as one short line for the heat tile, shared
/// by the phone and the laptop. Android reports the forecast in percent, 100
/// at severe throttling; `None` when the phone does not report it.
pub fn headroom_line(hr30: Option<u8>) -> Option<String> {
    match hr30 {
        Some(pct) if pct >= HEADROOM_THROTTLE_PCT => {
            Some("Likely throttling within 30 s".to_string())
        }
        Some(pct) if pct >= HEADROOM_WARM_PCT => Some("Likely warmer soon".to_string()),
        _ => None,
    }
}

/// The forecast line of a heat tile next to Android's thermal status. A phone
/// that already throttles says so, and that real fault keeps its line: the
/// forecast never pushes it off a two-fact tile.
pub fn forecast_line(hr30: Option<u8>, thr: Option<u8>) -> Option<String> {
    if thr.is_some_and(|status| status >= THROTTLE_MODERATE) {
        return None;
    }
    headroom_line(hr30)
}

/// The battery feature shows the temperature in its own line. When the heat
/// warning speaks, that line is replaced: the phone shows one temperature.
pub fn replace_temperature_line(lines: &mut Vec<String>) {
    lines.retain(|line| !is_temperature_line(line));
}

/// True for the battery feature's own temperature lines. The heat line reads
/// "Phone is hot (43.1 C)", so it does not match.
fn is_temperature_line(line: &str) -> bool {
    line.starts_with("Phone temperature:") || line.starts_with("Phone is hot:")
}
