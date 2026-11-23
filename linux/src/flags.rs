//! Flag values persisted in `~/.config/jafnify/flags.toml`, next to
//! `state.toml`. It is a separate file on purpose: a 0.1.1 build never reads
//! or writes it, so a downgrade is lossless. Unknown keys are kept on write;
//! a missing or unreadable file only ever means the core defaults.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use jafnify_core::best_spot::{self, Mark, Sample};
use jafnify_core::bottleneck::{self, LinkLeg};
use jafnify_core::flags::{self, platform, Bucket, FeatureFlag};
use jafnify_core::good_hours;
use jafnify_core::phone_stats::{self, PhoneStats};
use jafnify_core::wifi_leg::WifiLeg;

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = "flags.toml";

/// Flag values with core defaults for every key that is not stored.
///
/// The whole TOML table is kept, not only the booleans, so a key written by
/// a newer build survives a load and save whatever its type is (A7). A key
/// whose value is not a boolean reads as the core default.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlagStore {
    values: toml::Table,
}

#[derive(Debug, thiserror::Error)]
pub enum FlagStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("atomic write failed: {0}")]
    AtomicWrite(String),
}

impl FlagStore {
    /// Load a store. A missing or unreadable file gives all defaults and is
    /// never an error that could stop the daemon.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(values) = text.parse::<toml::Table>() else {
            return Self::default();
        };
        Self { values }
    }

    /// The stored value, or the core default when the key is absent. A
    /// standard row is always on: a stored `false` from an older build is
    /// ignored, so an update never turns a standard feature off.
    pub fn get(&self, id: &str) -> bool {
        if flags::standard_id(id) {
            return true;
        }
        match self.values.get(id).and_then(toml::Value::as_bool) {
            Some(value) => value,
            None => flags::find(id).map(|flag| flag.default).unwrap_or(false),
        }
    }

    /// Store one value. The other keys, including unknown ones loaded from
    /// the file, are left as they are.
    pub fn set(&mut self, id: &str, value: bool) {
        self.values
            .insert(id.to_string(), toml::Value::Boolean(value));
    }

    /// Every declared flag with its effective value (stored or core default).
    pub fn effective(&self) -> BTreeMap<String, bool> {
        flags::FLAGS
            .iter()
            .map(|flag| (flag.id.to_string(), self.get(flag.id)))
            .collect()
    }

    /// Write the store atomically (temp + fsync + rename), 0600 like
    /// `state.toml`, so a reader never sees a half-written file.
    pub fn save(&self, path: &Path) -> Result<(), FlagStoreError> {
        if let Some(parent) = path.parent() {
            crate::settings::ensure_private_dir(parent)?;
        }
        let text = toml::to_string(&self.values)
            .map_err(|e| FlagStoreError::AtomicWrite(e.to_string()))?;
        let tmp = tmp_path(path);
        match fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            FlagStoreError::AtomicWrite(e.to_string())
        })?;
        Ok(())
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| FILE_NAME.to_string());
    path.with_file_name(format!(".{file_name}.tmp"))
}

/// Why `SetSettings` may not write a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagRefusal {
    /// No row with this id is declared.
    Unknown,
    /// The row exists but no frontend reads it yet.
    NotWired,
    /// The row is a standard feature: always on, with no switch to write.
    NotSwitch,
    /// The row does not apply to this platform.
    NotSupported,
}

impl FlagRefusal {
    /// The log line for a refused write.
    pub fn reason(self) -> &'static str {
        match self {
            FlagRefusal::Unknown => "not a declared flag",
            FlagRefusal::NotWired => "not wired yet",
            FlagRefusal::NotSwitch => "always on; no switch",
            FlagRefusal::NotSupported => "not supported on this platform",
        }
    }
}

/// The row a `SetSettings` request may write on Linux.
pub fn writable(id: &str, platform: u8) -> Result<&'static FeatureFlag, FlagRefusal> {
    writable_in(flags::FLAGS, id, platform)
}

/// The pure lookup; `table` is a parameter so a test can pass synthetic rows.
pub fn writable_in<'a>(
    table: &'a [FeatureFlag],
    id: &str,
    platform: u8,
) -> Result<&'a FeatureFlag, FlagRefusal> {
    let flag = table
        .iter()
        .find(|flag| flag.id == id)
        .ok_or(FlagRefusal::Unknown)?;
    if !flag.wired {
        return Err(FlagRefusal::NotWired);
    }
    if flags::is_standard(flag) {
        return Err(FlagRefusal::NotSwitch);
    }
    if !flags::supports(flag, platform) {
        return Err(FlagRefusal::NotSupported);
    }
    Ok(flag)
}

/// One rendered flag row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagRow {
    pub id: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub value: bool,
}

/// Where a section is drawn on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Inline after the existing switches; no extra header.
    MainList,
    /// Inline at the end of the existing Advanced body; no second header.
    AdvancedBody,
    /// A new collapsed header below the existing Advanced one.
    OwnHeader,
}

/// The Linux placement of a bucket's rows.
fn placement(bucket: Bucket) -> Placement {
    match bucket {
        // A standard row has no switch; it never reaches a section, but the
        // exhaustive match keeps a new bucket from compiling silently.
        Bucket::Standard => Placement::MainList,
        Bucket::Settings => Placement::MainList,
        Bucket::Advanced => Placement::AdvancedBody,
        Bucket::Experimental => Placement::OwnHeader,
    }
}

