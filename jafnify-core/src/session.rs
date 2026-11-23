//! Session statistics: the tracker rules and the record shapes.
//!
//! [`SessionTracker`] follows one live session from the display state and the
//! session's cumulative rx/tx counters. It tracks only Connected time, treats
//! a counter reset as a new baseline, and records drops and reconnects. The
//! completed-session file (its retention, private write and atomic rename) is
//! platform I/O and stays with the frontend; [`MAX_SESSIONS`] is the shared cap.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Completed sessions kept by the frontend's session history.
pub const MAX_SESSIONS: usize = 50;

const CONNECTED: &str = "Connected";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One disconnected interval inside a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReconnectRecord {
    #[serde(default)]
    pub disconnected_at: i64,
    #[serde(default)]
    pub detected_at: i64,
    #[serde(default)]
    pub connected_at: i64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub reason: String,
}

/// A completed session as stored in the history file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub last_activity_at: i64,
    #[serde(default)]
    pub ended_at: i64,
    #[serde(default)]
    pub connected_ms: u64,
    #[serde(default)]
    pub up_bytes: u64,
    #[serde(default)]
    pub down_bytes: u64,
    #[serde(default)]
    pub avg_up_bps: u64,
    #[serde(default)]
    pub avg_down_bps: u64,
    #[serde(default)]
    pub peak_up_bps: u64,
    #[serde(default)]
    pub peak_down_bps: u64,
    #[serde(default)]
    pub drops: u32,
    #[serde(default)]
    pub link_downs: u32,
    #[serde(default)]
    pub link_ups: u32,
    #[serde(default)]
    pub reconnects: Vec<ReconnectRecord>,
    #[serde(default)]
    pub peer_endpoint: String,
    #[serde(default)]
    pub end_reason: String,
}

/// The current session, without the end fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionStats {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub last_activity_at: i64,
    #[serde(default)]
    pub connected_ms: u64,
    #[serde(default)]
    pub up_bytes: u64,
    #[serde(default)]
    pub down_bytes: u64,
    #[serde(default)]
    pub avg_up_bps: u64,
    #[serde(default)]
    pub avg_down_bps: u64,
    #[serde(default)]
    pub peak_up_bps: u64,
    #[serde(default)]
    pub peak_down_bps: u64,
    #[serde(default)]
    pub drops: u32,
    #[serde(default)]
    pub link_downs: u32,
    #[serde(default)]
    pub link_ups: u32,
    #[serde(default)]
    pub reconnects: Vec<ReconnectRecord>,
    #[serde(default)]
    pub peer_endpoint: String,
}

/// Tracks one live session. Times are UTC epoch milliseconds.
#[derive(Debug, Clone)]
pub struct SessionTracker {
    id: String,
    role: String,
    started_at: i64,
    last_activity_at: i64,
    connected_ms: u64,
    up_bytes: u64,
    down_bytes: u64,
    peak_up_bps: u64,
    peak_down_bps: u64,
    drops: u32,
    link_downs: u32,
    link_ups: u32,
    reconnects: Vec<ReconnectRecord>,
    peer_endpoint: String,
    last_state: Option<String>,
    last_sample_at: Option<i64>,
    prev_rx: Option<u64>,
    prev_tx: Option<u64>,
    ended_at: Option<i64>,
    end_reason: String,
}

impl SessionTracker {
    /// Start a session. The id is derived from the start time and a process
    /// counter only; it carries no key material.
    pub fn new(role: &str, started_at: i64) -> Self {
        let sequence = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("{started_at:x}-{sequence:x}");
        Self {
            id,
            role: role.to_string(),
            started_at,
            last_activity_at: 0,
            connected_ms: 0,
            up_bytes: 0,
            down_bytes: 0,
            peak_up_bps: 0,
            peak_down_bps: 0,
            drops: 0,
            link_downs: 0,
            link_ups: 0,
            reconnects: Vec::new(),
            peer_endpoint: String::new(),
            last_state: None,
            last_sample_at: None,
            prev_rx: None,
            prev_tx: None,
            ended_at: None,
            end_reason: String::new(),
        }
    }

