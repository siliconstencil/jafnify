//! The Linux diagnostics export: the connection log, the journal tail and the
//! application log tail, redacted once.
//!
//! Everything that leaves the device goes through the core redactor
//! (`jafnify_core::redact`), so the Linux export, the Android zip and the
//! Android share text redact the same way. The rules live in the core; this
//! module only gathers the pieces, caps them and lays the sections out.
//!
//! The export is plain text, so it can be pasted into a bug report. Every
//! section is bounded: the journal and the application log contribute a tail
//! of a fixed number of lines, never the whole file.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use jafnify_core::redact::{redact_export, redact_export_with};

use crate::applog;

/// How many journal lines the export carries at most.
pub const JOURNAL_TAIL_LINES: usize = 400;

/// How many application log lines the export carries at most.
pub const APP_LOG_TAIL_LINES: usize = 400;

/// How many link evidence lines the export carries at most. The link log holds
/// numbers only: a station reading never carries an address.
pub const LINK_LOG_TAIL_LINES: usize = 60;

/// The opening lines of the export: what it is and what it leaves out.
pub const HEADER: &str = "Jafnify diagnostics\n\
Redacted: public IP addresses, SSIDs and peer identifiers are replaced with placeholders.\n\
Private and LAN addresses are kept. Keys are never included.";

/// Redact one text with the core rules.
pub fn redact(text: &str) -> String {
    redact_export(text)
}

/// Join lines and redact them as one export.
pub fn redact_lines(lines: &[String]) -> String {
    redact(&lines.join("\n"))
}

/// The application log path.
pub fn app_log_path() -> PathBuf {
    applog::default_path()
}

/// The journal paths for one active journal, oldest first: the rotated
/// sibling and then the active file.
pub fn journal_paths_for(active: &Path) -> Vec<PathBuf> {
    vec![crate::diag::rotated_path(active, 1), active.to_path_buf()]
}

/// The journal paths, oldest first: the rotated sibling and then the active
/// journal. Empty in a test process, which never writes a journal.
pub fn journal_paths() -> Vec<PathBuf> {
    match applog::diag_path_unless_test() {
        Some(active) => journal_paths_for(&active),
        None => Vec::new(),
    }
}

/// The last `n` lines of a file, oldest first. A missing or unreadable file
/// gives an empty tail: an export must never fail and must never panic.
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut lines: VecDeque<String> = VecDeque::with_capacity(n);
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if lines.len() == n {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    lines.into_iter().collect()
}

/// The journal tail: the last [`JOURNAL_TAIL_LINES`] lines of the journal and
/// of its rotated sibling, oldest first.
pub fn journal_tail() -> Vec<String> {
    let mut lines = Vec::new();
    for path in journal_paths() {
        lines.extend(tail_lines(&path, JOURNAL_TAIL_LINES));
    }
    let start = lines.len().saturating_sub(JOURNAL_TAIL_LINES);
    lines.split_off(start)
}

/// The application log tail: the last [`APP_LOG_TAIL_LINES`] lines.
///
/// A test process reads nothing: the operator's own log must stay untouched.
pub fn app_log_tail() -> Vec<String> {
    if std::env::var_os(crate::root::TEST_NO_ROOT_ENV).is_some() {
        return Vec::new();
    }
    tail_lines(&app_log_path(), APP_LOG_TAIL_LINES)
}

/// The link evidence tail from a config directory: the last
/// [`LINK_LOG_TAIL_LINES`] lines of its `link.tsv`. The file holds numbers
/// only; a missing file gives an empty tail.
pub fn link_log_tail_in(config_dir: &Path) -> Vec<String> {
    tail_lines(&config_dir.join("link.tsv"), LINK_LOG_TAIL_LINES)
}

/// The machine name, so it never appears verbatim in an export. Empty when
/// the name cannot be read.
pub fn hostname() -> String {
    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: `buf` is a live, zeroed buffer of exactly the size the call
    // expects; the kernel writes at most `buf.len()` bytes into it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let bytes: Vec<u8> = buf
        .iter()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Assemble the diagnostics export and redact it once.
///
/// The core entry point handles the whole export, so the same peer keeps the
/// same `peer-N` label in every section. `peer_aliases` are the peer's human
/// name, key prefix and pair id; an endpoint is not an alias, so a private
/// address is kept. The host name is replaced by `<host>` wherever it appears.
pub fn bundle(
    connection_log: &[String],
    journal: &[String],
    app_log: &[String],
    link_log: &[String],
    peer_aliases: &[&str],
    hostname: &str,
) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    push_section(&mut out, "connection log ({n} lines)", connection_log);
    push_section(&mut out, "journal (last {n} lines)", journal);
    push_section(&mut out, "application log (last {n} lines)", app_log);
    push_section(&mut out, "link evidence (last {n} lines)", link_log);
    let peers_storage: [&[&str]; 1] = [peer_aliases];
    let peers: &[&[&str]] = if peer_aliases.is_empty() {
        &[]
    } else {
        &peers_storage
    };
    redact_export_with(&out, peers, hostname)
}

/// One section: its header and its lines. A section without lines is left
/// out, so a missing journal costs the export nothing.
fn push_section(out: &mut String, label: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str("--- ");
    out.push_str(&label.replace("{n}", &lines.len().to_string()));
    out.push_str(" ---\n");
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
}
