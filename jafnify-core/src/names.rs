//! Readable device names.
//!
//! A pair's display name is the role of the OTHER device plus two words
//! ("Phone - Blue Falcon"). Both devices hold the same pair id, so both derive
//! the same words. Nothing here touches the wire: frames never carry a name.
//!
//! The words are a convenience, NOT a verification. They carry only 12 bits,
//! so an attacker who can grind keys could make both legs show equal words.
//! There is no short-authentication-string check (SECURITY.md, "Accepted risk: the first pairing").

use sha2::{Digest, Sha256};

use crate::state::{Peer, Role};

pub const ADJECTIVES: [&str; 64] = [
    "Amber", "Blue", "Bold", "Brave", "Bright", "Brisk", "Calm", "Clever", "Cloudy", "Coral",
    "Crisp", "Curious", "Daring", "Dusty", "Eager", "Fancy", "Fuzzy", "Gentle", "Golden", "Green",
    "Happy", "Hardy", "Honest", "Humble", "Icy", "Ivory", "Jolly", "Keen", "Kind", "Lively",
    "Lucky", "Lunar", "Mellow", "Mighty", "Mossy", "Misty", "Noble", "Olive", "Orange", "Patient",
    "Plucky", "Polite", "Proud", "Quiet", "Rocky", "Red", "Royal", "Ruby", "Rusty", "Sandy",
    "Silver", "Sleek", "Snowy", "Solar", "Steady", "Sunny", "Swift", "Teal", "Tidy", "Violet",
    "Warm", "Witty", "Young", "Zesty",
];

pub const NOUNS: [&str; 64] = [
    "Falcon", "Otter", "Fox", "Heron", "Lynx", "Panda", "Robin", "Tiger", "Wolf", "Eagle",
    "Badger", "Beaver", "Bison", "Crane", "Dolphin", "Finch", "Gecko", "Hare", "Ibis", "Jaguar",
    "Koala", "Lark", "Lemur", "Llama", "Magpie", "Marten", "Moose", "Newt", "Osprey", "Owl",
    "Parrot", "Pelican", "Puffin", "Quail", "Raven", "Salmon", "Seal", "Sparrow", "Squid", "Stork",
    "Swan", "Tapir", "Turtle", "Walrus", "Whale", "Wren", "Yak", "Zebra", "Anchor", "Beacon",
    "Bridge", "Canyon", "Comet", "Harbor", "Island", "Lantern", "Meadow", "Orchard", "Planet",
    "River", "Summit", "Valley", "Willow", "Compass",
];

/// "Adjective Noun" from SHA-256(pair_id): adjective = byte 0 & 63, noun =
/// byte 1 & 63 (64 divides 256, so there is no modulo bias).
pub fn pair_words(pair_id: &str) -> String {
    let d = Sha256::digest(pair_id.as_bytes());
    format!(
        "{} {}",
        ADJECTIVES[(d[0] & 63) as usize],
        NOUNS[(d[1] & 63) as usize]
    )
}

/// Sharer is the phone, joiner is the laptop.
pub fn role_label(role: Role) -> &'static str {
    match role {
        Role::Sharer => "Phone",
        Role::Joiner => "Laptop",
    }
}

/// `peer_role` is the PEER's role, as in `Peer.role`: "Phone - Blue Falcon" on
/// the laptop, "Laptop - Blue Falcon" on the phone.
pub fn display_name(peer_role: Role, pair_id: &str) -> String {
    format!("{} - {}", role_label(peer_role), pair_words(pair_id))
}

/// Replace a legacy name (the first 12 characters of the peer key) with the
/// display name. Only a paired record (it has a private key, so its `id` is a
/// real pair id) whose name still equals the key prefix is touched; any other
/// name is kept. Returns true if the name changed.
pub fn upgrade_legacy_name(peer: &mut Peer) -> bool {
    let legacy: String = peer.public_key.chars().take(12).collect();
    if peer.private_key.is_none() || peer.name != legacy {
        return false;
    }
    peer.name = display_name(peer.role, &peer.id);
    true
}

/// Apply [`upgrade_legacy_name`] to every peer; true if any changed.
pub fn upgrade_legacy_names(peers: &mut [Peer]) -> bool {
    let mut changed = false;
    for p in peers {
        changed |= upgrade_legacy_name(p);
    }
    changed
}
