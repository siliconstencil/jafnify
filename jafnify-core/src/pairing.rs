//! Pairing codes, discovery tokens (HMAC), rate limiting, and code validation.
//! A 4-digit code is required; there is no passwordless mode. The raw code
//! never leaves the device -- only an HMAC-derived token is broadcast.

use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;

use crate::CODE_TTL;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    code: String,
}

impl PairingCode {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let first = rng.gen_range(1..=9);
        let rest: u16 = rng.gen_range(0..=999);
        Self {
            code: format!("{first}{rest:03}"),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.code
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != 4 || !s.chars().all(|c| c.is_ascii_digit()) || s.starts_with('0') {
            return None;
        }
        Some(Self {
            code: s.to_string(),
        })
    }

    pub fn is_valid(&self) -> bool {
        self.code.len() == 4
            && self.code.chars().all(|c| c.is_ascii_digit())
            && !self.code.starts_with('0')
    }
}

/// Public label that makes the v2 presence tag a format marker, not a secret.
pub const PUBLIC_DISCOVERY_KEY: &[u8] = b"jafnify/v2/discovery-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryToken {
    pub nonce: [u8; 16],
    pub mac: [u8; 32],
}

impl DiscoveryToken {
    /// v2 tag: `HMAC-SHA256(PUBLIC_DISCOVERY_KEY, "jafnify/v2/discovery" || nonce)`.
    pub fn derive(nonce: &[u8; 16]) -> Self {
        let mut mac =
            HmacSha256::new_from_slice(PUBLIC_DISCOVERY_KEY).expect("HMAC accepts any key length");
        mac.update(b"jafnify/v2/discovery");
        mac.update(nonce);
        Self {
            nonce: *nonce,
            mac: mac.finalize().into_bytes().into(),
        }
    }

    pub fn verify(&self) -> bool {
        let mut mac =
            HmacSha256::new_from_slice(PUBLIC_DISCOVERY_KEY).expect("HMAC accepts any key length");
        mac.update(b"jafnify/v2/discovery");
        mac.update(&self.nonce);
        mac.verify_slice(&self.mac).is_ok()
    }

    /// The pre-v2 tag was `HMAC-SHA256(code, nonce)`. It only tells us the
    /// sender runs an older Jafnify; it never advances pairing.
    pub fn verify_legacy(&self, code: &PairingCode) -> bool {
        let mut mac = HmacSha256::new_from_slice(code.as_str().as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(&self.nonce);
        mac.verify_slice(&self.mac).is_ok()
    }

    /// Wire format: nonce (16) + mac (32). Version byte added by discovery layer.
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut out = [0u8; 48];
        out[..16].copy_from_slice(&self.nonce);
        out[16..].copy_from_slice(&self.mac);
        out
    }

    /// Decodes a wire token. Infallible: a `[u8; 48]` is always exactly a
    /// nonce + MAC, so this is the pure inverse of `to_bytes`.
    pub fn from_bytes(b: &[u8; 48]) -> Self {
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&b[..16]);
        let mut mac = [0u8; 32];
        mac.copy_from_slice(&b[16..]);
        Self { nonce, mac }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimiter {
    window: Duration,
    max_attempts: u32,
    first_attempt: Option<Instant>,
    attempts: u32,
}

impl RateLimiter {
    pub fn new(window: Duration, max_attempts: u32) -> Self {
        Self {
            window,
            max_attempts,
            first_attempt: None,
            attempts: 0,
        }
    }

    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        match self.first_attempt {
            None => {
                self.first_attempt = Some(now);
                self.attempts = 1;
                true
            }
            Some(start) => {
                if now.duration_since(start) > self.window {
                    self.first_attempt = Some(now);
                    self.attempts = 1;
                    true
                } else {
                    self.attempts += 1;
                    self.attempts <= self.max_attempts
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PairingError {
    #[error("pairing code expired")]
    Expired,
    #[error("too many attempts")]
    TooManyAttempts,
    #[error("malformed pairing code")]
    Malformed,
    #[error("code does not match the code this device is searching with")]
    CodeMismatch,
}

/// A pairing code and the instant it was issued.
///
/// The lifetime of a code runs from the instant it was issued, not from the
/// instant the search for it was last (re-)armed. A code that is handed out
/// again after a retry keeps the window it was first given.
#[derive(Debug, Clone)]
pub struct CodeSession {
    code: PairingCode,
    issued_at: Instant,
}

impl CodeSession {
    /// A code issued now.
    pub fn issue(code: PairingCode) -> Self {
        Self::issue_at(code, Instant::now())
    }

    /// A code issued at the given instant.
    pub fn issue_at(code: PairingCode, issued_at: Instant) -> Self {
        Self { code, issued_at }
    }

    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    pub fn issued_at(&self) -> Instant {
        self.issued_at
    }

    /// The instant the code stops working.
    pub fn expires_at(&self) -> Instant {
        self.issued_at + CODE_TTL
    }

    /// How long the code is still good for.
    pub fn remaining_at(&self, now: Instant) -> Duration {
        self.expires_at().saturating_duration_since(now)
    }

    /// True once the code is past its lifetime: the code is good for exactly
    /// [`CODE_TTL`], so it stops working when that time is up.
    pub fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.issued_at) >= CODE_TTL
    }
}

pub struct CodeValidator {
    issued_at: Instant,
    limiter: RateLimiter,
}

impl CodeValidator {
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// A validator whose window starts at the given instant, so it accepts
    /// exactly as long as the code it guards is good for.
    pub fn new_at(issued_at: Instant) -> Self {
        Self {
            issued_at,
            limiter: RateLimiter::new(CODE_TTL, 5),
        }
    }

    pub fn try_accept(&mut self, candidate: &str) -> Result<(), PairingError> {
        if !self.limiter.allow() {
            return Err(PairingError::TooManyAttempts);
        }
        // Same shape rule as `PairingCode::parse`: 4 ASCII digits, no leading
        // zero. A generated code never starts with 0, so neither may an
        // accepted one.
        if candidate.len() != 4
            || !candidate.chars().all(|c| c.is_ascii_digit())
            || candidate.starts_with('0')
        {
            return Err(PairingError::Malformed);
        }
        Ok(())
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.issued_at) >= CODE_TTL
    }
}

impl Default for CodeValidator {
    fn default() -> Self {
        Self::new()
    }
}
