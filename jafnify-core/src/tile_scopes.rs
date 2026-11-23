//! The `tile-scopes.toml` format: one `scopes` table of tile id to word.
//!
//! The scopes are local only and are never sent to the peer. A missing,
//! corrupt or partial file is no scope at all, so every tile falls back to its
//! own default. The file is deliberately outside `state.toml`, whose strict
//! `deny_unknown_fields` schema would make an older build set it aside.

use std::collections::BTreeMap;

/// File name under the Jafnify config directory.
pub const FILE_NAME: &str = "tile-scopes.toml";

/// The table the scopes live in.
const TABLE: &str = "scopes";

/// Parse the stored scopes. Non-string values and non-table shapes are ignored.
pub fn parse(text: &str) -> BTreeMap<String, String> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return BTreeMap::new();
    };
    let Some(scopes) = table.get(TABLE).and_then(toml::Value::as_table) else {
        return BTreeMap::new();
    };
    scopes
        .iter()
        .filter_map(|(id, value)| value.as_str().map(|word| (id.clone(), word.to_string())))
        .collect()
}

/// Render the scopes exactly as the store writes them.
pub fn to_text(scopes: &BTreeMap<String, String>) -> std::io::Result<String> {
    let mut inner = toml::Table::new();
    for (id, word) in scopes {
        inner.insert(id.clone(), toml::Value::String(word.clone()));
    }
    let mut table = toml::Table::new();
    table.insert(TABLE.to_string(), toml::Value::Table(inner));
    toml::to_string(&table).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parsed_file_keeps_only_string_words() {
        let text = "[scopes]\ndrops = \"24 H\"\ntoday = \"TODAY\"\nbad = 3\n";
        let scopes = parse(text);
        assert_eq!(scopes.get("drops").map(String::as_str), Some("24 H"));
        assert_eq!(scopes.get("today").map(String::as_str), Some("TODAY"));
        assert!(!scopes.contains_key("bad"));
    }

    #[test]
    fn a_corrupt_file_or_another_table_is_empty() {
        assert!(parse("not [valid toml").is_empty());
        assert!(parse("[other]\ndrops = \"TODAY\"\n").is_empty());
    }

    #[test]
    fn the_rendered_file_round_trips_through_the_parser() {
        let mut scopes = BTreeMap::new();
        scopes.insert("drops".to_string(), "24 H".to_string());
        let text = to_text(&scopes).unwrap();
        assert_eq!(parse(&text), scopes);
        assert!(text.contains("scopes"));
    }
}
