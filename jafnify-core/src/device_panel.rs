//! What the phone can do right now: the facts and the limits Android puts on
//! the app.
//!
//! Pure: the caller hands over one blob of "key=value;key=value" pairs and
//! gets back the lines. The keys are the field names: `maker`, `model`,
//! `android`, `sdk`, `app_version`, `battery_pct`, `charging`, `temp_c_x10`,
//! `thermal`, `battery_saver`, `exempt_from_optimisation`,
//! `background_restricted`, `data_saver`, `mem_free_mb`, `mem_total_mb` and
//! `uptime_secs`. An unknown key is ignored and a value that does not parse
//! leaves its fact unknown, so a newer phone cannot break the panel.
//!
//! Nothing is sent anywhere: the facts stay on the phone.

use crate::battery::temp_text;

/// What the phone reports about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceFacts {
    /// The maker of the phone.
    pub maker: Option<String>,
    /// The model of the phone.
    pub model: Option<String>,
    /// The Android version, "Android 14".
    pub android: Option<String>,
    /// The SDK level of the Android version.
    pub sdk: Option<u32>,
    /// The version of Jafnify on the phone.
    pub app_version: Option<String>,
    /// The battery level in percent.
    pub battery_pct: Option<u8>,
    /// Whether the phone is charging.
    pub charging: Option<bool>,
    /// The battery temperature in tenths of a degree Celsius.
    pub temp_c_x10: Option<i16>,
    /// The thermal status of the phone.
    pub thermal: Option<u8>,
    /// Whether the battery saver is on.
    pub battery_saver: Option<bool>,
    /// Whether the phone is exempt from the battery optimisation.
    pub exempt_from_optimisation: Option<bool>,
    /// Whether the background is restricted.
    pub background_restricted: Option<bool>,
    /// The data saver status of the phone.
    pub data_saver: Option<u8>,
    /// The free memory in megabytes.
    pub mem_free_mb: Option<u64>,
    /// The total memory in megabytes.
    pub mem_total_mb: Option<u64>,
    /// How long the phone has been up, in seconds.
    pub uptime_secs: Option<u64>,
}

impl DeviceFacts {
    /// The facts of the blob. An unknown key is ignored; a value that does
    /// not parse leaves its fact unknown.
    pub fn parse(blob: &str) -> Self {
        let mut facts = DeviceFacts::default();
        for pair in blob.split([';', '\n']) {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "maker" => facts.maker = Some(value.to_string()),
                "model" => facts.model = Some(value.to_string()),
                "android" => facts.android = Some(value.to_string()),
                "sdk" => facts.sdk = value.parse().ok(),
                "app_version" => facts.app_version = Some(value.to_string()),
                "battery_pct" => facts.battery_pct = parse_battery(value),
                "charging" => facts.charging = parse_bool(value),
                "temp_c_x10" => facts.temp_c_x10 = value.parse().ok(),
                "thermal" => facts.thermal = value.parse().ok(),
                "battery_saver" => facts.battery_saver = parse_bool(value),
                "exempt_from_optimisation" => facts.exempt_from_optimisation = parse_bool(value),
                "background_restricted" => facts.background_restricted = parse_bool(value),
                "data_saver" => facts.data_saver = value.parse().ok(),
                "mem_free_mb" => facts.mem_free_mb = value.parse().ok(),
                "mem_total_mb" => facts.mem_total_mb = value.parse().ok(),
                "uptime_secs" => facts.uptime_secs = value.parse().ok(),
                _ => {}
            }
        }
        facts
    }

    /// The facts as lines. An unknown fact leaves its line out.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut phone = Vec::new();
        if let Some(maker) = &self.maker {
            phone.push(maker.clone());
        }
        if let Some(model) = &self.model {
            phone.push(model.clone());
        }
        if !phone.is_empty() {
            lines.push(format!("Phone: {}", phone.join(" ")));
        }
        if let Some(android) = &self.android {
            let version = android.strip_prefix("Android ").unwrap_or(android);
            let mut line = format!("Android: {version}");
            if let Some(sdk) = self.sdk {
                line.push_str(&format!(" (SDK {sdk})"));
            }
            lines.push(line);
        }
        if let Some(version) = &self.app_version {
            lines.push(format!("Jafnify: {version}"));
        }
        if let Some(pct) = self.battery_pct {
            let mut line = format!("Battery: {pct}%");
            if self.charging == Some(true) {
                line.push_str(", charging");
            }
            lines.push(line);
        }
        if let Some(temp) = self.temp_c_x10 {
            lines.push(format!("Phone temperature: {}", temp_text(temp)));
        }
        if let Some(on) = self.battery_saver {
            lines.push(format!("Battery saver: {}", on_off(on)));
        }
        if let Some(exempt) = self.exempt_from_optimisation {
            lines.push(format!("Battery optimisation: {}", on_off(!exempt)));
        }
        if let Some(restricted) = self.background_restricted {
            lines.push(format!("Background restricted: {}", yes_no(restricted)));
        }
        if let Some(status) = self.data_saver {
            lines.push(format!("Data saver: {}", on_off(status != 0)));
        }
        if let Some(free) = self.mem_free_mb {
            let mut line = format!("Memory free: {free} MB");
            if let Some(total) = self.mem_total_mb {
                line.push_str(&format!(" of {total} MB"));
            }
            lines.push(line);
        }
        if let Some(secs) = self.uptime_secs {
            lines.push(format!("Uptime: {}", uptime_text(secs)));
        }
        lines
    }

    /// The hints about the limits Android puts on Jafnify. A limit that is
    /// not known leaves its hint out.
    pub fn hints(&self) -> Vec<String> {
        let mut hints = Vec::new();
        if self.battery_saver == Some(true) {
            hints.push(
                "Battery saver is on: Android may stop sharing in the background.".to_string(),
            );
        }
        if self.exempt_from_optimisation == Some(false) {
            hints.push(
                "Battery optimisation is on for Jafnify: the phone can stop sharing when the screen is off."
                    .to_string(),
            );
        }
        if self.background_restricted == Some(true) {
            hints.push(
                "Background restricted: the phone can stop sharing in the background.".to_string(),
            );
        }
        if self.thermal.is_some_and(|status| status >= 3) {
            hints.push("The phone is very hot: Android can slow the sharing down.".to_string());
        }
        if self
            .data_saver
            .is_some_and(|status| (1..=3).contains(&status))
        {
            hints.push("Data saver is on: it can slow sharing.".to_string());
        }
        hints
    }

    /// The lines and then the hints, one per line.
    pub fn text(&self) -> String {
        let mut all = self.lines();
        all.extend(self.hints());
        all.join("\n")
    }
}

/// A battery level in percent; a value above 100 does not parse.
fn parse_battery(value: &str) -> Option<u8> {
    let pct: u8 = value.parse().ok()?;
    if pct <= 100 {
        Some(pct)
    } else {
        None
    }
}

/// A boolean of the blob.
fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// "on" or "off".
fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// "yes" or "no".
fn yes_no(yes: bool) -> &'static str {
    if yes {
        "yes"
    } else {
        "no"
    }
}

/// The uptime in words: "3 h 12 min".
fn uptime_text(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}
