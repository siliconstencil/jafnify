//! The private good-hours file of the laptop.
//!
//! The file holds one line per finished hour: how many samples the hour had,
//! the median DNS answer time, the share of the samples that were on 5G and
//! the median signal level. The daemon is the only writer, so the file cannot
//! race itself. It is local and private (0600) and holds no location, no
//! address, no endpoint and no peer id.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use jafnify_core::good_hours::{self, HourRecord};
use jafnify_core::line_file;

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = jafnify_core::good_hours_store::FILE_NAME;
/// The file keeps the newest lines only: 14 days of hours and one day more.
pub use jafnify_core::good_hours_store::MAX_LINES;

/// The lines of the good-hours file, oldest first.
pub struct GoodHoursStore {
    path: PathBuf,
    lines: Vec<String>,
    /// How many lines the file holds; the file is compacted when it grows
    /// past twice [`MAX_LINES`].
    file_lines: usize,
}

impl GoodHoursStore {
    /// Read the store. A missing or unreadable file reads as empty; a
    /// corrupt line is skipped.
    pub fn load(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap_or_default();
        let file_lines = line_file::count_lines(&text);
        let lines = line_file::load_lines(&text, MAX_LINES);
        Self {
            path: path.to_path_buf(),
            lines,
            file_lines,
        }
    }

    /// The lines of the file, oldest first.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The hours of the file, oldest first. A line that does not parse is
    /// skipped: a damaged file must not stop the learning.
    pub fn records(&self) -> Vec<HourRecord> {
        self.lines
            .iter()
            .filter_map(|line| HourRecord::parse_line(line).ok())
            .collect()
    }

    /// Append one line. The file keeps the newest [`MAX_LINES`] lines.
    ///
    /// One line per hour: the record of an hour that the file already has
    /// takes the place of the old line, so a hour that is written twice does
    /// not grow the file.
    ///
    /// A write that fails is reported to the caller: the line stays in
    /// memory and the next compaction writes it to the file.
    pub fn append(&mut self, line: String) -> std::io::Result<()> {
        if let Some(index) = good_hours::line_of_hour(&self.lines, &line) {
            self.lines[index] = line;
            self.save()?;
            self.file_lines = self.lines.len();
            return Ok(());
        }
        line_file::push_capped(&mut self.lines, line.clone(), MAX_LINES);
        if line_file::should_compact(self.file_lines, MAX_LINES) {
            // Keep the newest lines and start over.
            self.save()?;
            self.file_lines = self.lines.len();
            return Ok(());
        }
        // The file lives in a private directory; it is created here, so the
        // first hour can be written on a fresh install.
        if let Some(parent) = self.path.parent() {
            crate::settings::ensure_private_dir(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(format!("{line}\n").as_bytes())?;
        self.file_lines += 1;
        Ok(())
    }

    /// Delete the file and forget the lines.
    pub fn clear(&mut self) {
        let _ = fs::remove_file(&self.path);
        self.lines.clear();
        self.file_lines = 0;
    }

    /// Write every line atomically, 0600 in a private directory.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            crate::settings::ensure_private_dir(parent)?;
        }
        let text = line_file::body(&self.lines);
        let tmp = tmp_path(&self.path);
        match fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        {
            let mut file = jafnify_core::private_fs::open_private_file(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        Ok(())
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| FILE_NAME.to_string());
    path.with_file_name(format!(".{name}.tmp"))
}
