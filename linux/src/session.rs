//! The completed-session store. The tracker and the record shapes live in
//! `jafnify_core::session`; this module owns the private TOML file.
//!
//! [`SessionHistory`] keeps at most [`MAX_SESSIONS`] completed records, newest
//! first, plus the live session's last saved figures. The live record is
//! written periodically and on a clean exit; the daemon's next start closes an
//! unfinished one as "App closed" instead of losing it. Loading is read-only:
//! a CLI command can build an App while the daemon runs without touching the
//! file. Writes use the same private directory, temp file, fsync and rename
//! pattern as [`crate::settings`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::ensure_private_dir;

pub use jafnify_core::session::{
    ReconnectRecord, SessionRecord, SessionStats, SessionTracker, MAX_SESSIONS,
};

/// The on-disk shape; TOML needs a key for the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
struct SessionHistoryFile {
    #[serde(default)]
    sessions: Vec<SessionRecord>,
    #[serde(default)]
    live: Option<SessionRecord>,
}

/// Completed sessions, newest first, plus the live session's last saved
/// figures. The live record is never shown by [`Self::recent`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionHistory {
    records: Vec<SessionRecord>,
    live: Option<SessionRecord>,
}

impl SessionHistory {
    /// A missing, unreadable or corrupt file is an empty history. This is a
    /// read-only load: it never writes, so a CLI command that builds an App
    /// while the daemon is alive cannot close or rewrite the live session.
    /// The live slot is ignored here; only the daemon's start calls
    /// [`Self::recover_unfinished`] to close it.
    pub fn load(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => return Self::default(),
        };
        let mut file: SessionHistoryFile = match toml::from_str(&text) {
            Ok(file) => file,
            Err(_) => return Self::default(),
        };
        // The file is written newest first, but a file from an older build
        // (or one that was merged by hand) may be oldest first. Sorting here
        // means the list always shows the newest sessions, and truncating
        // after the sort never throws a newer record away for an older one.
        sort_newest_first(&mut file.sessions);
        file.sessions.truncate(MAX_SESSIONS);
        Self {
            records: file.sessions,
            live: None,
        }
    }

    /// Close an unfinished live record left by a previous process and append
    /// it once. Only the daemon calls this, after it holds the single-instance
    /// (tray) lock; a CLI command that builds an App must not write the
    /// history. A missing, unreadable or corrupt file has nothing to recover.
    /// Returns true when a record was appended.
    pub fn recover_unfinished(path: &Path) -> std::io::Result<bool> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let mut file: SessionHistoryFile = match toml::from_str(&text) {
            Ok(file) => file,
            Err(_) => return Ok(false),
        };
        if file.live.is_none() {
            return Ok(false);
        }
        sort_newest_first(&mut file.sessions);
        file.sessions.truncate(MAX_SESSIONS);
        let mut history = Self {
            records: file.sessions,
            live: file.live,
        };
        let appended = history.close_live("App closed");
        history.save(path)?;
        Ok(appended)
    }

    /// Insert a completed session and keep the newest [`MAX_SESSIONS`]. A
    /// completed record is no longer live.
    pub fn push(&mut self, record: SessionRecord) {
        self.live = None;
        self.records.insert(0, record);
        self.records.truncate(MAX_SESSIONS);
    }

    /// Remember the live session's last figures. The record is closed by a
    /// completed push or by [`Self::load`] on the next start.
    pub fn set_live(&mut self, stats: SessionStats) {
        self.live = Some(live_record(&stats));
    }

    /// Drop the live record, for example after a completed session was
    /// pushed. Returns true when there was one.
    pub fn clear_live(&mut self) -> bool {
        self.live.take().is_some()
    }

    /// Close the live record with `reason` and append it once. A record with
    /// no activity is dropped: it says nothing about a session. Returns true
    /// when a record was appended.
    fn close_live(&mut self, reason: &str) -> bool {
        let Some(mut live) = self.live.take() else {
            return false;
        };
        if live.connected_ms == 0
            && live.up_bytes == 0
            && live.down_bytes == 0
            && live.drops == 0
            && live.reconnects.is_empty()
        {
            return false;
        }
        if live.ended_at == 0 {
            live.ended_at = live.last_activity_at.max(live.started_at);
            live.end_reason = reason.to_string();
        }
        if self.records.iter().any(|record| record.id == live.id) {
            return false;
        }
        self.records.push(live);
        sort_newest_first(&mut self.records);
        self.records.truncate(MAX_SESSIONS);
        true
    }

    /// Take back the newest record when it is the session `id`: a resumed
    /// session is stored once, when it really ends.
    pub fn take_newest(&mut self, id: &str) -> Option<SessionRecord> {
        if self.records.first().is_some_and(|r| r.id == id) {
            Some(self.records.remove(0))
        } else {
            None
        }
    }

    /// Newest-first completed sessions, capped to `limit`.
    pub fn recent(&self, limit: usize) -> Vec<SessionRecord> {
        self.records.iter().take(limit).cloned().collect()
    }

    /// Write all completed sessions atomically, 0600 in a private directory.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        let file = SessionHistoryFile {
            sessions: self.records.clone(),
            live: self.live.clone(),
        };
        let text = toml::to_string(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = tmp_path(path);
        match fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        {
            let mut f = jafnify_core::private_fs::open_private_file(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        Ok(())
    }
}

/// Newest first; the end time breaks a tie between two sessions that started
/// in the same millisecond.
fn sort_newest_first(records: &mut [SessionRecord]) {
    records.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then(right.ended_at.cmp(&left.ended_at))
    });
}

/// The live session without an end. The end fields are filled by
/// [`SessionHistory::close_live`] when the session really ends.
fn live_record(stats: &SessionStats) -> SessionRecord {
    SessionRecord {
        id: stats.id.clone(),
        role: stats.role.clone(),
        started_at: stats.started_at,
        last_activity_at: stats.last_activity_at,
        ended_at: 0,
        connected_ms: stats.connected_ms,
        up_bytes: stats.up_bytes,
        down_bytes: stats.down_bytes,
        avg_up_bps: stats.avg_up_bps,
        avg_down_bps: stats.avg_down_bps,
        peak_up_bps: stats.peak_up_bps,
        peak_down_bps: stats.peak_down_bps,
        drops: stats.drops,
        link_downs: stats.link_downs,
        link_ups: stats.link_ups,
        reconnects: stats.reconnects.clone(),
        peer_endpoint: stats.peer_endpoint.clone(),
        end_reason: String::new(),
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "sessions.toml".to_string());
    path.with_file_name(format!(".{file_name}.tmp"))
}
