//! The view state both apps show, in one place.
//!
//! The words and the verdicts live here so the phone and the laptop say the
//! same thing about the same tunnel. A platform fills the values it knows and
//! renders the strings; it never works out the wording itself.

use std::time::Duration;

use crate::orchestrator::ConnectionState;
use crate::OLD_PEER_CARD_REASON;

/// A handshake younger than this proves the peer is still there.
/// WireGuard rekeys every 120 s, so 180 s leaves one rekey of slack.
pub const HANDSHAKE_FRESH_SECS: u64 = 180;

/// How the peer is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerPath {
    /// The peer answered recently: it is on the same hotspot.
    DirectHotspot,
    /// The tunnel is up but no handshake has ever completed.
    WaitingForHandshake,
    /// The peer did not answer in time.
    Unreachable,
    /// The platform cannot tell yet.
    Unknown,
}

impl PeerPath {
    /// The words on the card, or nothing when there is nothing to say.
    pub fn text(self) -> Option<&'static str> {
        match self {
            PeerPath::DirectHotspot => Some("Direct over hotspot"),
            PeerPath::WaitingForHandshake => Some("Waiting for the first handshake"),
            PeerPath::Unreachable => Some("Not reachable"),
            PeerPath::Unknown => None,
        }
    }
}

/// How fresh the peer is, as the platform can tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Seconds since the last WireGuard handshake.
    Age(u64),
    /// The platform knows no handshake ever happened.
    Never,
    /// The platform cannot say.
    Unknown,
}

/// The health of the link, for the card under the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// How fresh the peer is, as the platform can tell it.
    pub handshake: Freshness,
    /// Whether the peer is reachable.
    pub path: PeerPath,
    /// The peer runs an older Jafnify.
    pub old_peer: bool,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            handshake: Freshness::Unknown,
            path: PeerPath::Unknown,
            old_peer: false,
        }
    }
}

impl Health {
    /// The verdict on the main screen, or nothing when there is nothing to
    /// say. The moving handshake age is deliberately not part of it; that
    /// belongs in Details ([`Health::handshake_detail`]).
    pub fn verdict(&self) -> Option<String> {
        if self.old_peer {
            return Some(OLD_PEER_CARD_REASON.to_string());
        }
        self.path.text().map(str::to_string)
    }

    /// The handshake age for the Details view. `Never` and `Unknown` have no
    /// age to show.
    pub fn handshake_detail(&self) -> Option<String> {
        match self.handshake {
            Freshness::Age(age) => Some(format!("Last handshake {age} s ago")),
            Freshness::Never | Freshness::Unknown => None,
        }
    }

    /// The one-line form (verdict and age), kept for callers that show both
    /// together. The main screens use [`Health::verdict`] only.
    pub fn line(&self) -> Option<String> {
        if self.old_peer {
            return Some(OLD_PEER_CARD_REASON.to_string());
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(detail) = self.handshake_detail() {
            parts.push(detail);
        }
        if let Some(text) = self.path.text() {
            parts.push(text.to_string());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" - "))
        }
    }
}

/// The retry countdown, with the reason it is retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryView {
    /// Seconds left before the next try.
    pub remaining_secs: u64,
    /// Why the connection was lost.
    pub reason: Option<String>,
}

impl RetryView {
    /// The line the card shows while retrying.
    pub fn line(&self) -> String {
        let base = format!("Connection lost - retrying in {} s", self.remaining_secs);
        match self.reason.as_deref() {
            Some(reason) if !reason.is_empty() => format!("{base} - {reason}"),
            _ => base,
        }
    }
}

/// The pairing code and the time it is good for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeView {
    /// The four digits, as shown to the user.
    pub digits: String,
    /// Seconds left before the code stops working, when known.
    pub expires_in_secs: Option<u64>,
    /// The code is past its lifetime.
    pub expired: bool,
}

impl CodeView {
    /// The line under the code.
    ///
    /// Above the last minute it says the remaining minutes; only in the last
    /// minute does it count down in `m:ss`, so a screen reader is not given a
    /// new sentence every second.
    pub fn expiry_line(&self) -> String {
        if self.expired {
            return "Code expired".to_string();
        }
        match self.expires_in_secs {
            Some(secs) if secs < LAST_MINUTE_SECS => format!("Expires in {}", format_mmss(secs)),
            Some(secs) => format!("Expires in {}", format_minutes(secs)),
            None => String::new(),
        }
    }
}

/// The seconds below which the code line counts down in `m:ss`.
const LAST_MINUTE_SECS: u64 = 60;

/// Whole minutes left, rounded up so "Expires in 10 minutes" never understates
/// the remaining time.
fn format_minutes(secs: u64) -> String {
    let minutes = secs.div_ceil(LAST_MINUTE_SECS);
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

/// Everything the card needs, in one struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The connection state.
    pub state: ConnectionState,
    /// The pairing code, while one is on screen.
    pub code: Option<CodeView>,
    /// The retry countdown, while retrying.
    pub retry: Option<RetryView>,
    /// The health of the link.
    pub health: Health,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: ConnectionState::Idle,
            code: None,
            retry: None,
            health: Health::default(),
        }
    }
}

