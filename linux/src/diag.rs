//! The diagnostic journal (flight recorder): `diag.log` plus four rotations.
//!
//! One record is one line: `ts=<UTC ISO-8601 ms> kind=STATS|EVENT key=value`.
//! The line format, the rotation naming and the sample cadence live in
//! `jafnify_core::diag`; this module only owns the file handle.
//!
//! The journal is diagnostics only: nothing here reads a clock, touches the
//! network, or changes connection behaviour.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub use jafnify_core::diag::{
    format_line, format_utc_iso_ms, rotated_path, stats_due, KEEP, MAX_BYTES,
};

pub struct Diag {
    path: PathBuf,
    file: File,
    size: u64,
}

impl Diag {
    /// Open `path` for appending, creating parent directories (0700) first.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            crate::settings::ensure_private_dir(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(Self { path, file, size })
    }

    /// Append one `kind=EVENT event=<name>` record.
    pub fn event(&mut self, epoch_ms: i64, name: &str, fields: &[(&str, &str)]) -> io::Result<()> {
        let mut all = Vec::with_capacity(fields.len() + 1);
        all.push(("event", name));
        all.extend_from_slice(fields);
        self.write_record(&format_line(epoch_ms, "EVENT", &all))
    }

    /// Append one `kind=STATS` record.
    pub fn stats(&mut self, epoch_ms: i64, fields: &[(&str, &str)]) -> io::Result<()> {
        self.write_record(&format_line(epoch_ms, "STATS", fields))
    }

    /// Rotate first when the next line would exceed [`MAX_BYTES`], then append
    /// the line and flush it. A failed write leaves a short file but never a
    /// panic; the caller decides whether to keep the journal.
    fn write_record(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.len() as u64 + 1;
        if self.size.saturating_add(bytes) > MAX_BYTES {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        self.size = self.size.saturating_add(bytes);
        Ok(())
    }

    /// active -> `.1`, `.1` -> `.2`, ..., `.3` -> `.4`; remove the old `.4`.
    fn rotate(&mut self) -> io::Result<()> {
        let _ = self.file.flush();

        let oldest = rotated_path(&self.path, KEEP - 1);
        match fs::remove_file(&oldest) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        for n in (1..KEEP - 1).rev() {
            let from = rotated_path(&self.path, n);
            let to = rotated_path(&self.path, n + 1);
            match fs::rename(&from, &to) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        fs::rename(&self.path, rotated_path(&self.path, 1))?;

        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        self.size = 0;
        Ok(())
    }
}
