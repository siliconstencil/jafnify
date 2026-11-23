//! Known-peer reconnect, core part: pair secret S, pairId, versioned
//! 0x03 Hello/Proof frames.
//! Keys here are freshly generated per test, never real WireGuard keys.

use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{
    Frame, FrameError, KnownError, KnownSession, PairSecret, HELLO_LEN, PROOF_LEN,
};
use jafnify_core::state::Role;

fn secret(own: &KeyPair, peer: &KeyPair) -> PairSecret {
    PairSecret::derive(own, &peer.public_key_b64()).unwrap()
}

fn session(own: &KeyPair, peer: &KeyPair, role: Role) -> KnownSession {
    KnownSession::new(own, &peer.public_key_b64(), role).unwrap()
}

fn other(role: Role) -> Role {
    match role {
        Role::Sharer => Role::Joiner,
        Role::Joiner => Role::Sharer,
    }
}

/// A paired sharer/joiner session couple.
fn couple() -> (KeyPair, KeyPair, KnownSession, KnownSession) {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let sa = session(&a, &b, Role::Sharer);
    let sb = session(&b, &a, Role::Joiner);
    (a, b, sa, sb)
}

// ---- S and pairId ----------------------------------------------------------

#[test]
fn secret_and_pair_id_are_equal_on_both_sides() {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let sa = secret(&a, &b);
    let sb = secret(&b, &a);
    assert_eq!(sa.pair_id(), sb.pair_id());
    // Same S: a tag made by one side verifies on the other.
    let hello = sa.hello(Role::Sharer, [7u8; 16]);
    assert!(hello.verify(&sb));
}