impl Snapshot {
    /// The status line under the title.
    pub fn status_line(&self) -> String {
        match self.state {
            ConnectionState::Idle => "Not sharing".to_string(),
            ConnectionState::Searching => "Searching for the other device".to_string(),
            ConnectionState::Pairing => "Pairing".to_string(),
            ConnectionState::Connecting => "Connecting".to_string(),
            ConnectionState::Connected => "Connected".to_string(),
            ConnectionState::Retry => self
                .retry
                .as_ref()
                .map(|r| r.line())
                .unwrap_or_else(|| "Connection lost - retrying".to_string()),
        }
    }

    /// The status line with the peer's own name appended.
    ///
    /// The words stay the core's; only `<status> - <peer>` is added, so every
    /// front end shows the same sentence and the same suffix.
    pub fn status_line_with_peer(&self, peer: Option<&str>) -> String {
        let base = self.status_line();
        match peer {
            Some(peer) if !peer.is_empty() => format!("{base} - {peer}"),
            _ => base,
        }
    }

    /// The retry line, when the tunnel is retrying.
    pub fn retry_line(&self) -> Option<String> {
        self.retry.as_ref().map(|r| r.line())
    }

    /// The expiry line under the code, when a code is on screen.
    pub fn code_expiry_line(&self) -> Option<String> {
        self.code.as_ref().map(|c| c.expiry_line())
    }

    /// The health line under the status.
    pub fn health_line(&self) -> Option<String> {
        self.health.line()
    }

    /// The verdict on the main screen (no moving seconds).
    pub fn health_verdict(&self) -> Option<String> {
        self.health.verdict()
    }

    /// The handshake age for the Details view.
    pub fn handshake_detail(&self) -> Option<String> {
        self.health.handshake_detail()
    }
}

/// Decide whether the peer is reachable, from how fresh it is.
///
/// A handshake younger than [`HANDSHAKE_FRESH_SECS`] proves the peer is
/// there. A handshake that never happened is not a failure: the tunnel may
/// still be coming up, so it waits for the first handshake. An older one
/// means the peer cannot be reached. When the handshake age is not known the
/// last received data is used instead; without that there is no verdict.
pub fn classify_path(handshake: Freshness, data_age_secs: Option<u64>) -> PeerPath {
    let age = match handshake {
        Freshness::Age(age) => Some(age),
        Freshness::Never => return PeerPath::WaitingForHandshake,
        Freshness::Unknown => data_age_secs,
    };
    match age {
        Some(age) if age <= HANDSHAKE_FRESH_SECS => PeerPath::DirectHotspot,
        Some(_) => PeerPath::Unreachable,
        None => PeerPath::Unknown,
    }
}

/// The non-colour identity of a state marker.
///
/// The colour is only a hint: every state also has a stable name and a shape,
/// so a state is never rendered with colour alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateGlyph {
    /// Stable name the frontends switch on (`connected`, `retrying`).
    pub name: &'static str,
    /// The outline from the shared J-Signal vocabulary.
    pub shape: GlyphShape,
    /// The indicator colour role that accompanies the shape.
    pub colour: &'static str,
}

/// The shape vocabulary shared by the tray, the launcher and the state icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphShape {
    /// No arc, hollow dot.
    NoArcHollowDot,
    /// One arc, hollow dot.
    OneArcHollowDot,
    /// One arc, filled dot.
    OneArcFilledDot,
    /// Two arcs, filled dot.
    TwoArcsFilledDot,
    /// One arc, hollow dot, a slash across it.
    OneArcHollowDotSlash,
    /// The error exclamation.
    Exclamation,
}

impl GlyphShape {
    /// Stable name the frontends switch on.
    pub fn name(self) -> &'static str {
        match self {
            GlyphShape::NoArcHollowDot => "no-arc-hollow-dot",
            GlyphShape::OneArcHollowDot => "one-arc-hollow-dot",
            GlyphShape::OneArcFilledDot => "one-arc-filled-dot",
            GlyphShape::TwoArcsFilledDot => "two-arcs-filled-dot",
            GlyphShape::OneArcHollowDotSlash => "one-arc-hollow-dot-slash",
            GlyphShape::Exclamation => "exclamation",
        }
    }
}

/// The glyph for a connection state.
///
/// Searching and pairing share one shape on purpose (the design review's
/// vocabulary); their names and words still differ, so neither is colour-only.
pub fn state_glyph(state: ConnectionState) -> StateGlyph {
    match state {
        ConnectionState::Idle => StateGlyph {
            name: "idle",
            shape: GlyphShape::NoArcHollowDot,
            colour: "muted",
        },
        ConnectionState::Searching => StateGlyph {
            name: "searching",
            shape: GlyphShape::OneArcHollowDot,
            colour: "muted",
        },
        ConnectionState::Pairing => StateGlyph {
            name: "pairing",
            shape: GlyphShape::OneArcHollowDot,
            colour: "wait",
        },
        ConnectionState::Connecting => StateGlyph {
            name: "connecting",
            shape: GlyphShape::OneArcFilledDot,
            colour: "wait",
        },
        ConnectionState::Connected => StateGlyph {
            name: "connected",
            shape: GlyphShape::TwoArcsFilledDot,
            colour: "ok",
        },
        ConnectionState::Retry => StateGlyph {
            name: "retrying",
            shape: GlyphShape::OneArcHollowDotSlash,
            colour: "wait",
        },
    }
}

/// Minutes and seconds, as `m:ss`.
pub fn format_mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Minutes and seconds of a duration, as `m:ss`.
pub fn format_duration_mmss(duration: Duration) -> String {
    format_mmss(duration.as_secs())
}
