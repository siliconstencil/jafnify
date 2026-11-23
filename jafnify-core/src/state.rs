//! Persistent peer state.
//! Writes are atomic (temp file + rename) so a crash mid-write can never
//! corrupt the saved peer list.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::private_fs::{create_private_dir, open_private_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Sharer,
    Joiner,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub role: Role,
    #[serde(default)]
    pub last_connected: Option<u64>,
    /// This pair's WireGuard private key (base64). Present = paired. Secret:
    /// never log or print it; the store writes the file 0600.
    #[serde(default)]
    pub private_key: Option<String>,
    /// Peer ip:port at last connect. A hint only.
    #[serde(default)]
    pub last_endpoint: Option<String>,
}

/// Root table wrapper: toml 0.8 rejects an array as the document root, so the
/// peer list is serialized under a `peers` key.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    peers: Vec<Peer>,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt state file: {0}")]
    Corrupt(String),
    #[error("atomic write failed: {0}")]
    AtomicWrite(String),
}

pub trait StateStore {
    fn load(&self) -> Result<Vec<Peer>, StateError>;
    fn save(&self, peers: &[Peer]) -> Result<(), StateError>;
}

pub struct TomlStateStore {
    pub path: PathBuf,
}

impl TomlStateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl StateStore for TomlStateStore {
    fn load(&self) -> Result<Vec<Peer>, StateError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.path)?;
        let mut file: StateFile =
            toml::from_str(&text).map_err(|e| StateError::Corrupt(e.to_string()))?;
        // In memory only: the file is rewritten at the next save.
        crate::names::upgrade_legacy_names(&mut file.peers);
        Ok(file.peers)
    }

    fn save(&self, peers: &[Peer]) -> Result<(), StateError> {
        // A directory we create is 0700; an existing one is left as it is.
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                create_private_dir(parent)?;
            }
        }
        let text = toml::to_string(&StateFile {
            peers: peers.to_vec(),
        })
        .map_err(|e| StateError::AtomicWrite(e.to_string()))?;
        let tmp = tmp_path(&self.path);
        // The file may hold a private key: create it 0600 from the start (a
        // stale temp file would keep its old mode, so remove it first).
        match fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        {
            let mut f = open_private_file(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            StateError::AtomicWrite(e.to_string())
        })?;
        Ok(())
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "state.toml".to_string());
    path.with_file_name(format!(".{file_name}.tmp"))
}
