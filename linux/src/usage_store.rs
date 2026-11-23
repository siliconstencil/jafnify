//! The laptop's hourly usage buckets, in the core line format.
//!
//! The file is local, private and never read by the diagnostics export. A
//! missing file is empty; a corrupt line is skipped. Writes are atomic with a
//! private directory and a 0600 file, the same pattern as the session store.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use jafnify_core::usage::{
    buckets_from_lines, push_bucket, UsageBucket, UsageSummary, UsageWindow, MAX_BUCKETS,
};

pub struct UsageStore {
    path: PathBuf,
    buckets: Vec<UsageBucket>,
}

impl UsageStore {
    /// Read the store. A missing or corrupt file reads as empty.
    pub fn load(path: &Path) -> Self {
        let buckets = match fs::read_to_string(path) {
            Ok(text) => buckets_from_lines(&text),
            Err(_) => Vec::new(),
        };
        UsageStore {
            path: path.to_path_buf(),
            buckets,
        }
    }

    pub fn buckets(&self) -> &[UsageBucket] {
        &self.buckets
    }

    /// Add one completed hour, merge a duplicate hour, cap, and save.
    pub fn push(&mut self, bucket: UsageBucket) {
        push_bucket(&mut self.buckets, bucket);
        let _ = self.save();
    }

    /// Aggregate the stored buckets for one window. The current partial hour is
    /// added by the caller before calling this.
    pub fn summary(
        &self,
        window: UsageWindow,
        now_ms: i64,
        utc_offset_minutes: i32,
    ) -> UsageSummary {
        UsageSummary::build(&self.buckets, window, now_ms, utc_offset_minutes)
    }

    /// Write every line atomically, 0600 in a private directory.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            crate::settings::ensure_private_dir(parent)?;
        }
        let mut text = String::new();
        for bucket in &self.buckets {
            text.push_str(&bucket.line());
            text.push('\n');
        }
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

    /// The cap, exposed for tests and the report.
    pub fn max_buckets() -> usize {
        MAX_BUCKETS
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "usage.tsv".to_string());
    path.with_file_name(format!(".{name}.tmp"))
}
