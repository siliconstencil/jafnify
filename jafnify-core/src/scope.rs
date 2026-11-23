//! The scope of a tile: what a tile sums over.
//!
//! A tile that sums over time can be tapped to change the window it sums
//! over: the sharing session, today or the last day. The core owns the
//! enum, the cycle, the window and the words; the platforms only store the
//! choice and hand it in.
//!
//! The session starts when the sharing starts. A reconnect inside the same
//! sharing and an absence of the phone do not end it: the window of the
//! session runs from the start of the sharing to now.

use crate::tiles::{ID_AVERAGE, ID_DROPS, ID_SPEED, ID_TODAY};

/// The words that say a tile can be tapped to change its scope.
pub const TAP_TO_CHANGE: &str = "tap to change";

/// How long a stopped session may be continued. The user stops and
/// reconnects in a few seconds after an update of the phone app, and the
/// session of that moment should not restart because of it.
pub const RESUME_GRACE_MS: i64 = 120_000;

/// Whether a session that was stopped at `stopped_at_ms` may be continued
/// at `now_ms`. The session continues when the same peer comes back inside
/// the grace; a longer gap, another peer or a restart of the daemon starts
/// a new session.
pub fn resumes(stopped_at_ms: i64, now_ms: i64, same_peer: bool) -> bool {
    if !same_peer || stopped_at_ms <= 0 {
        return false;
    }
    let gap = now_ms - stopped_at_ms;
    (0..=RESUME_GRACE_MS).contains(&gap)
}

/// What a tile sums over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileScope {
    /// Since the sharing started.
    Session,
    /// Since the local midnight.
    Today,
    /// The last day, hour by hour.
    Last24Hours,
}

/// The scopes of the cycle, in the order of the cycle.
const CYCLE: [TileScope; 3] = [TileScope::Session, TileScope::Today, TileScope::Last24Hours];

impl TileScope {
    /// The word the scope shows and the settings store.
    pub fn word(self) -> &'static str {
        match self {
            TileScope::Session => "SESSION",
            TileScope::Today => "TODAY",
            TileScope::Last24Hours => "24 H",
        }
    }

    /// The scope of a word. The word is read leniently: a hand-made or an
    /// older settings file still loads.
    pub fn parse(word: &str) -> Option<TileScope> {
        let text: String = word
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|letter| letter.is_ascii_alphanumeric())
            .collect();
        match text.as_str() {
            "session" | "currentsession" | "sessionsince" => Some(TileScope::Session),
            "today" => Some(TileScope::Today),
            "24h" | "24hours" | "last24hours" | "lastday" => Some(TileScope::Last24Hours),
            _ => None,
        }
    }

    /// The words of the scope as a fact of a tile.
    pub fn fact_word(self) -> &'static str {
        match self {
            TileScope::Session => "in session",
            TileScope::Today => "today",
            TileScope::Last24Hours => "in 24 h",
        }
    }

    /// The scope as the subject of a sentence: "this session", "today",
    /// "in 24 h". A tile with no event says it this way.
    pub fn fact_subject(self) -> &'static str {
        match self {
            TileScope::Session => "this session",
            TileScope::Today => "today",
            TileScope::Last24Hours => "in 24 h",
        }
    }

    /// The scopes a tile has, in the order of the cycle.
    pub fn supported(tile_id: &str) -> &'static [TileScope] {
        match tile_id {
            ID_DROPS => &CYCLE,
            ID_TODAY | ID_SPEED | ID_AVERAGE => &[TileScope::Session, TileScope::Today],
            _ => &[TileScope::Today],
        }
    }

    /// Whether the tile can show this scope.
    pub fn supports(self, tile_id: &str) -> bool {
        Self::supported(tile_id).contains(&self)
    }

    /// Whether the tile can change its scope: a tile with one scope only has
    /// nothing to cycle.
    pub fn is_scopeable(tile_id: &str) -> bool {
        Self::supported(tile_id).len() > 1
    }

    /// The next scope of the tile: the cycle skips the scopes the tile does
    /// not have.
    pub fn next(self, tile_id: &str) -> TileScope {
        let scopes = Self::supported(tile_id);
        let Some(index) = scopes.iter().position(|scope| *scope == self) else {
            return scopes[0];
        };
        scopes[(index + 1) % scopes.len()]
    }

    /// The scope a tile starts with.
    pub fn default_for(tile_id: &str) -> TileScope {
        match tile_id {
            ID_DROPS => TileScope::Last24Hours,
            _ => TileScope::Today,
        }
    }

    /// The scope of a tile that the caller remembers, or the default of the
    /// tile when the word is unknown or the tile does not have it.
    pub fn for_tile(tile_id: &str, word: Option<&str>) -> TileScope {
        word.and_then(TileScope::parse)
            .filter(|scope| scope.supports(tile_id))
            .unwrap_or_else(|| TileScope::default_for(tile_id))
    }
}

