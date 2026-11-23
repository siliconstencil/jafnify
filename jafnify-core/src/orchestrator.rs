//! Connection lifecycle state machine. Owns search/retry timing and translates
//! discovery and pairing events into UI-visible events.

use std::time::{Duration, Instant};

use crate::pairing::{CodeSession, CodeValidator, DiscoveryToken, PairingCode, PairingError};
use crate::state::Peer;
use crate::RETRY_INTERVAL;

/// Time before the UI is told "no peer found yet" (search continues).
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Idle,
    Searching,
    Pairing,
    Connecting,
    Connected,
    Retry,
}

/// Transient UI notifications emitted by [Orchestrator::tick]. Connection
/// state is the primary interface (poll via [Orchestrator::state]); events are
/// hints for UI messaging, not a state source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    NoPeerFound,
}

pub struct Orchestrator {
    state: ConnectionState,
    local_code: Option<PairingCode>,
    /// Nonce of the discovery token this device announces. Multicast loop
    /// delivers our own frames back; a token with this nonce is ours.
    own_nonce: Option<[u8; 16]>,
    validator: Option<CodeValidator>,
    /// The code this device is searching with and the instant it was issued.
    /// The acceptance window follows the issue instant, so re-arming the code
    /// track never extends it.
    code_session: Option<CodeSession>,
    // Reserved for the peer-list surface the UI layer will expose; populated
    // once state store integration lands (later task).
    #[allow(dead_code)]
    peers: Vec<Peer>,
    search_deadline: Option<Instant>,
    next_retry: Option<Instant>,
    /// The retry delay of the current/last disconnect (10 s normally, 2 s for
    /// a peer-session reset).
    retry_interval: Duration,
    last_disconnect_reason: Option<String>,
    /// A legacy 0x01 presence tag from a pre-v2 peer was seen this search.
    old_peer: bool,
}

impl Orchestrator {
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Idle,
            local_code: None,
            own_nonce: None,
            validator: None,
            code_session: None,
            peers: Vec::new(),
            search_deadline: None,
            next_retry: None,
            retry_interval: RETRY_INTERVAL,
            last_disconnect_reason: None,
            old_peer: false,
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Arms the code track. The acceptance window follows the instant the
    /// code was issued: re-arming the track with the same code keeps the
    /// original window, a different code starts a new one.
    pub fn start_search(&mut self, code: PairingCode) {
        self.arm_code_track_at(code, None, Instant::now());
    }

    /// Arms the code track with the instant the code was issued, for a device
    /// that generated the code itself (the sharer). The window then starts at
    /// the issue instant, not at the moment the track was armed.
    pub fn start_search_at(&mut self, code: PairingCode, issued_at: Instant) {
        self.arm_code_track_at(code, Some(issued_at), Instant::now());
    }

    /// Re-arms the current code track as if the clock read `now`, the way the
    /// retry path does. The window still ends one code lifetime after the
    /// code was issued.
    pub fn rearm_code_track_at(&mut self, now: Instant) {
        if let Some(code) = self.local_code.clone() {
            self.arm_code_track_at(code, None, now);
        }
    }

    fn arm_code_track_at(&mut self, code: PairingCode, issued_at: Option<Instant>, now: Instant) {
        let same_code = self
            .code_session
            .as_ref()
            .is_some_and(|session| session.code().as_str() == code.as_str());
        let session = match (same_code, issued_at) {
            (_, Some(instant)) => Some(CodeSession::issue_at(code.clone(), instant)),
            (true, None) => self.code_session.clone(),
            (false, None) => Some(CodeSession::issue_at(code.clone(), now)),
        };
        if !same_code || issued_at.is_some() || self.validator.is_none() {
            // A new code, an explicit issue instant, or a first arm gets a
            // fresh attempt budget; a plain re-arm keeps the current one.
            self.validator = session
                .as_ref()
                .map(|session| CodeValidator::new_at(session.issued_at()));
        }
        self.code_session = session;
        self.local_code = Some(code);
        self.own_nonce = Some(rand::random());
        self.state = ConnectionState::Searching;
        self.search_deadline = Some(now + SEARCH_TIMEOUT);
        self.next_retry = None;
        self.last_disconnect_reason = None;
        self.old_peer = false;
    }

    /// The code this device is searching with, with the instant it was issued.
    pub fn code_session(&self) -> Option<&CodeSession> {
        self.code_session.as_ref()
    }

    /// True once the code of the current track has outlived its lifetime.
    pub fn code_expired_at(&self, now: Instant) -> bool {
        self.code_session
            .as_ref()
            .is_some_and(|session| session.is_expired(now))
    }

    /// Known-peer track: search for the stored peer with no pairing code (no
    /// 0x01 token, no code to accept; the 0x03 frames carry the proof).
    pub fn start_known_search(&mut self) {
        self.local_code = None;
        self.own_nonce = None;
        self.validator = None;
        self.code_session = None;
        self.state = ConnectionState::Searching;
        self.search_deadline = Some(Instant::now() + SEARCH_TIMEOUT);
        self.next_retry = None;
        self.last_disconnect_reason = None;
        self.old_peer = false;
    }

