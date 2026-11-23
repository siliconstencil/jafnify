//! The Details panel as label/value rows, the same structure the phone uses
//! (Connection, This session). Pure: the window only draws these rows, so the
//! wording and order are unit-tested without egui.
//!
//! The platform supplies the facts through [`DetailsSource`]; the row wording
//! and order stay here.

use crate::connection_details::{ConnectionDetails, DeviceSide, DnsMode};
use crate::ui::ui_text::{format_elapsed, format_timestamp_or_none};
use crate::usage;

/// One row: a muted label and its value. `warn` draws the value in the
/// warning colour (a host problem the user should act on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    pub label: String,
    pub value: String,
    pub warn: bool,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailSection {
    pub title: &'static str,
    pub rows: Vec<DetailRow>,
}

fn row(label: &str, value: impl Into<String>) -> DetailRow {
    DetailRow {
        label: label.to_string(),
        value: value.into(),
        warn: false,
    }
}

/// A byte rate, in the same unit the tiles use (one decimal, decimal units),
/// so a rate reads the same on a tile and in a row.
pub fn rate(bytes_per_second: u64) -> String {
    usage::format_rate(bytes_per_second)
}

fn endpoint(value: &str) -> String {
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value.to_string()
    }
}

/// The core's handshake detail without its own lead ("18 s ago").
pub fn handshake_age(detail: &str) -> String {
    detail
        .strip_prefix("Last handshake ")
        .unwrap_or(detail)
        .to_string()
}

/// The facts `sections` reads from a platform snapshot.
pub trait DetailsSource {
    fn status_text(&self) -> &str;
    fn path(&self) -> Option<&str>;
    fn health_line(&self) -> Option<&str>;
    fn dns_through_tunnel(&self) -> bool;
    fn peer_endpoint(&self) -> Option<&str>;
    fn health_detail(&self) -> Option<&str>;
    fn peer_key_prefix(&self) -> Option<&str>;
    fn pair_id(&self) -> Option<&str>;
    fn dns_override(&self) -> Option<&str>;
    /// The host hints as (title, help) pairs; empty when none were detected.
    fn host_hints(&self) -> Vec<(String, String)>;
    /// The live session's start time; 0 means there is no live session.
    fn current_session_started_at(&self) -> i64;
    /// The live session's rows, built by the platform from its own stats.
    fn current_session_rows(&self) -> Vec<DetailRow>;
}

/// The live or finished session facts `session_rows` and `history_rows` need.
pub trait SessionFacts {
    fn started_at(&self) -> i64;
    fn connected_ms(&self) -> u64;
    fn up_bytes(&self) -> u64;
    fn down_bytes(&self) -> u64;
    fn avg_up_bps(&self) -> u64;
    fn avg_down_bps(&self) -> u64;
    fn peak_up_bps(&self) -> u64;
    fn peak_down_bps(&self) -> u64;
    fn drops(&self) -> u32;
    fn reconnects(&self) -> usize;
    fn link_downs(&self) -> u32;
    fn link_ups(&self) -> u32;
    fn peer_endpoint(&self) -> &str;
}

/// A finished session also has its end.
pub trait SessionEndFacts: SessionFacts {
    fn ended_at(&self) -> i64;
    fn end_reason(&self) -> &str;
    fn role(&self) -> &str;
}

impl SessionFacts for crate::session::SessionStats {
    fn started_at(&self) -> i64 {
        self.started_at
    }

    fn connected_ms(&self) -> u64 {
        self.connected_ms
    }

    fn up_bytes(&self) -> u64 {
        self.up_bytes
    }

    fn down_bytes(&self) -> u64 {
        self.down_bytes
    }

    fn avg_up_bps(&self) -> u64 {
        self.avg_up_bps
    }

    fn avg_down_bps(&self) -> u64 {
        self.avg_down_bps
    }

    fn peak_up_bps(&self) -> u64 {
        self.peak_up_bps
    }

    fn peak_down_bps(&self) -> u64 {
        self.peak_down_bps
    }

    fn drops(&self) -> u32 {
        self.drops
    }

    fn reconnects(&self) -> usize {
        self.reconnects.len()
    }

    fn link_downs(&self) -> u32 {
        self.link_downs
    }

    fn link_ups(&self) -> u32 {
        self.link_ups
    }

    fn peer_endpoint(&self) -> &str {
        &self.peer_endpoint
    }
}

impl SessionFacts for crate::session::SessionRecord {
    fn started_at(&self) -> i64 {
        self.started_at
    }

    fn connected_ms(&self) -> u64 {
        self.connected_ms
    }

    fn up_bytes(&self) -> u64 {
        self.up_bytes
    }

    fn down_bytes(&self) -> u64 {
        self.down_bytes
    }

    fn avg_up_bps(&self) -> u64 {
        self.avg_up_bps
    }

    fn avg_down_bps(&self) -> u64 {
        self.avg_down_bps
    }

    fn peak_up_bps(&self) -> u64 {
        self.peak_up_bps
    }

    fn peak_down_bps(&self) -> u64 {
        self.peak_down_bps
    }

    fn drops(&self) -> u32 {
        self.drops
    }