    /// Feed one sample. `rx`/`tx` are cumulative for this session, `state` is
    /// the display state, `endpoint` is the last observed peer endpoint, and
    /// `reason` describes a drop if one is detected on this sample.
    pub fn sample(
        &mut self,
        now: i64,
        state: &str,
        rx: u64,
        tx: u64,
        endpoint: &str,
        reason: &str,
    ) {
        // A finished session no longer accepts activity.
        if self.ended_at.is_some() {
            return;
        }
        if !endpoint.is_empty() {
            self.peer_endpoint = endpoint.to_string();
        }

        let was_connected = self.last_state.as_deref() == Some(CONNECTED);
        let is_connected = state == CONNECTED;
        let previous_sample_at = self.last_sample_at;
        let last_activity_before = self.last_activity_at;

        // Only the interval spent in Connected is counted.
        if was_connected {
            if let Some(previous) = previous_sample_at {
                self.connected_ms = self.connected_ms.saturating_add(elapsed_ms(previous, now));
            }
        }

        // A reset is a new baseline: this sample contributes no delta.
        let rx_delta = counter_delta(self.prev_rx, rx);
        let tx_delta = counter_delta(self.prev_tx, tx);
        self.prev_rx = Some(rx);
        self.prev_tx = Some(tx);
        let advanced = rx_delta.unwrap_or(0) > 0 || tx_delta.unwrap_or(0) > 0;

        if was_connected && !is_connected {
            // A reconnect with no activity sample can still be pending. A
            // further drop means it never completed, so close it here rather
            // than leaving a zero connected_at/duration behind.
            let pending_closed_at = match self.reconnects.last_mut() {
                Some(pending) if pending.connected_at == 0 => {
                    pending.connected_at = now;
                    pending.duration_ms = elapsed_ms(pending.disconnected_at, now);
                    Some(now)
                }
                _ => None,
            };
            // Before the first activity sample there is no last-activity fact.
            // Never anchor a new drop before a just-closed pending reconnect.
            let mut disconnected_at = if last_activity_before > 0 {
                last_activity_before
            } else {
                now
            };
            if let Some(closed_at) = pending_closed_at {
                disconnected_at = disconnected_at.max(closed_at);
            }
            self.drops = self.drops.saturating_add(1);
            self.reconnects.push(ReconnectRecord {
                disconnected_at,
                detected_at: now,
                connected_at: 0,
                duration_ms: 0,
                reason: reason.to_string(),
            });
        }

        if let Some(delta) = rx_delta {
            self.down_bytes = self.down_bytes.saturating_add(delta);
        }
        if let Some(delta) = tx_delta {
            self.up_bytes = self.up_bytes.saturating_add(delta);
        }
        if advanced {
            self.last_activity_at = now;
        }

        if let Some(previous) = previous_sample_at {
            let elapsed = elapsed_ms(previous, now);
            if elapsed > 0 {
                if let Some(delta) = rx_delta {
                    let rate = per_second(delta, elapsed);
                    self.peak_down_bps = self.peak_down_bps.max(rate);
                }
                if let Some(delta) = tx_delta {
                    let rate = per_second(delta, elapsed);
                    self.peak_up_bps = self.peak_up_bps.max(rate);
                }
            }
        }

        // The first activity after a drop supplies the reconnect event time.
        if is_connected && advanced {
            if let Some(reconnect) = self.reconnects.last_mut() {
                if reconnect.connected_at == 0 {
                    reconnect.connected_at = now;
                    reconnect.duration_ms = elapsed_ms(reconnect.disconnected_at, now);
                }
            }
        }

        self.last_state = Some(state.to_string());
        self.last_sample_at = Some(now);
    }

    /// The peer restarted its session while data kept flowing: count one drop
    /// that reconnected at once. A pending reconnect is closed first.
    pub fn peer_restart(&mut self, now: i64, reason: &str) {
        if self.ended_at.is_some() {
            return;
        }
        if let Some(pending) = self.reconnects.last_mut() {
            if pending.connected_at == 0 {
                pending.connected_at = now;
                pending.duration_ms = elapsed_ms(pending.disconnected_at, now);
            }
        }
        let disconnected_at = if self.last_activity_at > 0 {
            self.last_activity_at.min(now)
        } else {
            now
        };
        self.drops = self.drops.saturating_add(1);
        self.reconnects.push(ReconnectRecord {
            disconnected_at,
            detected_at: now,
            connected_at: now,
            duration_ms: elapsed_ms(disconnected_at, now),
            reason: reason.to_string(),
        });
    }

    /// Record one uplink loss. Saturated, and a no-op after `finish`.
    pub fn link_down(&mut self) {
        if self.ended_at.is_none() {
            self.link_downs = self.link_downs.saturating_add(1);
        }
    }

