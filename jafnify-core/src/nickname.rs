//! The name this device calls itself. It is local: the peer never reads it.

/// The longest name a device may carry.
pub const MAX_LEN: usize = 32;

/// Clean a user-entered name: control characters removed, the rest trimmed,
/// and at most [`MAX_LEN`] characters kept. Any printable character is kept.
pub fn clean(input: &str) -> String {
    let printable: String = input.chars().filter(|c| !c.is_control()).collect();
    let trimmed = printable.trim();
    trimmed.chars().take(MAX_LEN).collect()
}
