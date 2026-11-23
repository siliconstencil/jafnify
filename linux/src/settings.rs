//! Role and autostart settings plus the Linux state file, persisted in
//! `~/.config/jafnify/state.toml`. Writes are atomic (temp + fsync +
//! rename) and every struct rejects unknown fields.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use jafnify_core::state::Peer;
use serde::{Deserialize, Serialize};

/// The window size in logical points and, when the window manager reports it,
/// the outer top-left position. Zero width/height means never saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WindowGeometry {
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
}

/// A saved window smaller than this is treated as corrupt, not restored.
pub const MIN_WINDOW_SIDE: u32 = 240;
/// Anything larger cannot be a real window on a normal desktop.
pub const MAX_WINDOW_SIDE: u32 = 10_000;
const MAX_POSITION: i32 = 1_000_000;

impl WindowGeometry {
    /// A geometry safe to restore. `None` when the size was never saved or is
    /// outside a sane range (a monitor change can leave stale values).
    pub fn sanitized(self) -> Option<Self> {
        if !(MIN_WINDOW_SIDE..=MAX_WINDOW_SIDE).contains(&self.width)
            || !(MIN_WINDOW_SIDE..=MAX_WINDOW_SIDE).contains(&self.height)
        {
            return None;
        }
        Some(Self {
            width: self.width,
            height: self.height,
            x: self
                .x
                .filter(|x| (-MAX_POSITION..=MAX_POSITION).contains(x)),
            y: self
                .y
                .filter(|y| (-MAX_POSITION..=MAX_POSITION).contains(y)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub role: jafnify_core::state::Role,
    pub autostart: bool,
    /// Laptop kill switch: off unless the user turns it on.
    #[serde(default)]
    pub kill_switch: bool,
    /// Tray icon visibility: when true, hide it only while a window is open.
    /// Default false: the tray is the status indicator and stays visible.
    #[serde(default)]
    pub tray_only_when_closed: bool,
    /// Details header was left open when the window last closed.
    #[serde(default)]
    pub details_expanded: bool,
    /// Advanced header was left open when the window last closed.
    #[serde(default)]
    pub advanced_expanded: bool,
    /// System/Dark/Light; old state files without it load as System.
    #[serde(default)]
    pub theme: crate::theme::ThemeMode,
    /// Size and position of the last GUI window.
    #[serde(default)]
    pub window: WindowGeometry,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            role: jafnify_core::state::Role::Joiner,
            autostart: false,
            kill_switch: false,
            tray_only_when_closed: false,
            details_expanded: false,
            advanced_expanded: false,
            theme: crate::theme::ThemeMode::System,
            window: WindowGeometry::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LinuxStateFile {
    pub peers: Vec<Peer>,
    pub settings: Settings,
}

impl LinuxStateFile {
    pub fn default_state() -> Self {
        Self::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt state file: {0}")]
    Corrupt(String),
    #[error("atomic write failed: {0}")]
    AtomicWrite(String),
}

pub fn load_state(path: &Path) -> Result<LinuxStateFile, SettingsError> {
    if !path.exists() {
        return Ok(LinuxStateFile::default());
    }
    let text = fs::read_to_string(path)?;
    let mut state: LinuxStateFile =
        toml::from_str(&text).map_err(|e| SettingsError::Corrupt(e.to_string()))?;
    // In memory only: the file is rewritten at the next save.
    jafnify_core::names::upgrade_legacy_names(&mut state.peers);
    Ok(state)
}

/// Load the state file for the running app. A file that cannot be read or
/// parsed (for example one written by a newer version with fields this one
/// does not know) is never overwritten: it is moved to
/// `state.toml.unreadable` (or `.unreadable.N` when that name is taken) and
/// the app starts with defaults. Without this the next save would replace
/// the file and lose the pairings in it.
pub fn load_state_or_preserve(path: &Path) -> LinuxStateFile {
    match load_state(path) {
        Ok(state) => state,
        Err(e) => {
            match preserve_unreadable(path) {
                Ok(kept) => eprintln!(
                    "warning: cannot read {} ({e}); kept it as {} and started with defaults",
                    path.display(),
                    kept.display()
                ),
                Err(move_error) => eprintln!(
                    "warning: cannot read {} ({e}) and cannot keep a copy ({move_error})",
                    path.display()
                ),
            }
            LinuxStateFile::default()
        }
    }
}

/// Move an unreadable state file out of the way under a free name. A hard
/// link never replaces an existing file (unlike rename), so an earlier kept
/// copy can not be overwritten even if the name is taken in between.
fn preserve_unreadable(path: &Path) -> std::io::Result<PathBuf> {
    let base = format!("{}.unreadable", path.display());
    for n in 0..1000u32 {
        let target = if n == 0 {
            PathBuf::from(&base)
        } else {
            PathBuf::from(format!("{base}.{n}"))
        };
        match fs::hard_link(path, &target) {
            Ok(()) => {
                fs::remove_file(path)?;
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free name for the unreadable state file",
    ))
}

/// Create `dir` (and parents) 0700 if missing; an existing one is left as it is.
/// The rule and the Windows DACL live in `jafnify_core::private_fs`.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    jafnify_core::private_fs::create_private_dir(dir)
}

pub fn save_state(path: &Path, state: &LinuxStateFile) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let text = toml::to_string(state).map_err(|e| SettingsError::AtomicWrite(e.to_string()))?;
    let tmp = tmp_path(path);
    // The file may hold a pair's private key: create it 0600 from the start (a
    // stale temp file would keep its old mode, so remove it first).
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        SettingsError::AtomicWrite(e.to_string())
    })?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "state.toml".to_string());
    path.with_file_name(format!(".{file_name}.tmp"))
}
