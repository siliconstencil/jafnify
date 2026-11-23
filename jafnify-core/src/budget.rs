//! The monthly mobile data budget. The caller supplies the local date; this
//! module never reads a clock. The record is one line in the phone's store.

use std::error::Error;
use std::fmt;

/// The warning threshold, in percent of the allowance.
pub const WARN_PCT: u128 = 80;
/// The fixed warning line, also used by the phone notification.
pub const WARNING_LINE: &str = "80% of your mobile data allowance used.";
/// The line shown before an allowance is entered.
pub const SET_ALLOWANCE_LINE: &str = "Set your monthly allowance.";

/// The one monthly budget record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BudgetRecord {
    pub allow_bytes: u64,
    pub used_bytes: u64,
    pub reset_day: u8,
    pub month_key: u32,
    pub warned_month: u32,
}

/// A rejected budget line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetParseError {
    BadPrefix,
    BadVersion,
    Malformed,
}

impl fmt::Display for BudgetParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BudgetParseError::BadPrefix => "bad budget prefix",
            BudgetParseError::BadVersion => "bad budget version",
            BudgetParseError::Malformed => "malformed budget line",
        })
    }
}

impl Error for BudgetParseError {}

impl BudgetRecord {
    /// Roll to `year`/`month` and clamp the reset day. Returns true when the
    /// month key changed and the used/warned values were reset.
    pub fn prepare(&mut self, year: i32, month: u32, reset_day: u8) -> bool {
        let current = month_key(year, month);
        let changed = self.month_key != current;
        if changed {
            self.used_bytes = 0;
            self.warned_month = 0;
            self.month_key = current;
        }
        self.reset_day = clamp_reset_day(reset_day, year, month);
        changed
    }

    /// Whether this month still needs the one warning.
    pub fn warning_due(&self) -> bool {
        warning_for(self.used_bytes, self.allow_bytes).is_some()
            && self.warned_month != self.month_key
    }

    /// The fixed warning line when it is due this month.
    pub fn warning_line(&self) -> Option<&'static str> {
        if self.warning_due() {
            Some(WARNING_LINE)
        } else {
            None
        }
    }

    /// The read-only budget line for either screen.
    pub fn display_line(&self) -> String {
        display_for(self.used_bytes, self.allow_bytes)
    }

    /// The one-line phone record: JFB1 1 allow=... used=... reset=... month=...
    /// warned=...
    pub fn line(&self) -> String {
        format!(
            "JFB1 1 allow={} used={} reset={} month={} warned={}",
            self.allow_bytes, self.used_bytes, self.reset_day, self.month_key, self.warned_month
        )
    }

    /// Parse the one-line phone record. Unknown keys are ignored; allow,
    /// used, reset and month are required; warned is optional and reads 0.
    pub fn parse_line(text: &str) -> Result<Self, BudgetParseError> {
        let mut parts = text.split_whitespace();
        if parts.next() != Some("JFB1") {
            return Err(BudgetParseError::BadPrefix);
        }
        if parts.next() != Some("1") {
            return Err(BudgetParseError::BadVersion);
        }
        let mut record = BudgetRecord::default();
        let mut seen_allow = false;
        let mut seen_used = false;
        let mut seen_reset = false;
        let mut seen_month = false;
        for part in parts {
            let (key, value) = part.split_once('=').ok_or(BudgetParseError::Malformed)?;
            if key.is_empty() || value.is_empty() {
                return Err(BudgetParseError::Malformed);
            }
            match key {
                "allow" => {
                    if seen_allow {
                        return Err(BudgetParseError::Malformed);
                    }
                    record.allow_bytes = value.parse().map_err(|_| BudgetParseError::Malformed)?;
                    seen_allow = true;
                }
                "used" => {
                    if seen_used {
                        return Err(BudgetParseError::Malformed);
                    }
                    record.used_bytes = value.parse().map_err(|_| BudgetParseError::Malformed)?;
                    seen_used = true;
                }
                "reset" => {
                    if seen_reset {
                        return Err(BudgetParseError::Malformed);
                    }
                    let day: u8 = value.parse().map_err(|_| BudgetParseError::Malformed)?;
                    if day > 31 {
                        return Err(BudgetParseError::Malformed);
                    }
                    record.reset_day = day;
                    seen_reset = true;
                }
                "month" => {
                    if seen_month {
                        return Err(BudgetParseError::Malformed);
                    }
                    record.month_key = value.parse().map_err(|_| BudgetParseError::Malformed)?;
                    seen_month = true;
                }
                "warned" => {
                    record.warned_month = value.parse().map_err(|_| BudgetParseError::Malformed)?;
                }
                _ => {}
            }
        }
        if !(seen_allow && seen_used && seen_reset && seen_month) {
            return Err(BudgetParseError::Malformed);
        }
        Ok(record)
    }
}

/// The M0-style month key: year * 12 + month, saturating at u32.
pub fn month_key(year: i32, month: u32) -> u32 {
    (year.max(0) as u32)
        .saturating_mul(12)
        .saturating_add(month)
}

/// The number of days in a Gregorian month.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
            if leap {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// A user reset day 1-31 clamped to the month's last day.
pub fn clamp_reset_day(reset_day: u8, year: i32, month: u32) -> u8 {
    let last = days_in_month(year, month).min(u32::from(u8::MAX)) as u8;
    reset_day.clamp(1, last)
}

/// The 80 percent rule without the once-per-month bookkeeping, for a reader
/// that only has the two totals.
pub fn warning_for(used_bytes: u64, allow_bytes: u64) -> Option<&'static str> {
    if allow_bytes == 0 {
        return None;
    }
    if u128::from(used_bytes) * 100 >= u128::from(allow_bytes) * WARN_PCT {
        Some(WARNING_LINE)
    } else {
        None
    }
}

/// The read-only budget line from the two totals.
pub fn display_for(used_bytes: u64, allow_bytes: u64) -> String {
    if allow_bytes == 0 {
        SET_ALLOWANCE_LINE.to_string()
    } else {
        format!(
            "Mobile data: {} of {} this month.",
            format_gb(used_bytes),
            format_gb(allow_bytes)
        )
    }
}

/// Decimal GB with no decimal for whole values and one decimal otherwise.
pub fn format_gb(bytes: u64) -> String {
    let tenths = (u128::from(bytes) * 10 / 1_000_000_000) as u64;
    if tenths.is_multiple_of(10) {
        format!("{} GB", tenths / 10)
    } else {
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}
