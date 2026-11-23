//! The bounded, redacted connection log ring and its exact line templates.
//!
//! Each entry is one line: `<local timestamp> <text>`. Keys are redacted on
//! the way in, so the UI is safe; the copied and saved text is redacted again
//! on the way out, so a public address and a peer identifier do not leave the
//! device either. The ring keeps the newest [`MAX_LINES`] entries. The platform
//! keeps the optional backing file; this module owns the ring, the templates
//! and the redaction rules.

use std::collections::VecDeque;

use crate::redact::redact_export;
use crate::ui::time::format_local_millis;

/// The in-memory log is bounded to this many lines.
pub const MAX_LINES: usize = 500;

/// The kind of the lines loaded from an earlier run.
pub const EARLIER_KIND: &str = "earlier";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: String,
    line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionLog {
    entries: VecDeque<Entry>,
}

impl ConnectionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest [`MAX_LINES`] lines of `text`, all marked as earlier runs.
    pub fn from_text(text: &str) -> Self {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let skip = lines.len().saturating_sub(MAX_LINES);
        let entries = lines[skip..]
            .iter()
            .map(|line| Entry {
                kind: EARLIER_KIND.to_string(),
                line: (*line).to_string(),
            })
            .collect();
        Self { entries }
    }

    /// Append one event and return the line for the backing file. `kind` is
    /// metadata for callers; `text` is the line after the timestamp.
    pub fn push(&mut self, kind: &str, text: &str, now_ms: i64) -> String {
        if self.entries.len() == MAX_LINES {
            self.entries.pop_front();
        }
        // One entry is one line, in memory and in the file.
        let text = text.replace(['\r', '\n'], " ");
        let line = format!(
            "{} {}",
            format_local_millis(now_ms),
            crate::redact::redact_key_shaped(&text)
        );
        self.entries.push_back(Entry {
            kind: kind.to_string(),
            line: line.clone(),
        });
        line
    }

    /// The last `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let skip = self.entries.len().saturating_sub(n);
        self.entries
            .iter()
            .skip(skip)
            .map(|entry| entry.line.clone())
            .collect()
    }

    /// The newest [`MAX_LINES`] lines joined with `\n` and one trailing
    /// newline, for an atomic rewrite of the backing file.
    pub fn rewrite_body(&self) -> String {
        let mut body = self.tail(MAX_LINES).join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        body
    }

    /// The same lines as [`tail`], joined with `\n` and no trailing newline.
    /// The text leaves the app here, so it is redacted on the way out.
    pub fn copy_text(&self, n: usize) -> String {
        redact_export(&self.tail(n).join("\n"))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The kind of the newest entry (useful to tests and future filters).
    pub fn last_kind(&self) -> Option<&str> {
        self.entries.back().map(|entry| entry.kind.as_str())
    }
}

/// `state <From> -> <To>`.
pub fn state_line(from: &str, to: &str) -> String {
    format!("state {from} -> {to}")
}

/// `disconnected at <event time>: <reason> (detected after <N> s)`.
pub fn drop_line(reason: &str, disconnected_at_ms: i64, detected_at_ms: i64) -> String {
    let delay = detected_at_ms.saturating_sub(disconnected_at_ms).max(0) / 1_000;
    format!(
        "disconnected at {}: {reason} (detected after {delay} s)",
        format_local_millis(disconnected_at_ms)
    )
}

/// The drop line with the likely cause the link evidence gave, appended after
/// the reason. The words live in the core; this only places them.
pub fn drop_line_with_hint(
    reason: &str,
    disconnected_at_ms: i64,
    detected_at_ms: i64,
    hint: Option<&str>,
) -> String {
    let base = drop_line(reason, disconnected_at_ms, detected_at_ms);
    match hint {
        Some(hint) => format!("{base}; {hint}"),
        None => base,
    }
}

/// `reconnected at <event time>`: the line that closes a logged drop. It
/// carries the time and nothing else: no endpoint, no address, no peer id.
pub fn reconnected_line(connected_at_ms: i64) -> String {
    format!("reconnected at {}", format_local_millis(connected_at_ms))
}

/// `privileged <up|down>: <ok|denied> (<polkit rule|password prompt>)`.
pub fn privileged_line(direction: &str, ok: bool, polkit_rule: bool) -> String {
    let outcome = if ok { "ok" } else { "denied" };
    let source = if polkit_rule {
        "polkit rule"
    } else {
        "password prompt"
    };
    format!("privileged {direction}: {outcome} ({source})")
}

pub const SUPPRESS_RESTORED: &str = "suppress rule restored";

/// Logged once when the search finds no usable network on this computer.
pub const NO_NETWORK: &str = "no network; waiting for it to return";

/// `network back after <N> s`.
pub fn network_back_line(secs: u64) -> String {
    format!("network back after {secs} s")
}

/// `session started (<role>)`.
pub fn session_started_line(role: &str) -> String {
    format!("session started ({role})")
}

/// `session resumed (<role>)`: a Connect soon after a Stop continues it.
pub fn session_resumed_line(role: &str) -> String {
    format!("session resumed ({role})")
}

/// `session ended (<reason>)`.
pub fn session_ended_line(reason: &str) -> String {
    format!("session ended ({reason})")
}