    fn reconnects(&self) -> usize {
        self.reconnects.len()
    }

    fn link_downs(&self) -> u32 {
        self.link_downs
    }

    fn link_ups(&self) -> u32 {
        self.link_ups
    }

    fn peer_endpoint(&self) -> &str {
        &self.peer_endpoint
    }
}

impl SessionEndFacts for crate::session::SessionRecord {
    fn ended_at(&self) -> i64 {
        self.ended_at
    }

    fn end_reason(&self) -> &str {
        &self.end_reason
    }

    fn role(&self) -> &str {
        &self.role
    }
}

/// Connection and, while one exists, This session. `error` is the error the
/// window shows (its own start error first, then the daemon's).
pub fn sections<S: DetailsSource>(snapshot: &S, error: Option<&str>) -> Vec<DetailSection> {
    let details = ConnectionDetails {
        path: snapshot.path().map(str::to_string),
        verdict: snapshot.health_line().map(str::to_string),
        dns: if snapshot.dns_through_tunnel() {
            DnsMode::ThroughTunnel
        } else {
            DnsMode::SystemResolver
        },
        endpoint: snapshot.peer_endpoint().map(str::to_string),
    };
    let mut link = vec![row("Status", snapshot.status_text())];
    for (label, value) in details.rows(DeviceSide::Laptop) {
        link.push(row(&label, value));
    }
    if let Some(detail) = snapshot.health_detail() {
        link.push(row("Last handshake", handshake_age(detail)));
    }
    if let Some(prefix) = snapshot.peer_key_prefix() {
        link.push(row("Peer key", prefix));
    }
    link.push(row(
        "Pair id",
        snapshot
            .pair_id()
            .map(str::to_string)
            .unwrap_or_else(|| "none".to_string()),
    ));
    link.push(row("Last error", error.unwrap_or("none")));
    if let Some(note) = snapshot.dns_override() {
        link.push(DetailRow {
            warn: true,
            ..row("System DNS", note)
        });
    }
    let mut out = vec![DetailSection {
        title: "Connection",
        rows: link,
    }];
    if snapshot.current_session_started_at() > 0 {
        out.push(DetailSection {
            title: "This session",
            rows: snapshot.current_session_rows(),
        });
    }
    let host_hints = snapshot.host_hints();
    if !host_hints.is_empty() {
        out.push(DetailSection {
            title: "This computer",
            rows: host_hints
                .into_iter()
                .map(|(title, help)| row(&title, help))
                .collect(),
        });
    }
    out
}

/// The current session from the laptop's side, in the tile's words and units:
/// down before up, decimal bytes and rates, the same numbers the session tile
/// shows.
pub fn session_rows<S: SessionFacts>(s: &S) -> Vec<DetailRow> {
    session_rows_with_drops(s, s.drops())
}

/// The same rows with the drops of the session supplied by the caller. The
/// platform counts them from the drop events, the same source the drops tile
/// reads, so a stop or a restart of the phone is not counted as a drop here
/// while the tile does not count it either.
pub fn session_rows_with_drops<S: SessionFacts>(s: &S, drops: u32) -> Vec<DetailRow> {
    vec![
        row("Started", format_timestamp_or_none(s.started_at())),
        row("Connected for", format_elapsed(s.connected_ms() / 1_000)),
        row("Down", usage::format_bytes(s.down_bytes())),
        row("Up", usage::format_bytes(s.up_bytes())),
        row(
            "Average",
            format!(
                "{} down, {} up",
                rate(s.avg_down_bps()),
                rate(s.avg_up_bps())
            ),
        ),
        row(
            "Peak",
            format!(
                "{} down, {} up",
                rate(s.peak_down_bps()),
                rate(s.peak_up_bps())
            ),
        ),
        row("Drops", drops.to_string()),
        row("Reconnects", s.reconnects().to_string()),
        row(
            "Link losses",
            format!("{} lost, {} restored", s.link_downs(), s.link_ups()),
        ),
        row("Peer address", endpoint(s.peer_endpoint())),
    ]
}

/// One finished session, in the same order plus its end.
pub fn history_rows<R: SessionEndFacts>(r: &R) -> Vec<DetailRow> {
    vec![
        row("Started", format_timestamp_or_none(r.started_at())),
        row("Ended", format_timestamp_or_none(r.ended_at())),
        row("End reason", r.end_reason()),
        row("Role", r.role()),
        row("Connected for", format_elapsed(r.connected_ms() / 1_000)),
        row("Down", usage::format_bytes(r.down_bytes())),
        row("Up", usage::format_bytes(r.up_bytes())),
        row(
            "Average",
            format!(
                "{} down, {} up",
                rate(r.avg_down_bps()),
                rate(r.avg_up_bps())
            ),
        ),
        row(
            "Peak",
            format!(
                "{} down, {} up",
                rate(r.peak_down_bps()),
                rate(r.peak_up_bps())
            ),
        ),
        row("Drops", r.drops().to_string()),
        row("Reconnects", r.reconnects().to_string()),
        row(
            "Link losses",
            format!("{} lost, {} restored", r.link_downs(), r.link_ups()),
        ),
        row("Peer address", endpoint(r.peer_endpoint())),
    ]
}
