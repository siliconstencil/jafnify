//! The local device name, kept in its own file `device-name.toml` next to
//! `state.toml`. It is not a field of `Settings` on purpose: `state.toml` is
//! read with `deny_unknown_fields`, so a new key there would make a 0.1.1
//! build set the file aside and start unpaired after a downgrade. A 0.1.1
//! build never reads this file. The name is local only: it is never sent to
//! the peer, never exported and never part of a session record.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = "device-name.toml";

const KEY: &str = "name";

/// The stored name, cleaned. A missing or unreadable file is an empty name.
pub fn load(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return String::new();
    };
    table
        .get(KEY)
        .and_then(toml::Value::as_str)
        .map(jafnify_core::nickname::clean)
        .unwrap_or_default()
}

/// Store the cleaned name atomically (temp + fsync + rename), 0600. An empty
/// name removes the file. Returns the name as stored.
pub fn save(path: &Path, name: &str) -> std::io::Result<String> {
    let name = jafnify_core::nickname::clean(name);
    if name.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        return Ok(name);
    }
    if let Some(parent) = path.parent() {
        crate::settings::ensure_private_dir(parent)?;
    }
    let mut table = toml::Table::new();
    table.insert(KEY.to_string(), toml::Value::String(name.clone()));
    let text = toml::to_string(&table).map_err(std::io::Error::other)?;
    let tmp = path.with_file_name(format!(".{FILE_NAME}.tmp"));
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
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
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(name)
}
