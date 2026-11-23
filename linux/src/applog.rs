//! The app event log: `~/.local/state/jafnify/jafnify.log` plus stderr.
//!
//! Lines are UTC, one event each, and never contain a key or a per-second
//! counter. Rotation happens before the file would grow past 5 MiB, also
//! while the logger is running; three files are kept (`jafnify.log`, `.1`,
//! `.2`).
//!
//! W2a moved the pure text clock into `jafnify_core::ui::time`; the names are
//! re-exported here so existing callers and tests keep their path.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use jafnify_core::ui::time::{
    format_local_millis, format_millis_with_clock, format_millis_with_zone, format_utc,
    format_utc_millis, format_utc_stamp, local_offset_minutes, locale_twelve_hour,
    time_format_is_twelve_hour,
};

pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const KEEP: usize = 3;

pub struct Logger {
    path: PathBuf,
    file: File,
    written: u64,
    stderr: bool,
}

impl Logger {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        rotate_if_needed(&path)?;
        let file = open_append(&path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written,
            stderr: true,
        })
    }

    /// Tests can keep the file only; production keeps both.
    pub fn without_stderr(mut self) -> Self {
        self.stderr = false;
        self
    }

    pub fn event(&mut self, level: &str, message: &str) {
        let line = format!(
            "{} {} {}\n",
            format_utc(SystemTime::now()),
            level,
            redact(message)
        );
        if self.written.saturating_add(line.len() as u64) > MAX_BYTES {
            self.rotate();
        }
        let _ = self.file.write_all(line.as_bytes());
        self.written = self.written.saturating_add(line.len() as u64);
        let _ = self.file.flush();
        if self.stderr {
            eprint!("{line}");
        }
    }

    fn rotate(&mut self) {
        let _ = self.file.flush();
        if rotate_files(&self.path).is_ok() {
            if let Ok(file) = open_append(&self.path) {
                self.file = file;
                self.written = 0;
            }
        }
    }

    pub fn flush(&mut self) {
        let _ = self.file.flush();
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn default_path() -> PathBuf {
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(crate::flavor::NAME).join("jafnify.log")
}

/// The diagnostic journal beside the app log, or None in a test process. The
/// same guard as [`open_default_unless_test`] keeps tests off the operator's
/// real state directory.
pub fn diag_path_unless_test() -> Option<PathBuf> {
    if std::env::var_os(crate::root::TEST_NO_ROOT_ENV).is_some() {
        return None;
    }
    Some(default_path().with_file_name("diag.log"))
}

/// The production logger. Test processes set `JAFNIFY_TEST_NO_ROOT`; they must
/// never write into the operator's real state directory.
pub fn open_default_unless_test() -> Option<Logger> {
    if std::env::var_os("JAFNIFY_TEST_NO_ROOT").is_some() {
        return None;
    }
    match Logger::open(default_path()) {
        Ok(log) => Some(log),
        Err(e) => {
            eprintln!("warning: cannot write the app log: {e}");
            None
        }
    }
}

/// Keys are removed whole, with no prefix left behind. The rule lives in the
/// core so every platform redacts log lines the same way.
pub fn redact(text: &str) -> String {
    jafnify_core::redact::redact_key_shaped(text)
}

fn ensure_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if path.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

fn rotate_if_needed(path: &Path) -> io::Result<()> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size < MAX_BYTES {
        return Ok(());
    }
    rotate_files(path)
}

fn rotate_files(path: &Path) -> io::Result<()> {
    let rotated = |n: usize| {
        let mut p = path.as_os_str().to_owned();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    };
    let _ = std::fs::remove_file(rotated(KEEP - 1));
    for n in (1..KEEP - 1).rev() {
        let from = rotated(n);
        if from.exists() {
            let _ = std::fs::rename(&from, rotated(n + 1));
        }
    }
    std::fs::rename(path, rotated(1))
}
