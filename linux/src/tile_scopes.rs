//! The scope of every tile that counts over time, kept in its own file
//! `tile-scopes.toml` next to `state.toml`.
//!
//! It is not a field of `Settings` on purpose: `state.toml` is read with
//! `deny_unknown_fields`, so a new key there would make a 0.1.1 build set the
//! file aside and start unpaired after a downgrade. A 0.1.1 build never reads
//! this file. The scopes are local only: they are never sent to the peer.
//!
//! The TOML shape lives in `jafnify_core::tile_scopes`; this module owns the
//! private file.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = jafnify_core::tile_scopes::FILE_NAME;

/// The stored scopes, tile id to scope word. A missing or unreadable file is
/// no scope at all: every tile falls back to its own default.
pub fn load(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    jafnify_core::tile_scopes::parse(&text)
}

/// Store the scopes atomically (temp + fsync + rename), 0600.
pub fn save(path: &Path, scopes: &BTreeMap<String, String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        crate::settings::ensure_private_dir(parent)?;
    }
    let text = jafnify_core::tile_scopes::to_text(scopes)?;
    let tmp = path.with_file_name(format!(".{FILE_NAME}.tmp"));
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