/// One settings bucket with at least one rendered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagSection {
    pub bucket: Bucket,
    pub title: &'static str,
    pub collapsed_by_default: bool,
    pub note: Option<&'static str>,
    pub placement: Placement,
    pub rows: Vec<FlagRow>,
}

/// The Linux settings section model, grouped by bucket. A bucket with no
/// rendered row is left out, so today's all-unwired table adds nothing.
pub fn sections(values: &BTreeMap<String, bool>) -> Vec<FlagSection> {
    sections_from(flags::FLAGS, values)
}

/// The read-only lines for the Linux phone block. Empty when the flag is off;
/// while disconnected with no value it says "Connect first.".
///
/// The passive leg is the measurement of the phone requests themselves. It
/// wins over the last link test, which is one shot: the verdict then says
/// "likely", because the round trip also carries the phone's own reply delay.
pub fn phone_stats_lines(
    enabled: bool,
    connected: bool,
    stats: Option<&PhoneStats>,
    age_secs: Option<u64>,
    unavailable: bool,
    link: Option<LinkLeg>,
    wifi: Option<&WifiLeg>,
) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    let mut lines = if !connected && stats.is_none() && !unavailable {
        vec![phone_stats::CONNECT_FIRST_LINE.to_string()]
    } else {
        phone_stats::display_lines(stats, age_secs, unavailable, true)
    };
    match wifi {
        Some(leg) => {
            lines.push(bottleneck::passive_line(Some(leg.link_leg()), stats));
            lines.push(leg.line());
        }
        None => lines.push(bottleneck::line(link, stats)),
    }
    if let Some(switches) = stats.and_then(|stats| stats.switches) {
        lines.push(jafnify_core::network_report::switches_line(switches));
    }
    lines
}

/// The line of the heat warning, for the phone block. The heat line replaces
/// the battery feature's own temperature line: the phone shows one
/// temperature, not two. The temperature is the one that made the phone hot,
/// so the line never calls a cooler phone hot.
pub fn heat_lines(
    mut lines: Vec<String>,
    enabled: bool,
    hot_temp_c_x10: Option<i16>,
) -> Vec<String> {
    if !enabled {
        return lines;
    }
    let Some(temp) = hot_temp_c_x10 else {
        return lines;
    };
    jafnify_core::phone_heat::replace_temperature_line(&mut lines);
    lines.push(jafnify_core::phone_heat::line(temp));
    lines
}

/// The line of the good-time notice, for the phone block.
pub fn good_time_lines(mut lines: Vec<String>, enabled: bool, good: bool) -> Vec<String> {
    if enabled && good {
        lines.push(jafnify_core::good_time::NOTICE_PREFIX.to_string());
    }
    lines
}

/// The lines of the best-spot block. The block adds nothing while the switch
/// is off; with the switch on it shows the live line, one line per marked
/// spot and the verdict. The wording is the core's: this only places it.
pub fn best_spot_lines(
    mut lines: Vec<String>,
    enabled: bool,
    samples: &[Sample],
    marks: &[Mark],
    now_ms: i64,
) -> Vec<String> {
    if !enabled {
        return lines;
    }
    lines.push(best_spot::live_line(live_sample(samples, now_ms)));
    let mut spots = Vec::new();
    let mut held = false;
    for mark in marks {
        match best_spot::spot_at(samples, *mark) {
            Some(spot) => {
                lines.push(best_spot::spot_line(&spot));
                spots.push(spot);
            }
            // The window of this spot is too short: ask for a moment more.
            // The line is said once, not once per mark.
            None => {
                if !held {
                    held = true;
                    lines.push(best_spot::hold_line().to_string());
                }
            }
        }
    }
    lines.push(best_spot::verdict(&spots));
    lines
}

/// The lines of the good-hours block. The block adds nothing while the
/// switch is off; with the switch on it shows what the learning knows and
/// the note about the places. The wording is the core's: this only places it.
pub fn good_hours_lines(enabled: bool, lines: Vec<String>) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    let mut out = lines;
    out.push(good_hours::PLACE_NOTE.to_string());
    out
}

/// The newest sample that is not in the future.
fn live_sample(samples: &[Sample], now_ms: i64) -> Option<&Sample> {
    samples.iter().rev().find(|sample| sample.at_ms <= now_ms)
}

/// The pure builder; `table` is a parameter so a test can pass synthetic rows.
pub fn sections_from(table: &[FeatureFlag], values: &BTreeMap<String, bool>) -> Vec<FlagSection> {
    let mut sections = Vec::new();
    for bucket in Bucket::ORDER {
        let rows: Vec<FlagRow> = table
            .iter()
            .filter(|flag| flag.bucket == bucket && flags::is_rendered(flag, platform::LINUX))
            .map(|flag| FlagRow {
                id: flag.id,
                label: flag.label,
                help: flag.help,
                value: values.get(flag.id).copied().unwrap_or(flag.default),
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        sections.push(FlagSection {
            bucket,
            title: bucket.title(),
            collapsed_by_default: bucket.collapsed_by_default(),
            note: bucket.note(),
            placement: placement(bucket),
            rows,
        });
    }
    sections
}
