//! The drop file's format constants. The lines themselves are the core drop
//! history lines; the platform keeps the private file.

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = "drops.tsv";
/// The file keeps the newest lines only.
pub const MAX_LINES: usize = 1000;