#[test]
fn pair_id_is_32_lowercase_hex_chars() {
    let id = secret(&KeyPair::generate(), &KeyPair::generate()).pair_id();
    assert_eq!(id.len(), 32);
    assert!(id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
}

#[test]
fn pair_id_differs_per_pair() {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let c = KeyPair::generate();
    assert_ne!(secret(&a, &b).pair_id(), secret(&a, &c).pair_id());
    assert_ne!(secret(&a, &b).pair_id(), secret(&b, &c).pair_id());
}

#[test]
fn low_order_peer_keys_are_rejected() {
    use base64::Engine as _;
    let own = KeyPair::generate();
    let low_order: [&str; 5] = [
        // all zero, and the order-1 point
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0100000000000000000000000000000000000000000000000000000000000000",
        // order-8 points
        "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
        "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
        // non-canonical encoding of zero
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ];
    for hex in low_order {
        let raw: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert!(
            matches!(PairSecret::derive(&own, &b64), Err(KnownError::Key(_))),
            "low-order key {hex} accepted by derive"
        );
        assert!(
            KnownSession::new(&own, &b64, Role::Sharer).is_err(),
            "low-order key {hex} accepted by session"
        );
    }
}

#[test]
fn malformed_peer_key_is_rejected() {
    let own = KeyPair::generate();
    assert!(PairSecret::derive(&own, "!!not-base64!!").is_err());
    assert!(PairSecret::derive(&own, "AAAA").is_err());
}

// ---- Hello -----------------------------------------------------------------

#[test]
fn hello_wire_layout() {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let wire = secret(&a, &b).hello(Role::Joiner, [9u8; 16]).encode();
    assert_eq!(wire.len(), HELLO_LEN);
    assert_eq!(HELLO_LEN, 36);
    assert_eq!(&wire[..4], &[0x03, 0x03, 0x01, 0x01]); // 0x03|ver 3|Hello|joiner
    assert_eq!(&wire[4..20], &[9u8; 16]);
}

#[test]
fn hello_round_trips_through_decode() {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let hello = secret(&a, &b).hello(Role::Sharer, [3u8; 16]);
    match Frame::decode(&hello.encode()).unwrap() {
        Frame::Hello(h) => assert_eq!(h, hello),
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn hello_tag_is_valid_only_under_its_secret() {
    let (a, b, c) = (
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    );
    let s_ab = secret(&a, &b);
    let hello = s_ab.hello(Role::Sharer, [1u8; 16]);
    assert!(hello.verify(&s_ab));
    // A different pair's secret (stranger knows no S) rejects it.
    assert!(!hello.verify(&secret(&a, &c)));
    assert!(!hello.verify(&secret(&c, &b)));
}

#[test]
fn hello_tag_binds_role_and_nonce() {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let s = secret(&a, &b);
    let hello = s.hello(Role::Sharer, [1u8; 16]);
    let mut role_flipped = hello.clone();
    role_flipped.role = Role::Joiner;
    assert!(!role_flipped.verify(&s));
    let mut nonce_flipped = hello.clone();
    nonce_flipped.nonce[0] ^= 1;
    assert!(!nonce_flipped.verify(&s));
    let mut tag_flipped = hello;
    tag_flipped.tag[15] ^= 1;
    assert!(!tag_flipped.verify(&s));
}

// ---- Proof -----------------------------------------------------------------

#[test]
fn proof_wire_layout_and_round_trip() {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let proof = secret(&a, &b).proof(
        Role::Sharer,
        [4u8; 16],
        [5u8; 16],
        &a.public_key_bytes(),
        &b.public_key_bytes(),
    );
    let wire = proof.encode();
    assert_eq!(wire.len(), PROOF_LEN);
    assert_eq!(PROOF_LEN, 74);
    assert_eq!(&wire[..4], &[0x03, 0x03, 0x02, 0x00]); // 0x03|ver 3|Proof|sharer
    assert_eq!(&wire[4..20], &[4u8; 16]);
    assert_eq!(&wire[20..36], &[5u8; 16]);
    assert_eq!(&wire[36..38], &9797u16.to_be_bytes()); // the sharer's port
    assert_eq!(&wire[38..42], &[127, 0, 0, 1]); // the Hello source address
    match Frame::decode(&wire).unwrap() {
        Frame::Proof(p) => assert_eq!(p, proof),
        other => panic!("expected Proof, got {other:?}"),
    }
}

#[test]
fn hello_is_answered_with_a_proof_the_asker_accepts() {
    let (_, _, sharer, joiner) = couple();
    let proof = sharer
        .on_hello(&joiner.hello())
        .expect("valid opposite-role Hello must be answered");
    assert!(joiner.accept_proof(&proof));
}

#[test]
fn proof_is_valid_for_the_echoed_nonce_only() {
    let (a, b, _, joiner) = couple();
    let s = secret(&a, &b);
    let (pk_a, pk_b) = (a.public_key_bytes(), b.public_key_bytes());
    // MAC-valid Proof from the real sharer, but echoing a nonce the joiner
    // never sent (a replay from another run).
    let stale = s.proof(Role::Sharer, [8u8; 16], [0xAA; 16], &pk_a, &pk_b);
    assert!(!joiner.accept_proof(&stale));
    // The same Proof echoing the joiner's current nonce is accepted.
    let good = s.proof(Role::Sharer, [8u8; 16], joiner.hello().nonce, &pk_a, &pk_b);
    assert!(joiner.accept_proof(&good));
}

#[test]
fn proof_for_previous_nonce_is_accepted_for_one_rotation_only() {
    let (_, _, sharer, mut joiner) = couple();
    let old_hello = joiner.hello();
    let proof = sharer.on_hello(&old_hello).unwrap();
    joiner.rotate();
    assert_ne!(joiner.hello().nonce, old_hello.nonce);
    assert!(joiner.accept_proof(&proof), "previous nonce still valid");
    joiner.rotate();
    assert!(!joiner.accept_proof(&proof), "two rotations ago is stale");
}

#[test]
fn proof_with_flipped_bits_is_rejected() {
    let (_, _, sharer, joiner) = couple();
    let good = sharer.on_hello(&joiner.hello()).unwrap();
    assert!(joiner.accept_proof(&good));
    let mut p = good.clone();
    p.mac[0] ^= 1;
    assert!(!joiner.accept_proof(&p));
    let mut p = good.clone();
    p.n_self[0] ^= 1;
    assert!(!joiner.accept_proof(&p));
    let mut p = good;
    p.role = Role::Joiner;
    assert!(!joiner.accept_proof(&p));
}

#[test]
fn proof_from_a_stranger_without_s_is_rejected() {
    let (_, b, _, joiner) = couple();
    let m = KeyPair::generate(); // stranger
    let stranger = session(&m, &b, Role::Sharer);
    let forged = stranger.on_hello(&joiner.hello());
    // The stranger's Hello-tag check fails (wrong S), so it cannot even answer.
    assert!(forged.is_none());
    // And a hand-forged Proof under the stranger's own secret fails verify.
    let forged = secret(&m, &b).proof(
        Role::Sharer,
        [1u8; 16],
        joiner.hello().nonce,
        &m.public_key_bytes(),
        &b.public_key_bytes(),
    );
    assert!(!joiner.accept_proof(&forged));
}

#[test]
fn proof_mac_binds_peer_addr() {
    let (a, b, _, joiner) = couple();
    let s = secret(&a, &b);
    let n_echo = joiner.hello().nonce;
    let good = s.proof_with_addr(
        Role::Sharer,
        [2u8; 16],
        n_echo,
        &a.public_key_bytes(),
        &b.public_key_bytes(),
        [192, 168, 1, 9],
    );
    assert!(joiner.accept_proof(&good));
    let mut altered = good;
    altered.peer_addr = [192, 168, 1, 10];
    assert!(!joiner.accept_proof(&altered));
}

#[test]
fn proof_mac_binds_both_public_keys() {
    let (a, b, _, joiner) = couple();
    let s = secret(&a, &b);
    let n_echo = joiner.hello().nonce;
    // pk_self / pk_peer swapped by the sender.
    let swapped = s.proof(
        Role::Sharer,
        [2u8; 16],
        n_echo,
        &b.public_key_bytes(),
        &a.public_key_bytes(),
    );
    assert!(!joiner.accept_proof(&swapped));
    // Correct order is accepted (control).
    let ok = s.proof(
        Role::Sharer,
        [2u8; 16],
        n_echo,
        &a.public_key_bytes(),
        &b.public_key_bytes(),
    );
    assert!(joiner.accept_proof(&ok));
}

// ---- own role / own nonce --------------------------------------------------

#[test]
fn own_role_hello_is_ignored() {
    let (a, b, sharer, _) = couple();
    // Another sharer-role Hello with a valid tag (e.g. a second sharer).
    let same_role = secret(&a, &b).hello(Role::Sharer, [0x11; 16]);
    assert!(same_role.verify(&secret(&a, &b)));
    assert!(sharer.on_hello(&same_role).is_none());
    // Our own looped-back Hello.
    assert!(sharer.on_hello(&sharer.hello()).is_none());
}

#[test]
fn own_nonce_hello_is_ignored_even_with_opposite_role() {
    let (a, b, sharer, _) = couple();
    let echo = secret(&a, &b).hello(other(Role::Sharer), sharer.hello().nonce);
    assert!(echo.verify(&secret(&a, &b)));
    assert!(sharer.on_hello(&echo).is_none());
}

#[test]
fn own_nonce_hello_is_ignored_for_previous_nonce_too() {
    let (a, b, mut sharer, _) = couple();
    let old = sharer.hello().nonce;
    sharer.rotate();
    let echo = secret(&a, &b).hello(Role::Joiner, old);
    assert!(sharer.on_hello(&echo).is_none());
}

#[test]
fn hello_with_bad_tag_gets_no_proof() {
    let (_, _, sharer, joiner) = couple();
    let mut h = joiner.hello();
    h.tag[0] ^= 1;
    assert!(sharer.on_hello(&h).is_none());
}

#[test]
fn own_role_proof_is_rejected() {
    let (a, b, _, joiner) = couple();
    // A MAC-valid Proof that carries the joiner's own role, echoing its nonce.
    let reflected = secret(&b, &a).proof(
        Role::Joiner,
        [6u8; 16],
        joiner.hello().nonce,
        &a.public_key_bytes(),
        &b.public_key_bytes(),
    );
    assert!(!joiner.accept_proof(&reflected));
}

#[test]
fn proof_carrying_our_own_nonce_as_sender_nonce_is_rejected() {
    let (a, b, _, joiner) = couple();
    let own_nonce = joiner.hello().nonce;
    let p = secret(&a, &b).proof(
        Role::Sharer,
        own_nonce, // n_self == the nonce we use ourselves
        own_nonce,
        &a.public_key_bytes(),
        &b.public_key_bytes(),
    );
    assert!(!joiner.accept_proof(&p));
}

#[test]
fn rotate_changes_the_nonce() {
    let (_, _, mut s, _) = couple();
    let first = s.hello().nonce;
    s.rotate();
    assert_ne!(s.hello().nonce, first);
}

#[test]
fn nonce_is_fresh_per_session() {
    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let n1 = session(&a, &b, Role::Sharer).hello().nonce;
    let n2 = session(&a, &b, Role::Sharer).hello().nonce;
    assert_ne!(n1, n2);
}

// ---- decode rejections -----------------------------------------------------

fn sample_hello() -> [u8; HELLO_LEN] {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    secret(&a, &b).hello(Role::Sharer, [1u8; 16]).encode()
}

fn sample_proof() -> [u8; PROOF_LEN] {
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    secret(&a, &b)
        .proof(
            Role::Sharer,
            [1u8; 16],
            [2u8; 16],
            &a.public_key_bytes(),
            &b.public_key_bytes(),
        )
        .encode()
}

#[test]
fn truncated_or_oversized_frames_are_rejected() {
    let hello = sample_hello();
    for len in 1..HELLO_LEN {
        assert!(
            matches!(Frame::decode(&hello[..len]), Err(FrameError::Length)),
            "hello truncated to {len} decoded"
        );
    }
    let proof = sample_proof();
    for len in HELLO_LEN + 1..PROOF_LEN {
        assert!(
            matches!(Frame::decode(&proof[..len]), Err(FrameError::Length)),
            "proof truncated to {len} decoded"
        );
    }
    let mut long = proof.to_vec();
    long.push(0);
    assert!(matches!(Frame::decode(&long), Err(FrameError::Length)));
}

#[test]
fn length_must_match_the_kind() {
    // A Hello header on a Proof-sized body and vice versa.
    let mut hello_as_proof = sample_proof();
    hello_as_proof[2] = 0x01;
    assert!(matches!(
        Frame::decode(&hello_as_proof),
        Err(FrameError::Length)
    ));
    let mut proof_as_hello = sample_hello();
    proof_as_hello[2] = 0x02;
    assert!(matches!(
        Frame::decode(&proof_as_hello),
        Err(FrameError::Length)
    ));
}

#[test]
fn unsupported_version_is_rejected() {
    for ver in [0u8, 1, 0xFF] {
        let mut h = sample_hello();
        h[1] = ver;
        assert!(matches!(Frame::decode(&h), Err(FrameError::Version(v)) if v == ver));
        let mut p = sample_proof();
        p[1] = ver;
        assert!(matches!(Frame::decode(&p), Err(FrameError::Version(v)) if v == ver));
    }
}

#[test]
fn unknown_kind_is_rejected() {
    for kind in [0u8, 5, 0xFF] {
        let mut h = sample_hello();
        h[2] = kind;
        assert!(matches!(Frame::decode(&h), Err(FrameError::Kind(k)) if k == kind));
    }
}

#[test]
fn unknown_role_is_rejected() {
    let mut h = sample_hello();
    h[3] = 2;
    assert!(matches!(Frame::decode(&h), Err(FrameError::Role(2))));
}

#[test]
fn other_frame_types_are_not_known_frames() {
    // 0x01 discovery (49 B) and 0x02 handshake (65 B) share the port.
    let mut disc = [0u8; 49];
    disc[0] = 0x01;
    let mut hs = [0u8; 65];
    hs[0] = 0x02;
    assert!(matches!(Frame::decode(&disc), Err(FrameError::NotKnown)));
    assert!(matches!(Frame::decode(&hs), Err(FrameError::NotKnown)));
    assert!(matches!(Frame::decode(&[]), Err(FrameError::NotKnown)));
}

#[test]
fn decoded_hello_still_needs_its_tag() {
    // decode() is syntax only; a garbage Hello decodes but never verifies.
    let mut wire = sample_hello();
    wire[20..].fill(0);
    let Frame::Hello(h) = Frame::decode(&wire).unwrap() else {
        panic!("expected Hello");
    };
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    assert!(!h.verify(&secret(&a, &b)));
}

// ---- nothing sensitive on the wire -----------------------------------------

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn two_hundred_frames_leak_no_keys_ids_or_secret_and_never_repeat() {
    use std::collections::HashSet;

    let a = KeyPair::generate();
    let b = KeyPair::generate();
    let (pk_a, pk_b) = (a.public_key_bytes(), b.public_key_bytes());
    // Fixed, known S so the test can search for its bytes on the wire.
    let s = PairSecret::from_bytes([0x5A; 32]);
    let id = s.pair_id();
    // The pair id as raw bytes too (hex-decode of the ASCII id).
    let id_raw: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap())
        .collect();

    let needles: Vec<(&str, Vec<u8>)> = vec![
        ("sharer pk", pk_a.to_vec()),
        ("joiner pk", pk_b.to_vec()),
        ("sharer pk b64", a.public_key_b64().into_bytes()),
        ("joiner pk b64", b.public_key_b64().into_bytes()),
        ("S", vec![0x5A; 32]),
        ("pairId ascii", id.clone().into_bytes()),
        ("pairId raw", id_raw),
    ];

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut hellos: Vec<Vec<u8>> = Vec::new();
    for i in 0..200 {
        // Hellos (alternating roles, fresh nonce each) and Proofs.
        let role = if i % 2 == 0 {
            Role::Sharer
        } else {
            Role::Joiner
        };
        let hello = s.hello(role, rand::random()).encode().to_vec();
        hellos.push(hello.clone());
        frames.push(hello);
        frames.push(
            s.proof(role, rand::random(), rand::random(), &pk_a, &pk_b)
                .encode()
                .to_vec(),
        );
    }

    assert!(frames.len() >= 200);
    for frame in &frames {
        for (what, needle) in &needles {
            assert!(!contains(frame, needle), "frame leaks {what}");
        }
    }
    // Unlinkable across runs: no 16-byte window of any Hello (after the 4
    // byte header) repeats in another Hello.
    let mut seen: HashSet<[u8; 16]> = HashSet::new();
    for hello in &hellos {
        let mut mine: HashSet<[u8; 16]> = HashSet::new();
        for w in hello[4..].windows(16) {
            let mut k = [0u8; 16];
            k.copy_from_slice(w);
            assert!(!seen.contains(&k), "16-byte window repeated across Hellos");
            mine.insert(k);
        }
        seen.extend(mine);
    }
}

#[test]
fn pair_secret_debug_does_not_print_the_secret() {
    let s = PairSecret::from_bytes([0xAB; 32]);
    assert_eq!(format!("{s:?}"), "PairSecret(<redacted>)");
}
