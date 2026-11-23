//! The bounded connection log shown in Advanced and copied/saved by the user.
//!
//! The ring, the line templates and the redaction rules live in
//! `jafnify_core::connection_log`; this module owns the optional private file.
//! A persistent log appends each line to the file and loads the newest lines
//! at startup, so earlier runs stay visible. The file is compacted to the
//! newest [`MAX_LINES`] lines once it holds twice as many.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use jafnify_core::connection_log as core_log;
use jafnify_core::line_file;

pub use jafnify_core::connection_log::{
    drop_line, drop_line_with_hint, network_back_line, privileged_line, reconnected_line,
    session_ended_line, session_resumed_line, session_started_line, state_line, EARLIER_KIND,
    MAX_LINES, NO_NETWORK, SUPPRESS_RESTORED,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionLog {
    entries: core_log::ConnectionLog,
    /// The backing file of a persistent log.
    file: Option<PathBuf>,
    /// Lines in the backing file (to know when to compact it).
    file_lines: usize,
}

impl ConnectionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// A log backed by `path`: the newest [`MAX_LINES`] lines of the file are
    /// loaded now, and every new line is appended to it. A missing or
    /// unreadable file starts an empty log; file errors never fail a push.
    pub fn persistent(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Self {
            file_lines: line_file::count_lines(&text),
            entries: core_log::ConnectionLog::from_text(&text),
            file: Some(path),
        }
    }

    /// Append one event. `kind` is metadata for callers; `text` is the line
    /// after the timestamp. Key-shaped text is redacted before it is stored.
    pub fn push(&mut self, kind: &str, text: &str, now_ms: i64) {
        let line = self.entries.push(kind, text, now_ms);
        self.append_to_file(&line);
    }

    /// Best effort: a log that cannot be written still works in memory.
    fn append_to_file(&mut self, line: &str) {
        let Some(path) = self.file.clone() else {
            return;
        };
        if line_file::should_compact(self.file_lines, MAX_LINES) {
            // Keep the newest lines and start over.
            let body = self.entries.rewrite_body();
            if write_private_atomic(&path, &body).is_ok() {
                self.file_lines = self.entries.len();
            }
            return;
        }
        let appended = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()));
        if appended.is_ok() {
            self.file_lines += 1;
        }
    }

    /// The last `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        self.entries.tail(n)
    }

    /// The same lines as [`tail`], joined with `\n` and no trailing newline.
    /// The text leaves the app here, so it is redacted on the way out.
    pub fn copy_text(&self, n: usize) -> String {
        self.entries.copy_text(n)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The kind of the newest entry (useful to tests and future filters).
    pub fn last_kind(&self) -> Option<&str> {
        self.entries.last_kind()
    }
}

/// Replace `path` with `body` (mode 0600) through a temporary file and a
/// rename, so a crash never leaves a half-written log.
fn write_private_atomic(path: &Path, body: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut file = jafnify_core::private_fs::open_private_file(&tmp)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Write `text` to a new private log file in `dir`, named
/// `jafnify-log-YYYYMMDD-HHMMSS.txt`. `create_new` makes the write exclude an
/// existing file or symlink; a second save in the same second gets `-1`, `-2`.
///
/// The text is redacted on the way out, exactly like [`ConnectionLog::
/// copy_text`]: the file holds what the clipboard would hold.
pub fn save_log_at(dir: &Path, text: &str, now_ms: i64) -> io::Result<PathBuf> {
    let text = crate::export::redact(text);
    let stamp = jafnify_core::ui::time::format_utc_stamp(now_ms);
    let mut index = 0_u32;
    loop {
        let name = if index == 0 {
            format!("jafnify-log-{stamp}.txt")
        } else {
            format!("jafnify-log-{stamp}-{index}.txt")
        };
        let path = dir.join(name);
        match jafnify_core::private_fs::open_private_file(&path) {
            Ok(mut file) => {
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                file.write_all(text.as_bytes())?;
                file.flush()?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                index = index.saturating_add(1);
            }
            Err(e) => return Err(e),
        }
    }
}
