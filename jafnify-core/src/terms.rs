//! Use-at-your-own-risk acceptance.
//! The text is the mobile plan's; the same words are in README.md, SECURITY.md and the
//! Android dialog. Acceptance is one small file in the config directory.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bump when the text changes: every user accepts the new text once.
pub const TERMS_VERSION: u32 = 3;
/// File name in the config directory, next to `state.toml`.
pub const TERMS_FILE: &str = "terms.toml";

/// The full text shown on first start (paragraphs separated by blank lines).
pub const DISCLAIMER: &str = "\
Use at your own risk. Jafnify is free, open-source software provided \"as is\", \
without warranty of any kind, as stated in the MIT License. The authors and \
contributors are not liable for any damage, data loss, security incident, \
service interruption, charges, or legal consequences that result from using it.

You are responsible for how you use it. Sharing a phone's connection may be \
limited or forbidden by your mobile plan's terms. Check them before use; any \
extra charges or account actions are your responsibility.

What it protects. Jafnify encrypts the link between your computer and your \
phone.

Kill switch. If you turn the kill switch on, the internet is blocked while the \
tunnel is down. It stays off unless you turn it on.

Security. Jafnify opens network ports on your devices while it runs. Only pair \
devices you own, on networks you trust. Keep both apps up to date. Report \
security problems privately as described in SECURITY.md.

Do not use Jafnify for anything that is illegal where you are.";

/// The short notice at the end of `jafnify --help`.
pub const HELP_FOOTER: &str = "\
Use at your own risk: Jafnify is free software, provided \"as is\", without \
warranty (MIT License). You are responsible for how you use it, including your \
mobile plan's terms. Full terms: README.md and SECURITY.md.";

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    accepted_at: u64,
}

/// The accepted version stored in `dir`; 0 when there is no usable record.
pub fn accepted_version(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join(TERMS_FILE))
        .ok()
        .and_then(|text| toml::from_str::<Stored>(&text).ok())
        .map_or(0, |s| s.version)
}

/// Whether the current text has been accepted in `dir`.
pub fn is_accepted(dir: &Path) -> bool {
    accepted_version(dir) >= TERMS_VERSION
}

/// Stores the acceptance of the current text in `dir`.
pub fn record_acceptance(dir: &Path, now_secs: u64) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = toml::to_string(&Stored {
        version: TERMS_VERSION,
        accepted_at: now_secs,
    })
    .map_err(|e| io::Error::other(e.to_string()))?;
    std::fs::write(dir.join(TERMS_FILE), text)
}