/// The scope a tile really shows: the asked scope when it has a window, the
/// default of the tile when it has none. A session scope without a session
/// has no window: the tile falls back to its default scope, so the label and
/// the numbers always belong together and the tile keeps its tap.
pub fn effective(scope: TileScope, tile_id: &str, session: Option<&ScopeSession>) -> TileScope {
    match scope {
        TileScope::Session if session.is_none() => TileScope::default_for(tile_id),
        _ => scope,
    }
}

/// The numbers of a tile over its scope, and the scope they come from. The
/// session scope counts the sharing session; the other scopes count what the
/// caller measured.
pub fn totals(
    scope: TileScope,
    tile_id: &str,
    measured: ScopeTotals,
    session: Option<&ScopeSession>,
) -> (TileScope, ScopeTotals) {
    let scope = effective(scope, tile_id, session);
    let numbers = match scope {
        TileScope::Session => match session {
            Some(session) => ScopeTotals {
                up_bytes: session.up_bytes,
                down_bytes: session.down_bytes,
                peak_down_bps: session.peak_down_bps,
            },
            None => measured,
        },
        _ => measured,
    };
    (scope, numbers)
}

/// The sharing session a scope counts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeSession {
    /// When the sharing started, in epoch milliseconds.
    pub started_at_ms: i64,
    /// The bytes that went up in the session.
    pub up_bytes: u64,
    /// The bytes that came down in the session.
    pub down_bytes: u64,
    /// The fastest rate of the session, in bytes per second.
    pub peak_down_bps: u64,
}

impl ScopeSession {
    /// The session as one line: the numbers a platform hands over.
    pub fn line(&self) -> String {
        format!(
            "up={} down={} peak={} start={}",
            self.up_bytes, self.down_bytes, self.peak_down_bps, self.started_at_ms
        )
    }

    /// The session of a line. An empty line and a line with a missing number
    /// are no session; an unknown key is ignored.
    pub fn parse(line: &str) -> Option<ScopeSession> {
        let mut up_bytes = None;
        let mut down_bytes = None;
        let mut peak_down_bps = None;
        let mut started_at_ms = None;
        for token in line.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            match key {
                "up" => up_bytes = value.parse().ok(),
                "down" => down_bytes = value.parse().ok(),
                "peak" => peak_down_bps = value.parse().ok(),
                "start" => started_at_ms = value.parse().ok(),
                _ => {}
            }
        }
        Some(ScopeSession {
            started_at_ms: started_at_ms?,
            up_bytes: up_bytes?,
            down_bytes: down_bytes?,
            peak_down_bps: peak_down_bps?,
        })
    }
}

/// The numbers of a tile over its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScopeTotals {
    /// The bytes that went up.
    pub up_bytes: u64,
    /// The bytes that came down.
    pub down_bytes: u64,
    /// The fastest rate, in bytes per second.
    pub peak_down_bps: u64,
}

/// The start of the window a scope counts over. The session scope needs a
/// session: without one there is no window.
pub fn window_start(
    scope: TileScope,
    now_ms: i64,
    offset_minutes: i32,
    session: Option<&ScopeSession>,
) -> Option<i64> {
    match scope {
        TileScope::Session => session.map(|session| session.started_at_ms),
        TileScope::Today => {
            let offset_ms = i64::from(offset_minutes) * 60_000;
            let (start, _) = crate::usage::window_bounds(now_ms, offset_ms, 1);
            Some(start)
        }
        TileScope::Last24Hours => {
            // The window of the hour strip: the last day, hour by hour,
            // ending at the current hour.
            let hour_ms = 3_600_000;
            let cells = crate::tiles::STRIP_CELLS as i64;
            Some(crate::usage::hour_start_ms(now_ms) - (cells - 1) * hour_ms)
        }
    }
}

/// Whether a moment falls in the window of the scope. The window of the
/// session runs from the start of the sharing to now: a reconnect inside the
/// sharing and an absence of the phone do not end it.
pub fn in_scope(
    at_ms: i64,
    scope: TileScope,
    now_ms: i64,
    offset_minutes: i32,
    session: Option<&ScopeSession>,
) -> bool {
    match window_start(scope, now_ms, offset_minutes, session) {
        Some(start) => at_ms >= start && at_ms <= now_ms,
        None => false,
    }
}
