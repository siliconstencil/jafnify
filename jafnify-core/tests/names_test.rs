//! Pair fingerprint names: two words from fixed lists, derived from the pair
//! id, plus the upgrade of legacy key-prefix names. No keys here are real.

use std::collections::HashSet;
use std::fs;

use jafnify_core::known::pair_record;
use jafnify_core::names::{
    display_name, pair_words, role_label, upgrade_legacy_name, upgrade_legacy_names, ADJECTIVES,
    NOUNS,
};
use jafnify_core::state::{Peer, Role, StateStore, TomlStateStore};
use jafnify_core::KeyPair;
use sha2::{Digest, Sha256};

const PAIR_ID: &str = "0123456789abcdef0123456789abcdef";

fn legacy_peer() -> Peer {
    let public_key = "EXAMPLEKEYAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string();
    Peer {
        id: PAIR_ID.to_string(),
        name: public_key.chars().take(12).collect(),
        public_key,
        role: Role::Sharer,
        last_connected: Some(1),
        private_key: Some("EXAMPLE-not-a-real-private-key".to_string()),
        last_endpoint: None,
    }
}

// ---- word lists ------------------------------------------------------------

#[test]
fn both_lists_hold_64_words() {
    assert_eq!(ADJECTIVES.len(), 64);
    assert_eq!(NOUNS.len(), 64);
}

#[test]
fn every_word_is_capitalised_ascii_letters() {
    for w in ADJECTIVES.iter().chain(NOUNS.iter()) {
        assert!(w.len() >= 3, "too short: {w}");
        assert!(
            w.chars().all(|c| c.is_ascii_alphabetic()),
            "not ascii letters: {w}"
        );
        assert!(
            w.chars().next().unwrap().is_ascii_uppercase(),
            "not capitalised: {w}"
        );
        assert!(
            w.chars().skip(1).all(|c| c.is_ascii_lowercase()),
            "not lowercase tail: {w}"
        );
    }
}

#[test]
fn no_word_appears_twice_in_either_list_or_in_both() {
    let adjectives: HashSet<_> = ADJECTIVES.iter().collect();
    let nouns: HashSet<_> = NOUNS.iter().collect();
    assert_eq!(adjectives.len(), 64, "duplicate adjective");
    assert_eq!(nouns.len(), 64, "duplicate noun");
    assert!(adjectives.is_disjoint(&nouns), "a word is in both lists");
}

// ---- pair_words ------------------------------------------------------------

#[test]
fn pair_words_is_deterministic() {
    assert_eq!(pair_words(PAIR_ID), pair_words(PAIR_ID));
}

#[test]
fn pair_words_follow_sha256_of_the_pair_id() {
    let d = Sha256::digest(PAIR_ID.as_bytes());
    let expect = format!(
        "{} {}",
        ADJECTIVES[(d[0] & 63) as usize],
        NOUNS[(d[1] & 63) as usize]
    );
    assert_eq!(pair_words(PAIR_ID), expect);
}

// Literal values computed independently of this code (SHA-256 of the ASCII
// pair id; byte 0 & 63 -> ADJECTIVES, byte 1 & 63 -> NOUNS). They pin the
// word lists and their order: editing or reordering a list breaks this test.
#[test]
fn pair_words_match_independently_computed_values() {
    assert_eq!(
        pair_words("00112233445566778899aabbccddeeff"),
        "Ivory Tiger"
    );
    assert_eq!(
        pair_words("0123456789abcdef0123456789abcdef"),
        "Young Beacon"
    );
}

#[test]
fn pair_words_is_one_adjective_then_one_noun() {
    let words = pair_words(PAIR_ID);
    let parts: Vec<&str> = words.split(' ').collect();
    assert_eq!(parts.len(), 2, "{words}");
    assert!(ADJECTIVES.contains(&parts[0]));
    assert!(NOUNS.contains(&parts[1]));
}

#[test]
fn different_pair_ids_mostly_give_different_words() {
    let names: HashSet<String> = (0..200).map(|i| pair_words(&format!("pair-{i}"))).collect();
    // 4096 combinations: 200 samples collide a few times at most.
    assert!(names.len() > 180, "only {} distinct of 200", names.len());
}

// ---- display_name ----------------------------------------------------------

#[test]
fn role_labels_are_phone_for_sharer_and_laptop_for_joiner() {
    assert_eq!(role_label(Role::Sharer), "Phone");
    assert_eq!(role_label(Role::Joiner), "Laptop");
}

#[test]
fn display_name_is_role_dash_words() {
    let words = pair_words(PAIR_ID);
    assert_eq!(
        display_name(Role::Sharer, PAIR_ID),
        format!("Phone - {words}")
    );
    assert_eq!(
        display_name(Role::Joiner, PAIR_ID),
        format!("Laptop - {words}")
    );
}

#[test]
fn both_sides_of_a_pairing_show_the_same_words_and_the_other_role() {
    let (phone, laptop) = (KeyPair::generate(), KeyPair::generate());
    // The phone is the sharer; its record describes the laptop (joiner).
    let on_phone = pair_record(&phone, &laptop.public_key_b64(), Role::Sharer, None, 1).unwrap();
    let on_laptop = pair_record(&laptop, &phone.public_key_b64(), Role::Joiner, None, 1).unwrap();

    let words = pair_words(&on_phone.id);
    assert_eq!(on_phone.id, on_laptop.id);
    assert_eq!(on_phone.name, format!("Laptop - {words}"));
    assert_eq!(on_laptop.name, format!("Phone - {words}"));
}

// ---- legacy migration ------------------------------------------------------

#[test]
fn legacy_key_prefix_name_is_replaced_by_the_display_name() {
    let mut p = legacy_peer();
    assert!(upgrade_legacy_name(&mut p));
    assert_eq!(p.name, display_name(Role::Sharer, PAIR_ID));
}

#[test]
fn upgrade_is_idempotent() {
    let mut p = legacy_peer();
    assert!(upgrade_legacy_name(&mut p));
    let once = p.clone();
    assert!(!upgrade_legacy_name(&mut p));
    assert_eq!(p, once);
}

#[test]
fn a_custom_name_is_left_alone() {
    let mut p = legacy_peer();
    p.name = "My phone".to_string();
    assert!(!upgrade_legacy_name(&mut p));
    assert_eq!(p.name, "My phone");
}

#[test]
fn an_unpaired_record_is_left_alone() {
    let mut p = legacy_peer();
    p.private_key = None;
    let before = p.clone();
    assert!(!upgrade_legacy_name(&mut p));
    assert_eq!(p, before);
}

#[test]
fn upgrade_legacy_names_reports_whether_any_peer_changed() {
    let mut peers = vec![legacy_peer()];
    assert!(upgrade_legacy_names(&mut peers));
    assert!(!upgrade_legacy_names(&mut peers));
    assert!(!upgrade_legacy_names(&mut []));
}

#[test]
fn state_store_load_upgrades_a_legacy_name_without_rewriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let store = TomlStateStore::new(path.clone());
    let legacy = legacy_peer();
    store.save(std::slice::from_ref(&legacy)).unwrap();
    let on_disk = fs::read_to_string(&path).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded[0].name, display_name(Role::Sharer, PAIR_ID));
    assert_eq!(loaded[0].public_key, legacy.public_key);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        on_disk,
        "load must not write"
    );

    // Saving after the upgrade persists the new name, and it then stays put.
    store.save(&loaded).unwrap();
    assert_eq!(store.load().unwrap(), loaded);
}
