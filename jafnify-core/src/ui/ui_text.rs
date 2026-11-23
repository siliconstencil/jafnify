//! Wording and formatting shared by the GUI and the CLI: pure functions, no
//! I/O. The Rust twin of the phone's UiText.java (same outputs for the same
//! inputs); the sentences themselves live in `view`.

use super::time;

/// Stop could not remove the tunnel because the password prompt was refused.
pub const TEARDOWN_DENIED: &str = "could not remove the tunnel: permission denied";

/// While Connected, the uplink carrying the peer route is down.
pub const LINK_LOST: &str = "Wi-Fi link lost - waiting for the hotspot";

/// Another instance owns `jafn0`: this one must not connect or touch it.
pub const FOREIGN_TUNNEL: &str = "Another Jafnify connection is active";

/// Details and history: `Link losses: N   Link restores: N`.
pub fn link_counts_line(downs: u32, ups: u32) -> String {
    format!("Link losses: {downs}   Link restores: {ups}")
}

/// What kind of failure an error text describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    None,
    NoNetwork,
    Other,
}

/// Classifies an error or retry reason; matches the texts `App` produces.
pub fn problem(text: &str) -> Problem {
    if text.is_empty() {
        return Problem::None;
    }
    let t = text.to_lowercase();
    if [
        "no lan interface",
        "bind failed",
        "unreachable",
        "discovery error",
    ]
    .iter()
    .any(|m| t.contains(m))
    {
        Problem::NoNetwork
    } else {
        Problem::Other
    }
}

/// The text after the retry countdown in `App::status_text`; "" for any other
/// status line. Accepts the old `Retrying (Ns) - reason` spelling too, so a
/// window from an older daemon still finds the reason.
pub fn retry_reason(raw: &str) -> &str {
    let rest = if let Some(rest) = raw.strip_prefix("Connection lost - retrying") {
        rest
    } else if let Some(rest) = raw.strip_prefix("Retrying") {
        rest
    } else {
        return "";
    };
    rest.split_once(" - ").map_or("", |(_, reason)| reason)
}

/// "4321" -> "4 3 2 1".
pub fn spaced_code(code: &str) -> String {
    code.chars().map(String::from).collect::<Vec<_>>().join(" ")
}

/// "m:ss", or "h:mm:ss" from one hour.
pub fn format_elapsed(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// First 12 characters of a peer key (public); for the details view only.
pub fn key_prefix(key: &str) -> String {
    key.chars().take(12).collect()
}

/// A local timestamp with its zone abbreviation, or `none` for no value.
pub fn format_timestamp_or_none(epoch_ms: i64) -> String {
    if epoch_ms <= 0 {
        "none".to_string()
    } else {
        time::format_local_millis(epoch_ms)
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago". Relative on purpose: an
/// absolute date would need a time zone library.
pub fn format_ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => match seconds / 86_400 {
            1 => "1 day ago".to_string(),
            d => format!("{d} days ago"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the Linux texts byte for byte after the move into core.
    #[test]
    fn the_moved_texts_are_unchanged() {
        assert_eq!(spaced_code("4321"), "4 3 2 1");
        assert_eq!(format_elapsed(3723), "1:02:03");
        assert_eq!(format_bytes(340 * 1024), "340 KB");
        assert_eq!(format_ago(5 * 60 + 30), "5 min ago");
        assert_eq!(link_counts_line(2, 1), "Link losses: 2   Link restores: 1");
        assert_eq!(format_timestamp_or_none(0), "none");
    }

    #[test]
    fn the_retry_and_problem_rules_are_unchanged() {
        assert_eq!(
            retry_reason("Connection lost - retrying in 9 s - no data from peer"),
            "no data from peer"
        );
        assert_eq!(problem("Network is unreachable"), Problem::NoNetwork);
        assert_eq!(problem("Root privileges required"), Problem::Other);
    }
}