    /// Record one uplink restore. Saturated, and a no-op after `finish`.
    pub fn link_up(&mut self) {
        if self.ended_at.is_none() {
            self.link_ups = self.link_ups.saturating_add(1);
        }
    }

    /// The id of this session.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Continue a finished session: a quick Stop and Connect keeps it. A
    /// pending reconnect is closed at the stop, the end is forgotten and the
    /// next sample is a new counter baseline, so the stopped gap counts
    /// neither as connected time nor as a drop.
    pub fn resume(&mut self) {
        if let Some(ended_at) = self.ended_at {
            if let Some(pending) = self.reconnects.last_mut() {
                if pending.connected_at == 0 {
                    pending.connected_at = ended_at;
                    pending.duration_ms = elapsed_ms(pending.disconnected_at, ended_at);
                }
            }
        }
        self.ended_at = None;
        self.end_reason.clear();
        self.last_state = None;
        self.last_sample_at = None;
        self.prev_rx = None;
        self.prev_tx = None;
    }

    /// The live session. Safe before the first sample.
    pub fn current(&self) -> SessionStats {
        SessionStats {
            id: self.id.clone(),
            role: self.role.clone(),
            started_at: self.started_at,
            last_activity_at: self.last_activity_at,
            connected_ms: self.connected_ms,
            up_bytes: self.up_bytes,
            down_bytes: self.down_bytes,
            avg_up_bps: average(self.up_bytes, self.connected_ms),
            avg_down_bps: average(self.down_bytes, self.connected_ms),
            peak_up_bps: self.peak_up_bps,
            peak_down_bps: self.peak_down_bps,
            drops: self.drops,
            link_downs: self.link_downs,
            link_ups: self.link_ups,
            reconnects: self.reconnects.clone(),
            peer_endpoint: self.peer_endpoint.clone(),
        }
    }

    /// Close the session at `now` without persisting it.
    pub fn finish(&mut self, now: i64, reason: &str) -> SessionRecord {
        if self.ended_at.is_none() {
            if self.last_state.as_deref() == Some(CONNECTED) {
                if let Some(previous) = self.last_sample_at {
                    self.connected_ms = self.connected_ms.saturating_add(elapsed_ms(previous, now));
                }
                // A pending reconnect is complete only when the tracker saw a
                // Connected sample after the drop. Finishing while Retry or
                // Searching must leave it at connected_at = 0.
                if let Some(pending) = self.reconnects.last_mut() {
                    if pending.connected_at == 0 {
                        pending.connected_at = now;
                        pending.duration_ms = elapsed_ms(pending.disconnected_at, now);
                    }
                }
            }
            self.ended_at = Some(now);
            self.end_reason = reason.to_string();
        }

        let stats = self.current();
        SessionRecord {
            id: stats.id,
            role: stats.role,
            started_at: stats.started_at,
            last_activity_at: stats.last_activity_at,
            ended_at: self.ended_at.unwrap_or(now),
            connected_ms: stats.connected_ms,
            up_bytes: stats.up_bytes,
            down_bytes: stats.down_bytes,
            avg_up_bps: stats.avg_up_bps,
            avg_down_bps: stats.avg_down_bps,
            peak_up_bps: stats.peak_up_bps,
            peak_down_bps: stats.peak_down_bps,
            drops: stats.drops,
            link_downs: stats.link_downs,
            link_ups: stats.link_ups,
            reconnects: stats.reconnects,
            peer_endpoint: stats.peer_endpoint,
            end_reason: self.end_reason.clone(),
        }
    }
}

fn counter_delta(previous: Option<u64>, current: u64) -> Option<u64> {
    match previous {
        Some(previous) if current >= previous => Some(current - previous),
        // First sample, or a reset: no delta for this tick.
        _ => None,
    }
}

fn elapsed_ms(previous: i64, now: i64) -> u64 {
    now.saturating_sub(previous).max(0) as u64
}

fn per_second(bytes: u64, elapsed_ms: u64) -> u64 {
    if elapsed_ms == 0 {
        return 0;
    }
    let rate = bytes as u128 * 1_000 / elapsed_ms as u128;
    rate.min(u64::MAX as u128) as u64
}

fn average(bytes: u64, connected_ms: u64) -> u64 {
    per_second(bytes, connected_ms)
}
