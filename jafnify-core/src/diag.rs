//! The diagnostic journal's line format, rotation naming and sample cadence.
//!
//! This module is pure: the platform keeps the file handle, this module decides
//! what one record looks like, which rotated name a file gets, and when the
//! next stats sample is due. One record is one line:
//! `ts=<UTC ISO-8601 ms> kind=STATS|EVENT key=value`. Values are redacted and
//! every whitespace character becomes `_`, so each value stays one token.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::redact::redact_key_shaped;

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const KEEP: usize = 5;

/// The stats sample interval while peer rx is fresh, and while it is flat.
const STATS_INTERVAL: Duration = Duration::from_secs(5);
const STATS_INTERVAL_FLAT: Duration = Duration::from_secs(1);
/// Peer rx older than this switches stats to the flat cadence.
const PEER_RX_FLAT: Duration = Duration::from_secs(3);

/// `diag.log` -> `diag.1.log`, `diag.2.log`, ...
pub fn rotated_path(path: &Path, n: usize) -> PathBuf {
    match path.extension() {
        Some(ext) => {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let ext = ext.to_string_lossy();
            path.with_file_name(format!("{stem}.{n}.{ext}"))
        }
        None => {
            let stem = path.file_name().unwrap_or_default().to_string_lossy();
            path.with_file_name(format!("{stem}.{n}"))
        }
    }
}

/// `2026-10-31T12:34:56.789Z` from UTC epoch milliseconds.
pub fn format_utc_iso_ms(epoch_ms: i64) -> String {
    let secs = epoch_ms.div_euclid(1_000);
    let millis = epoch_ms.rem_euclid(1_000);
    let (year, month, day, hour, minute, second) = utc_parts(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// One journal line, without a trailing newline:
/// `ts=<UTC ms> kind=<kind> <key>=<redacted value> ...`.
pub fn format_line(epoch_ms: i64, kind: &str, fields: &[(&str, &str)]) -> String {
    let mut line = format!("ts={} kind={kind}", format_utc_iso_ms(epoch_ms));
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&sanitize(value));
    }
    line
}

/// Redact key-shaped base64, then keep every value one whitespace-free token.
fn sanitize(value: &str) -> String {
    redact_key_shaped(value)
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

/// True when a stats sample is due: every 5 s while peer rx is fresh, every
/// 1 s while peer rx is older than 3 s or has never been seen.
pub fn stats_due(now: Instant, last_stats: Option<Instant>, last_peer_rx: Option<Instant>) -> bool {
    let Some(last) = last_stats else {
        return true;
    };
    let peer_is_flat = last_peer_rx
        .map(|rx| now.saturating_duration_since(rx) > PEER_RX_FLAT)
        .unwrap_or(true);
    let interval = if peer_is_flat {
        STATS_INTERVAL_FLAT
    } else {
        STATS_INTERVAL
    };
    now.saturating_duration_since(last) >= interval
}

fn utc_parts(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    (year, month, day, hour as u32, minute as u32, second as u32)
}

/// Howard Hinnant's civil-from-days, the same integer conversion as `applog`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}
