//! Read-only check for a host-wide systemd-resolved override.
//!
//! Jafnify only sets DNS on the `jafn0` link. A global `DNS=` or a
//! `Domains=~.` in `/etc/systemd/resolved.conf` or a `resolved.conf.d` drop-in
//! belongs to the machine, not to Jafnify, and can take every lookup away from
//! the hotspot while the tunnel is down. Report it; never change it.

use std::path::{Path, PathBuf};

/// Why a file is an override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsReason {
    /// A `Domains=` list containing `~.` (route every name here).
    Domains,
}

/// The file that carries the host override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsOverride {
    pub file: PathBuf,
    pub reason: DnsReason,
}

impl DnsOverride {
    /// The one plain diagnostic line shown in Details/status.
    pub fn line(&self) -> String {
        format!(
            "System DNS override in {} may break the internet when the tunnel is down",
            self.file.display()
        )
    }
}

/// The `[Resolve]` keys that take name resolution away from the per-link
/// configuration. Other sections are irrelevant to resolved's resolver.
pub fn scan_file(path: &Path) -> Option<DnsReason> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_resolve = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_resolve = line.eq_ignore_ascii_case("[Resolve]");
            continue;
        }
        if !in_resolve {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Only a global `~.` takes every lookup away from the uplink's own
        // DNS; a global DNS= on its own is a common, harmless choice (the
        // links' servers stay in use), so it is not flagged.
        if key.trim() == "Domains"
            && value
                .split(|c: char| c.is_whitespace() || c == ',')
                .any(|d| d == "~.")
        {
            return Some(DnsReason::Domains);
        }
    }
    None
}

fn drop_ins(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("conf"))
        .collect();
    files.sort();
    files
}

/// The first override under `root`, in resolved's precedence order:
/// `/etc/systemd` (base, then drop-ins), `/run/systemd`, `/usr/lib/systemd`.
/// Only real `.conf` drop-ins count; a renamed backup is not read by resolved.
pub fn find(root: &Path) -> Option<DnsOverride> {
    for dir in ["etc/systemd", "run/systemd", "usr/lib/systemd"] {
        let base = root.join(dir).join("resolved.conf");
        if let Some(reason) = scan_file(&base) {
            return Some(DnsOverride { file: base, reason });
        }
        for file in drop_ins(&root.join(dir).join("resolved.conf.d")) {
            if let Some(reason) = scan_file(&file) {
                return Some(DnsOverride { file, reason });
            }
        }
    }
    None
}

/// The override on this machine, if any.
pub fn find_on_host() -> Option<DnsOverride> {
    find(Path::new("/"))
}
