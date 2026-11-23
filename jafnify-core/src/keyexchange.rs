//! X25519 key exchange. Generates a fresh keypair per session and derives a
//! shared secret with the peer after pairing confirms the code.

use base64::Engine as _;
use rand::RngCore;
use x25519_dalek::{PublicKey, StaticSecret};

#[derive(Debug, thiserror::Error)]
pub enum KeyExchangeError {
    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),
    #[error("diffie-hellman failed: {0}")]
    Dh(String),
    #[error("invalid private key: {0}")]
    InvalidPrivateKey(String),
}

pub struct KeyPair {
    secret: StaticSecret,
    public: PublicKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Rebuilds the keypair from a stored base64 private key (a persisted
    /// pair keeps its static WireGuard key).
    pub fn from_private_b64(private_b64: &str) -> Result<Self, KeyExchangeError> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(private_b64)
            .map_err(|e| KeyExchangeError::InvalidPrivateKey(e.to_string()))?;
        let seed: [u8; 32] = raw.try_into().map_err(|r: Vec<u8>| {
            KeyExchangeError::InvalidPrivateKey(format!("expected 32 bytes, got {}", r.len()))
        })?;
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret);
        Ok(Self { secret, public })
    }

    pub fn public_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.public.as_bytes())
    }

    pub fn private_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.secret.to_bytes())
    }

    /// Raw 32-byte X25519 public key, for the handshake wire frame.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(self.public.as_bytes());
        out
    }

    pub fn exchange(&self, remote_public_b64: &str) -> Result<[u8; 32], KeyExchangeError> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(remote_public_b64)
            .map_err(|e| KeyExchangeError::InvalidPublicKey(e.to_string()))?;
        if raw.len() != 32 {
            return Err(KeyExchangeError::InvalidPublicKey(format!(
                "expected 32 bytes, got {}",
                raw.len()
            )));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&raw);
        let remote = PublicKey::from(bytes);
        let shared = self.secret.diffie_hellman(&remote);
        let out = shared.as_bytes().to_owned();
        // A low-order remote public key (e.g. all-zero) yields an all-zero
        // shared secret -- never usable as a session key.
        if out.iter().all(|b| *b == 0) {
            return Err(KeyExchangeError::Dh(
                "shared secret is all zero (low-order public key)".into(),
            ));
        }
        Ok(out)
    }
}