    /// A known peer proved itself (or its WireGuard data is about to flow):
    /// Searching -> Connecting, where the connect timeout and loss rules apply.
    pub fn known_peer_found(&mut self) {
        if self.state == ConnectionState::Searching {
            self.state = ConnectionState::Connecting;
        }
    }

    /// The discovery token this device announces while searching (stable for
    /// one search, fresh for the next). None before `start_search`.
    pub fn own_token(&self) -> Option<DiscoveryToken> {
        Some(DiscoveryToken::derive(&self.own_nonce?))
    }

    /// True once a legacy 0x01 tag from a pre-v2 peer was seen this search.
    pub fn old_peer_seen(&self) -> bool {
        self.old_peer
    }

    pub fn on_discovery_event(&mut self, token: &DiscoveryToken) -> bool {
        if self.state != ConnectionState::Searching {
            return false;
        }
        let Some(code) = &self.local_code else {
            return false;
        };
        if self.own_nonce == Some(token.nonce) {
            return false;
        }
        if token.verify() {
            self.state = ConnectionState::Pairing;
            true
        } else {
            if token.verify_legacy(code) {
                self.old_peer = true;
            }
            false
        }
    }

    /// Validates a code the user typed in. The code must be well-formed and
    /// MUST equal the code this device is searching with; a mismatched code is
    /// rejected, so a wrong code can never progress toward a connection.
    /// A match moves the connection into [ConnectionState::Connecting] (key
    /// exchange phase); the caller finishes with `confirm_connected`.
    pub fn try_accept_code(&mut self, candidate: &str) -> Result<(), PairingError> {
        self.try_accept_code_at(candidate, Instant::now())
    }

    /// Like [Orchestrator::try_accept_code] with the caller's clock, so the
    /// lifetime of the code is checked against the same time source the UI
    /// uses for the countdown.
    pub fn try_accept_code_at(
        &mut self,
        candidate: &str,
        now: Instant,
    ) -> Result<(), PairingError> {
        let validator = self.validator.as_mut().ok_or(PairingError::Malformed)?;
        if validator.is_expired(now) {
            return Err(PairingError::Expired);
        }
        validator.try_accept(candidate)?;
        let matches = self
            .local_code
            .as_ref()
            .is_some_and(|code| code.as_str() == candidate);
        if !matches {
            return Err(PairingError::CodeMismatch);
        }
        self.state = ConnectionState::Connecting;
        Ok(())
    }

    pub fn confirm_connected(&mut self) {
        self.state = ConnectionState::Connected;
        self.next_retry = None;
    }

    pub fn on_disconnect(&mut self, reason: String) {
        self.on_disconnect_at(reason, Instant::now());
    }

    /// Like [Orchestrator::on_disconnect] with the caller's clock, so the
    /// 10 s retry deadline follows the same time source as `tick`.
    pub fn on_disconnect_at(&mut self, reason: String, now: Instant) {
        self.on_disconnect_at_with_retry(reason, now, RETRY_INTERVAL);
    }

    /// `on_disconnect_at` with an explicit retry delay. The default stays
    /// `RETRY_INTERVAL`; the reset-specific path uses a shorter one.
    pub fn on_disconnect_at_with_retry(&mut self, reason: String, now: Instant, retry: Duration) {
        self.last_disconnect_reason = Some(reason);
        self.state = ConnectionState::Retry;
        self.retry_interval = retry;
        self.next_retry = Some(now + retry);
    }

    /// The retry delay of the current/last disconnect.
    pub fn retry_interval(&self) -> Duration {
        self.retry_interval
    }

    /// How long the caller must wait for the retry search as of `now`; None
    /// outside Retry. Saturates at zero once the deadline has passed.
    pub fn retry_remaining_at(&self, now: Instant) -> Option<Duration> {
        self.next_retry
            .map(|when| when.saturating_duration_since(now))
    }

    /// Reason of the most recent disconnect, for the UI status line.
    pub fn last_disconnect_reason(&self) -> Option<&str> {
        self.last_disconnect_reason.as_deref()
    }

    pub fn tick(&mut self, now: Instant) -> Option<Event> {
        match self.state {
            ConnectionState::Searching => {
                if let Some(deadline) = self.search_deadline {
                    if now >= deadline {
                        self.search_deadline = Some(now + SEARCH_TIMEOUT);
                        return Some(Event::NoPeerFound);
                    }
                }
                None
            }
            ConnectionState::Retry => {
                if let Some(when) = self.next_retry {
                    if now >= when {
                        self.state = ConnectionState::Searching;
                        self.search_deadline = Some(now + SEARCH_TIMEOUT);
                        self.next_retry = None;
                    }
                }
                None
            }
            _ => None,
        }
    }
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}
