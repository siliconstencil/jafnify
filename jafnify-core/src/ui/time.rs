//! The text clock, shared by the app log and the user-visible UI texts.
//!
//! Lines written to disk stay UTC. The local form is only for text the user
//! sees; it follows the machine's time zone and, where the platform exposes
//! it, the locale's 12/24-hour clock. Pure: no I/O and no global state.

use std::time::{SystemTime, UNIX_EPOCH};

/// A date and time with milliseconds, used by the platform clock adapters.
pub type DateTimeParts = (i64, u32, u32, u32, u32, u32, u32);

/// `2026-10-28T22:37:36Z`.
pub fn format_utc(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = utc_parts(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `2026-10-28 22:37:36 UTC` from UTC epoch milliseconds, for stored files.
pub fn format_utc_millis(epoch_ms: i64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(epoch_ms.div_euclid(1_000));
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// `2026-10-28 15:37:36 PDT` from epoch milliseconds, in the machine's local
/// time zone and with its abbreviation. The stored app and diagnostic logs
/// stay UTC; this is only for text the user sees.
/// The clock follows the locale: `2026-10-28 3:37:36 PM PDT` where the
/// locale uses a 12-hour clock (en_US), 24-hour otherwise.
pub fn format_local_millis(epoch_ms: i64) -> String {
    match local_zone(epoch_ms) {
        Some((offset_secs, abbrev)) => {
            format_millis_with_clock(epoch_ms, offset_secs, &abbrev, locale_twelve_hour())
        }
        None => format_utc_millis(epoch_ms),
    }
}

/// The pure 24-hour part of [`format_local_millis`], for tests and reuse.
pub fn format_millis_with_zone(epoch_ms: i64, offset_secs: i64, abbrev: &str) -> String {
    format_millis_with_clock(epoch_ms, offset_secs, abbrev, false)
}

/// [`format_millis_with_zone`] with the clock chosen: `twelve_hour` gives
/// `3:37:36 PM` (and `12:05:00 AM` just after midnight).
pub fn format_millis_with_clock(
    epoch_ms: i64,
    offset_secs: i64,
    abbrev: &str,
    twelve_hour: bool,
) -> String {
    let secs = epoch_ms.div_euclid(1_000).saturating_add(offset_secs);
    let (year, month, day, hour, minute, second) = utc_parts(secs);
    if twelve_hour {
        let suffix = if hour < 12 { "AM" } else { "PM" };
        let hour12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{year:04}-{month:02}-{day:02} {hour12}:{minute:02}:{second:02} {suffix} {abbrev}")
    } else {
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {abbrev}")
    }
}

/// Whether a locale time format (`nl_langinfo(T_FMT)`) is a 12-hour clock.
pub fn time_format_is_twelve_hour(t_fmt: &str) -> bool {
    ["%I", "%l", "%r", "%p", "%P"]
        .iter()
        .any(|code| t_fmt.contains(code))
}

/// Whether a Windows `LOCALE_STIMEFORMAT` picture string is a 12-hour clock.
/// Windows uses `h`/`hh` for 1-12 and `H`/`HH` for 0-23, plus `t`/`tt` for
/// the AM/PM marker.
pub fn windows_style_is_twelve_hour(style: &str) -> bool {
    let mut quoted = false;
    for c in style.chars() {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if !quoted && (c == 'h' || c == 't') {
            return true;
        }
    }
    false
}

/// The clock of the user's time locale (LC_ALL, LC_TIME, LANG), read once.
/// Read without `setlocale`, so the process locale is left alone.
pub fn locale_twelve_hour() -> bool {
    static TWELVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TWELVE.get_or_init(platform_twelve_hour)
}

#[cfg(target_os = "linux")]
fn platform_twelve_hour() -> bool {
    locale_time_format().is_some_and(|f| time_format_is_twelve_hour(&f))
}

#[cfg(windows)]
fn platform_twelve_hour() -> bool {
    windows_time_format().is_some_and(|f| windows_style_is_twelve_hour(&f))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_twelve_hour() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn locale_time_format() -> Option<String> {
    // SAFETY: newlocale with "" reads the locale from the environment and
    // returns a new handle or null; the string from nl_langinfo_l belongs to
    // that handle and is copied before the handle is freed.
    unsafe {
        let loc = libc::newlocale(libc::LC_TIME_MASK, c"".as_ptr(), std::ptr::null_mut());
        if loc.is_null() {
            return None;
        }
        let ptr = libc::nl_langinfo_l(libc::T_FMT, loc);
        let format = if ptr.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
        };
        libc::freelocale(loc);
        format
    }
}

#[cfg(windows)]
fn windows_time_format() -> Option<String> {
    // SAFETY: both calls pass a null locale name (the user default) and a
    // null output only when asking for the length. The second call fills a
    // buffer of exactly that many UTF-16 units.
    unsafe {
        let len = windows_sys::Win32::Globalization::GetLocaleInfoEx(
            std::ptr::null(),
            windows_sys::Win32::Globalization::LOCALE_STIMEFORMAT,
            std::ptr::null_mut(),
            0,
        );
        if len <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize];
        let read = windows_sys::Win32::Globalization::GetLocaleInfoEx(
            std::ptr::null(),
            windows_sys::Win32::Globalization::LOCALE_STIMEFORMAT,
            buffer.as_mut_ptr(),
            len,
        );
        if read <= 0 {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

/// The local UTC offset in minutes at this instant; 0 when unknown.
pub fn local_offset_minutes(epoch_ms: i64) -> i32 {
    local_zone(epoch_ms)
        .map(|(seconds, _)| (seconds / 60) as i32)
        .unwrap_or(0)
}

fn local_zone(epoch_ms: i64) -> Option<(i64, String)> {
    #[cfg(unix)]
    {
        let secs = epoch_ms.div_euclid(1_000) as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let out = unsafe { libc::localtime_r(&secs, &mut tm) };
        if out.is_null() {
            return None;
        }
        let offset = tm.tm_gmtoff as i64;
        let abbrev = unsafe {
            if tm.tm_zone.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(tm.tm_zone)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let abbrev = if abbrev.is_empty() {
            format_offset(offset)
        } else {
            abbrev
        };
        Some((offset, abbrev))
    }
    #[cfg(windows)]
    {
        windows_local_zone(epoch_ms)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = epoch_ms;
        None
    }
}

#[cfg(windows)]
fn windows_local_zone(epoch_ms: i64) -> Option<(i64, String)> {
    use windows_sys::Win32::System::Time::{
        GetTimeZoneInformation, SystemTimeToTzSpecificLocalTime, TIME_ZONE_ID_INVALID,
    };

    // SAFETY: all-zero is a valid initial TIME_ZONE_INFORMATION.
    let mut info = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a live, correctly sized TIME_ZONE_INFORMATION.
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    if id == TIME_ZONE_ID_INVALID {
        return None;
    }
    let utc = epoch_parts_ms(epoch_ms);
    if !(1601..=9999).contains(&utc.0) {
        return None;
    }
    let utc_system = parts_to_systemtime(utc);
    // SAFETY: all-zero is a valid initial SYSTEMTIME.
    let mut local_system = unsafe { std::mem::zeroed() };
    // SAFETY: both SYSTEMTIME values are live; `info` was filled above.
    let converted =
        unsafe { SystemTimeToTzSpecificLocalTime(&info, &utc_system, &mut local_system) };
    if converted == 0 {
        return None;
    }
    let local = systemtime_parts(&local_system);
    let offset = offset_seconds_from_parts(utc, local)?;
    let bias = info.Bias.saturating_add(info.DaylightBias);
    Some((
        offset,
        zone_abbrev(&info.StandardName, &info.DaylightName, bias, offset),
    ))
}

/// The UTC offset in seconds from the Windows bias in minutes (`UTC = local +
/// bias`, so the offset is the negated bias).
pub fn offset_seconds_from_bias(bias_minutes: i32) -> i64 {
    -(bias_minutes as i64) * 60
}

/// The zone abbreviation for a local offset. A zone without daylight saving
/// has `daylight_bias_minutes == 0` and often an empty daylight name; the
/// standard name wins unless a non-empty daylight name really matches the
/// daylight offset.
pub fn zone_abbrev(
    standard_name: &[u16],
    daylight_name: &[u16],
    daylight_bias_minutes: i32,
    offset_secs: i64,
) -> String {
    let standard = wide_name_to_string(standard_name);
    let daylight = wide_name_to_string(daylight_name);
    let use_daylight = daylight_bias_minutes != 0
        && offset_secs == offset_seconds_from_bias(daylight_bias_minutes)
        && !daylight.is_empty();
    let name = if use_daylight { daylight } else { standard };
    let abbrev = abbrev_from_zone_name(&name);
    if abbrev.is_empty() {
        format_offset(offset_secs)
    } else {
        abbrev
    }
}

/// `Pacific Standard Time` -> `PST`, `Coordinated Universal Time` -> `CUT`.
/// A single-word name is kept unchanged.
pub fn abbrev_from_zone_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() <= 1 {
        return name.trim().to_string();
    }
    let mut out = String::new();
    for word in words {
        if let Some(c) = word.chars().find(|c| c.is_ascii_alphabetic()) {
            out.push(c.to_ascii_uppercase());
        }
    }
    if out.is_empty() {
        name.trim().to_string()
    } else {
        out
    }
}

/// A NUL-terminated UTF-16 name (as the Windows time-zone structs carry it).
pub fn wide_name_to_string(name: &[u16]) -> String {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end])
}

/// Epoch milliseconds broken into UTC fields, milliseconds included.
pub fn epoch_parts_ms(epoch_ms: i64) -> DateTimeParts {
    let secs = epoch_ms.div_euclid(1_000);
    let (year, month, day, hour, minute, second) = utc_parts(secs);
    (
        year,
        month,
        day,
        hour,
        minute,
        second,
        epoch_ms.rem_euclid(1_000) as u32,
    )
}

/// The inverse of [`epoch_parts_ms`]. Out-of-range fields are rejected.
pub fn epoch_ms_from_parts(parts: DateTimeParts) -> Option<i64> {
    let (year, month, day, hour, minute, second, millis) = parts;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
        || millis > 999
    {
        return None;
    }
    let days = days_from_civil(year, month as i32, day as i32);
    let seconds = hour as i64 * 3_600 + minute as i64 * 60 + second as i64;
    Some(days * 86_400_000 + seconds * 1_000 + millis as i64)
}

/// The offset of a local time from the matching UTC time, in seconds.
pub fn offset_seconds_from_parts(utc: DateTimeParts, local: DateTimeParts) -> Option<i64> {
    let utc_ms = epoch_ms_from_parts(utc)?;
    let local_ms = epoch_ms_from_parts(local)?;
    Some((local_ms - utc_ms).div_euclid(1_000))
}

#[cfg(windows)]
fn parts_to_systemtime(parts: DateTimeParts) -> windows_sys::Win32::Foundation::SYSTEMTIME {
    let (year, month, day, hour, minute, second, millis) = parts;
    windows_sys::Win32::Foundation::SYSTEMTIME {
        wYear: year as u16,
        wMonth: month as u16,
        wDayOfWeek: 0,
        wDay: day as u16,
        wHour: hour as u16,
        wMinute: minute as u16,
        wSecond: second as u16,
        wMilliseconds: millis as u16,
    }
}

#[cfg(windows)]
fn systemtime_parts(value: &windows_sys::Win32::Foundation::SYSTEMTIME) -> DateTimeParts {
    (
        value.wYear as i64,
        value.wMonth as u32,
        value.wDay as u32,
        value.wHour as u32,
        value.wMinute as u32,
        value.wSecond as u32,
        value.wMilliseconds as u32,
    )
}

fn format_offset(offset_secs: i64) -> String {
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let total = offset_secs.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", total / 60, total % 60)
}

/// `20261028-223736` from UTC epoch milliseconds, for a file name.
pub fn format_utc_stamp(epoch_ms: i64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(epoch_ms.div_euclid(1_000));
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
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

/// Howard Hinnant's days-from-civil, the inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: i32, day: i32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i64;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_stamp_and_millis_are_stable() {
        assert_eq!(format_utc_millis(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc_stamp(0), "19700101-000000");
        assert_eq!(
            format_millis_with_zone(1_700_000_000_000, -8 * 3_600, "PST"),
            "2023-11-14 14:13:20 PST"
        );
    }

    #[test]
    fn the_epoch_fields_round_trip_through_the_inverse() {
        for epoch_ms in [0, 1_700_000_000_000, 1_794_201_528_789] {
            let parts = epoch_parts_ms(epoch_ms);
            assert_eq!(epoch_ms_from_parts(parts), Some(epoch_ms));
        }
        let (year, month, day, _, _, _, millis) = epoch_parts_ms(1_700_000_000_123);
        assert_eq!((year, month, day, millis), (2023, 11, 14, 123));
    }

    #[test]
    fn the_windows_bias_is_negated_into_the_utc_offset() {
        // Recorded from the Windows TIME_ZONE_INFORMATION contract: BIAS is
        // in minutes and UTC = local + BIAS.
        assert_eq!(offset_seconds_from_bias(-480), 28_800);
        assert_eq!(offset_seconds_from_bias(300), -18_000);
        assert_eq!(offset_seconds_from_bias(0), 0);
    }

    #[test]
    fn a_windows_picture_string_names_the_clock() {
        // Recorded from the Windows docs LOCALE_STIMEFORMAT examples: en-US
        // "h:mm:ss tt", de-DE "HH:mm:ss", some locales "h:mm:ss".
        assert!(windows_style_is_twelve_hour("h:mm:ss tt"));
        assert!(windows_style_is_twelve_hour("hh:mm:ss t"));
        assert!(!windows_style_is_twelve_hour("HH:mm:ss"));
        assert!(!windows_style_is_twelve_hour("H:mm"));
        assert!(!windows_style_is_twelve_hour("HH:mm 't'"));
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[test]
    fn a_zone_without_daylight_keeps_its_standard_name() {
        let pst = wide("Pacific Standard Time");
        let pdt = wide("Pacific Daylight Time");
        assert_eq!(zone_abbrev(&pst, &pdt, -420, 25_200), "PDT");
        assert_eq!(zone_abbrev(&pst, &pdt, -420, 28_800), "PST");
        let china = wide("China Standard Time");
        assert_eq!(zone_abbrev(&china, &[], 0, 28_800), "CST");
    }

    #[test]
    fn a_zone_name_becomes_its_capitalised_initials() {
        // Recorded from the Windows docs time-zone names.
        assert_eq!(abbrev_from_zone_name("Pacific Standard Time"), "PST");
        assert_eq!(
            abbrev_from_zone_name("Central European Summer Time"),
            "CEST"
        );
        assert_eq!(abbrev_from_zone_name("UTC"), "UTC");
        assert_eq!(
            wide_name_to_string(&[b'P' as u16, b'S' as u16, b'T' as u16, 0]),
            "PST"
        );
    }

    #[test]
    fn a_local_minus_utc_pair_gives_the_offset() {
        let utc = epoch_parts_ms(1_794_201_528_000);
        let local = epoch_parts_ms(1_794_201_528_000 - 28_800_000);
        assert_eq!(offset_seconds_from_parts(utc, local), Some(-28_800));
    }
}
