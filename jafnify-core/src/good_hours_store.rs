//! The good-hours file's format constants. The hour lines and the
//! one-line-per-hour rule live in `good_hours`; the platform keeps the file.

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = "good_hours.tsv";
/// The file keeps the newest lines only: 14 days of hours and one day more.
pub const MAX_LINES: usize = 14 * 24 + 24;
