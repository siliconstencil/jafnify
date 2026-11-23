use jafnify_core::keyexchange::{KeyExchangeError, KeyPair};

// Test fixtures: placeholder EXAMPLE keys (all-zero base64), never real WireGuard keys.
#[test]
fn public_key_is_44_char_base64() {
    let kp = KeyPair::generate();
    let b64 = kp.public_key_b64();
    assert_eq!(b64.len(), 44);
    assert!(b64
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
}

#[test]
fn dh_agrees_between_two_parties() {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let s1 = a.exchange(&b.public_key_b64()).unwrap();
    let s2 = b.exchange(&a.public_key_b64()).unwrap();
    assert_eq!(s1, s2);
    assert_ne!(s1, [0u8; 32]);
}

#[test]
fn dh_differs_across_parties() {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let c = KeyPair::generate();
    let s_ab = a.exchange(&b.public_key_b64()).unwrap();
    let s_ac = a.exchange(&c.public_key_b64()).unwrap();
    assert_ne!(s_ab, s_ac);
}

#[test]
fn invalid_public_key_rejected() {
    let a = KeyPair::generate();
    assert!(matches!(
        a.exchange("!!not-base64!!"),
        Err(KeyExchangeError::InvalidPublicKey(_))
    ));
}

// A persisted pair re-creates its static key from the stored private key.
#[test]
fn keypair_restores_from_private_key_b64() {
    let a = KeyPair::generate();
    let restored = KeyPair::from_private_b64(&a.private_key_b64()).unwrap();
    assert_eq!(restored.public_key_b64(), a.public_key_b64());
    assert_eq!(restored.private_key_b64(), a.private_key_b64());
    let b = KeyPair::generate();
    assert_eq!(
        restored.exchange(&b.public_key_b64()).unwrap(),
        b.exchange(&a.public_key_b64()).unwrap()
    );
}

#[test]
fn malformed_private_key_rejected() {
    assert!(KeyPair::from_private_b64("!!not-base64!!").is_err());
    assert!(KeyPair::from_private_b64("AAAA").is_err());
    assert!(KeyPair::from_private_b64("").is_err());
}

#[test]
fn all_zero_public_key_rejected() {
    // 32 zero bytes base64 = the all-zero/low-order point; DH must refuse it
    // instead of silently producing an all-zero shared secret.
    let a = KeyPair::generate();
    assert!(matches!(
        a.exchange("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
        Err(KeyExchangeError::Dh(_))
    ));
}
