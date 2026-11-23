use jafnify_core::nickname::{clean, MAX_LEN};

#[test]
fn control_characters_are_stripped() {
    assert_eq!(clean("Ali\nce\t"), "Alice");
}

#[test]
fn the_name_is_trimmed() {
    assert_eq!(clean("  Bob  "), "Bob");
}

#[test]
fn the_name_is_at_most_thirty_two_characters() {
    let long = "x".repeat(40);
    assert_eq!(clean(&long).chars().count(), MAX_LEN);
}

#[test]
fn printable_unicode_is_kept() {
    assert_eq!(clean("Kai \u{00fc}ber"), "Kai \u{00fc}ber");
}

#[test]
fn an_empty_name_is_allowed() {
    assert_eq!(clean(""), "");
}

#[test]
fn the_parts_the_peer_reads_carry_no_nickname() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in ["snapshot.rs", "config.rs", "wire.rs", "state.rs"] {
        let text = std::fs::read_to_string(src.join(name)).unwrap();
        assert!(!text.contains("nickname"), "{name} mentions the nickname");
    }
}
