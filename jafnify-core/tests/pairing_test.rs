use std::time::Duration;

use jafnify_core::pairing::{
    CodeValidator, DiscoveryToken, PairingCode, PairingError, RateLimiter,
};

#[test]
fn code_is_four_digits() {
    for _ in 0..100 {
        let code = PairingCode::generate();
        let s = code.as_str();
        assert_eq!(s.len(), 4);
        assert!(s.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(s.as_bytes()[0], b'0');
    }
}

#[test]
fn code_parse_rejects_wrong_length_and_non_digits() {
    assert!(PairingCode::parse("123").is_none());
    assert!(PairingCode::parse("12a4").is_none());
    assert!(PairingCode::parse("12345").is_none());
    assert!(PairingCode::parse("1234").is_some());
}

fn legacy_token(code: &PairingCode, nonce: [u8; 16]) -> DiscoveryToken {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_str().as_bytes()).unwrap();
    mac.update(&nonce);
    DiscoveryToken {
        nonce,
        mac: mac.finalize().into_bytes().into(),
    }
}

#[test]
fn the_v2_token_is_a_public_presence_tag() {
    let nonce = [7u8; 16];
    let token = DiscoveryToken::derive(&nonce);
    assert!(token.verify());
    assert_eq!(token, DiscoveryToken::derive(&nonce));
}

#[test]
fn a_legacy_token_still_verifies_only_with_its_code() {
    let code_a = PairingCode::parse("1234").unwrap();
    let code_b = PairingCode::parse("5678").unwrap();
    let legacy = legacy_token(&code_a, [7u8; 16]);
    assert!(legacy.verify_legacy(&code_a));
    assert!(!legacy.verify_legacy(&code_b));
    assert!(!legacy.verify());
}

#[test]
fn the_same_nonce_gives_the_same_token_whatever_the_code() {
    let nonce = [7u8; 16];
    let token = DiscoveryToken::derive(&nonce);
    assert_ne!(
        token,
        legacy_token(&PairingCode::parse("1234").unwrap(), nonce)
    );
    assert_ne!(
        token,
        legacy_token(&PairingCode::parse("5678").unwrap(), nonce)
    );
    assert_eq!(token, DiscoveryToken::derive(&nonce));
}

#[test]
fn token_changes_with_nonce() {
    let t1 = DiscoveryToken::derive(&[1u8; 16]);
    let t2 = DiscoveryToken::derive(&[2u8; 16]);
    assert_ne!(t1.mac, t2.mac);
}

#[test]
fn rate_limiter_blocks_after_max() {
    let mut rl = RateLimiter::new(Duration::from_secs(60), 3);
    assert!(rl.allow());
    assert!(rl.allow());
    assert!(rl.allow());
    assert!(!rl.allow());
}

#[test]
fn validator_rejects_malformed_and_limits() {
    let mut v = CodeValidator::new();
    assert!(matches!(v.try_accept("abc"), Err(PairingError::Malformed)));

    let mut v2 = CodeValidator::new();
    for _ in 0..5 {
        assert!(v2.try_accept("1234").is_ok());
    }
    assert!(matches!(
        v2.try_accept("1234"),
        Err(PairingError::TooManyAttempts)
    ));
}

#[test]
fn validator_rejects_leading_zero() {
    // PairingCode::parse and CodeValidator must agree: a leading zero is
    // malformed (code space is 9000 values, first digit 1-9).
    let mut v = CodeValidator::new();
    assert!(matches!(v.try_accept("0123"), Err(PairingError::Malformed)));
}

#[test]
fn token_bytes_contain_no_plaintext_code() {
    let token = DiscoveryToken::derive(&[7u8; 16]);
    let bytes = token.to_bytes();
    let needle: [u8; 4] = *b"1234";
    assert!(
        !bytes.windows(4).any(|w| w == needle),
        "token bytes leaked the plaintext code: {bytes:?}"
    );
}
