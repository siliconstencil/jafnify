//! The home layout: how many tile slots the home has (6, 7 or 8), where the
//! one wide slot of a 7 grid sits, and which tile occupies each slot.
//!
//! The model is pure. A platform hands in its catalog of placeable tiles and
//! gets back a normalized layout: unknown ids dropped, duplicates removed,
//! too few slots filled from the platform default, too many cut. The default
//! layout is today's screen, with the new average tile appended, so an update
//! keeps every tile where it was.
//!
//! The saved text is plain TOML, parsed here so Windows and iOS reuse the
//! same rules. A missing, corrupt or partial file is the platform default.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::tiles::{BigValue, Facts, Tile, Tone};

/// File name under the Jafnify config directory on the laptop. The phone
/// stores the same text in its preferences.
pub const FILE_NAME: &str = "home-layout.toml";

/// The most slots a home may have.
pub const MAX_SLOTS: usize = 8;

/// A platform whose catalog gives the placeable tiles and the default order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

/// One placeable tile: the core id and the word the picker shows.
pub type CatalogEntry = (&'static str, &'static str);

/// A catalog is a fixed table, in the default order.
pub type Catalog = &'static [CatalogEntry];

/// The phone's placeable tiles, in today's order plus the new average tile.
pub const ANDROID_CATALOG: Catalog = &[
    ("signal", "SIGNAL"),
    ("dns", "DNS"),
    ("heat", "HEAT"),
    ("battery", "BATTERY"),
    ("range", "RANGE"),
    ("speed", "SPEED"),
    ("today", "TODAY"),
    ("avg", "AVERAGE"),
];

/// The laptop's placeable tiles, in today's order plus the new average tile.
pub const LINUX_CATALOG: Catalog = &[
    ("dns", "DNS ANSWER"),
    ("signal", "SIGNAL"),
    ("phone", "PHONE"),
    ("wifi", "WI-FI LEG"),
    ("range", "RANGE"),
    ("today", "TODAY"),
    ("drops", "DROPS"),
    ("avg", "AVERAGE"),
];

/// The catalog of a platform.
pub fn catalog(platform: Platform) -> Catalog {
    match platform {
        Platform::Android => ANDROID_CATALOG,
        Platform::Linux => LINUX_CATALOG,
    }
}

/// The catalog entry of an id, if the platform may place it.
pub fn entry(platform: Platform, id: &str) -> Option<CatalogEntry> {
    catalog(platform)
        .iter()
        .copied()
        .find(|(candidate, _)| *candidate == id)
}

/// How many tile slots the grid has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grid {
    Six,
    Seven,
    Eight,
}

impl Grid {
    /// The three choices, in order.
    pub const ALL: [Grid; 3] = [Grid::Six, Grid::Seven, Grid::Eight];

    /// The number of slots the grid has.
    pub fn slots(self) -> usize {
        match self {
            Grid::Six => 6,
            Grid::Seven => 7,
            Grid::Eight => 8,
        }
    }

    /// The stable word of the choice.
    pub fn key(self) -> &'static str {
        match self {
            Grid::Six => "6",
            Grid::Seven => "7",
            Grid::Eight => "8",
        }
    }

    /// The choice of a word. A number out of range is not a grid.
    pub fn parse(word: &str) -> Option<Grid> {
        match word.trim() {
            "6" => Some(Grid::Six),
            "7" => Some(Grid::Seven),
            "8" => Some(Grid::Eight),
            _ => None,
        }
    }
}

/// Where the one wide slot of a 7 grid sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WideAt {
    #[default]
    Top,
    Bottom,
}

impl WideAt {
    /// The stable word of the choice.
    pub fn key(self) -> &'static str {
        match self {
            WideAt::Top => "top",
            WideAt::Bottom => "bottom",
        }
    }

    /// The choice of a word; an unknown word is the default, top.
    pub fn parse(word: &str) -> WideAt {
        match word.trim().to_ascii_lowercase().as_str() {
            "bottom" => WideAt::Bottom,
            _ => WideAt::Top,
        }
    }

    /// The slot index of the wide tile in a 7 grid.
    pub fn slot(self) -> usize {
        match self {
            WideAt::Top => 0,
            WideAt::Bottom => 6,
        }
    }
}

/// The layout of the home: the grid, the wide placement and one tile id per
/// slot, in the order the user wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub grid: Grid,
    #[serde(default)]
    pub wide: WideAt,
    pub slots: Vec<String>,
}

impl Layout {
    /// The default of a platform: today's screen plus the average tile.
    pub fn default_for(platform: Platform) -> Layout {
        Layout {
            grid: Grid::Eight,
            wide: WideAt::Top,
            slots: catalog(platform)
                .iter()
                .map(|(id, _)| id.to_string())
                .collect(),
        }
    }

    /// The number of slots this layout asks for.
    pub fn slot_count(&self) -> usize {
        self.grid.slots()
    }

    /// The layout with only placeable ids, no duplicates, exactly as many
    /// slots as the grid has, filled from the catalog when needed.
    pub fn normalize(&self, platform: Platform) -> Layout {
        let catalog = catalog(platform);
        let allowed: BTreeSet<&str> = catalog.iter().map(|(id, _)| *id).collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut slots = Vec::with_capacity(self.slot_count());
        for id in &self.slots {
            if slots.len() >= self.slot_count() {
                break;
            }
            if allowed.contains(id.as_str()) && seen.insert(id.clone()) {
                slots.push(id.clone());
            }
        }
        for (id, _) in catalog {
            if slots.len() >= self.slot_count() {
                break;
            }
            if seen.insert((*id).to_string()) {
                slots.push((*id).to_string());
            }
        }
        // A catalog shorter than the grid repeats the default order, so a
        // slot is never left without an id; today both catalogs hold 8.
        let mut index = 0;
        while slots.len() < self.slot_count() && !catalog.is_empty() {
            let id = catalog[index % catalog.len()].0.to_string();
            slots.push(id);
            index += 1;
        }
        Layout {
            grid: self.grid,
            wide: self.wide,
            slots,
        }
    }

    /// Whether a layout is already normalized for a platform.
    pub fn is_normalized(&self, platform: Platform) -> bool {
        *self == self.normalize(platform)
    }

    /// The slot index the wide tile has, when the grid has one.
    pub fn wide_slot(&self) -> Option<usize> {
        (self.grid == Grid::Seven).then(|| self.wide.slot())
    }

    /// Parse the saved text. A missing, corrupt or partial file has no layout
    /// and the caller uses the platform default. Unknown keys are ignored.
    pub fn parse(text: &str) -> Option<Layout> {
        let table: toml::Table = text.parse().ok()?;
        let grid = table
            .get("grid")
            .and_then(toml::Value::as_integer)
            .and_then(|grid| Grid::parse(&grid.to_string()))?;
        let wide = table
            .get("wide")
            .and_then(toml::Value::as_str)
            .map(WideAt::parse)
            .unwrap_or_default();
        let slots = table
            .get("slots")
            .and_then(toml::Value::as_array)
            .map(|slots| {
                slots
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Some(Layout { grid, wide, slots })
    }

    /// The saved text, as the store writes it.
    pub fn to_text(&self) -> String {
        format!(
            "grid = {}\nwide = \"{}\"\nslots = [{}]\n",
            self.grid.slots(),
            self.wide.key(),
            self.slots
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

/// The muted tile a slot shows when its data is missing: a "--" and one short
/// fact, never an empty hole and never a stale value.
pub fn placeholder_tile(platform: Platform, id: &str) -> Tile {
    let label = entry(platform, id)
        .map(|(_, label)| label)
        .unwrap_or("TILE");
    Tile {
        id: id.to_string(),
        label: label.to_string(),
        badge: None,
        big: Some(BigValue {
            value: "--".to_string(),
            unit: None,
            tone: Tone::Muted,
        }),
        trend: None,
        facts: Some(Facts::new(vec!["No data yet".to_string()])),
        note: None,
    }
}

/// The tiles in the order of the layout, one tile per slot. A slot whose
/// tile the caller did not hand in shows the muted "--" tile. A tile the
/// layout does not place is left out.
pub fn arrange(layout: &Layout, platform: Platform, tiles: Vec<Tile>) -> Vec<Tile> {
    let layout = layout.normalize(platform);
    let mut available: Vec<Option<Tile>> = tiles.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(layout.slots.len());
    for id in &layout.slots {
        let found = available
            .iter_mut()
            .find(|slot| slot.as_ref().map(|tile| tile.id == *id).unwrap_or(false));
        match found.and_then(Option::take) {
            Some(tile) => out.push(tile),
            None => out.push(placeholder_tile(platform, id)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::ID_DNS;

    fn ids(layout: &Layout) -> Vec<&str> {
        layout.slots.iter().map(String::as_str).collect()
    }

    #[test]
    fn a_default_layout_is_todays_screen_plus_the_average_tile() {
        let phone = Layout::default_for(Platform::Android);
        assert_eq!(phone.grid, Grid::Eight);
        assert_eq!(
            ids(&phone),
            vec!["signal", "dns", "heat", "battery", "range", "speed", "today", "avg"]
        );
        let laptop = Layout::default_for(Platform::Linux);
        assert_eq!(
            ids(&laptop),
            vec!["dns", "signal", "phone", "wifi", "range", "today", "drops", "avg"]
        );
        assert!(phone.is_normalized(Platform::Android));
        assert!(laptop.is_normalized(Platform::Linux));
    }

    #[test]
    fn the_grids_are_six_seven_and_eight_slots() {
        assert_eq!(Grid::parse("6"), Some(Grid::Six));
        assert_eq!(Grid::parse("7"), Some(Grid::Seven));
        assert_eq!(Grid::parse("8"), Some(Grid::Eight));
        assert_eq!(Grid::parse("9"), None);
        assert_eq!(Grid::Six.slots(), 6);
        assert_eq!(Grid::Seven.slots(), 7);
        assert_eq!(Grid::Eight.slots(), 8);
        assert_eq!(Grid::Seven.key(), "7");
    }

    #[test]
    fn only_the_seven_grid_has_a_wide_slot() {
        let mut layout = Layout::default_for(Platform::Android);
        assert_eq!(layout.wide_slot(), None);
        layout.grid = Grid::Seven;
        layout.wide = WideAt::Top;
        assert_eq!(layout.wide_slot(), Some(0));
        layout.wide = WideAt::Bottom;
        assert_eq!(layout.wide_slot(), Some(6));
        assert_eq!(WideAt::parse("bottom"), WideAt::Bottom);
        assert_eq!(WideAt::parse("nonsense"), WideAt::Top);
    }

    #[test]
    fn unknown_ids_are_dropped_and_duplicates_removed() {
        let layout = Layout {
            grid: Grid::Eight,
            wide: WideAt::Top,
            slots: vec![
                "dns".to_string(),
                "bogus".to_string(),
                "dns".to_string(),
                "today".to_string(),
            ],
        };
        let normalized = layout.normalize(Platform::Android);
        assert_eq!(normalized.slots.len(), 8);
        assert_eq!(normalized.slots[0], "dns");
        assert_eq!(normalized.slots[1], "today");
        assert!(!normalized.slots.iter().any(|id| id == "bogus"));
        // The fill follows the catalog and skips what is already there.
        assert_eq!(normalized.slots[2], "signal");
        assert_eq!(normalized.slots[3], "heat");
    }

    #[test]
    fn too_few_slots_are_filled_from_the_default_and_too_many_are_cut() {
        let mut layout = Layout::default_for(Platform::Linux);
        layout.grid = Grid::Six;
        layout.slots.truncate(2);
        layout.slots[0] = "drops".to_string();
        layout.slots[1] = "dns".to_string();
        let normalized = layout.normalize(Platform::Linux);
        assert_eq!(normalized.slots.len(), 6);
        assert_eq!(normalized.slots[0], "drops");
        assert_eq!(normalized.slots[1], "dns");
        assert_eq!(normalized.slots[2], "signal");

        let mut too_many = Layout::default_for(Platform::Linux);
        too_many
            .slots
            .extend(["signal".to_string(), "dns".to_string()]);
        assert_eq!(too_many.normalize(Platform::Linux).slots.len(), 8);
    }

    #[test]
    fn an_id_of_the_other_platform_is_not_placeable() {
        let layout = Layout {
            grid: Grid::Six,
            wide: WideAt::Top,
            slots: vec!["phone".to_string()],
        };
        let normalized = layout.normalize(Platform::Android);
        assert!(!normalized.slots.iter().any(|id| id == "phone"));
    }

    #[test]
    fn the_saved_text_round_trips_and_a_corrupt_text_is_none() {
        let mut layout = Layout::default_for(Platform::Linux);
        layout.grid = Grid::Seven;
        layout.wide = WideAt::Bottom;
        let text = layout.to_text();
        let parsed = Layout::parse(&text).expect("the saved text parses");
        let expected = layout.normalize(Platform::Linux);
        assert_eq!(parsed.normalize(Platform::Linux), expected);
        assert_eq!(expected.slots.len(), 7);
        assert_eq!(expected.wide, WideAt::Bottom);
        // Partial and corrupt text is no layout at all, never a crash.
        assert!(Layout::parse("not [valid toml").is_none());
        assert!(Layout::parse("grid = 5\n").is_none());
        let partial = Layout::parse("grid = 7\nslots = [\"dns\"]\n").expect("grid is enough");
        let normalized = partial.normalize(Platform::Linux);
        assert_eq!(normalized.grid, Grid::Seven);
        assert_eq!(normalized.slots[0], "dns");
        assert_eq!(normalized.slots.len(), 7);
    }

    #[test]
    fn a_layout_from_before_the_change_still_loads_and_keeps_the_order() {
        // No file at all means the platform default, exactly today's screen.
        let default = Layout::default_for(Platform::Android);
        assert_eq!(default.slots[0], "signal");
        // A hand-made file naming only some tiles keeps them in front.
        let saved = Layout::parse("grid = 8\nslots = [\"today\", \"dns\"]\n").unwrap();
        let normalized = saved.normalize(Platform::Android);
        assert_eq!(&normalized.slots[..2], ["today", "dns"]);
        assert_eq!(normalized.slots.len(), 8);
    }

    #[test]
    fn the_arrangement_puts_the_tiles_in_slot_order() {
        let layout = Layout {
            grid: Grid::Six,
            wide: WideAt::Top,
            slots: vec![
                "today".to_string(),
                "dns".to_string(),
                "signal".to_string(),
                "heat".to_string(),
                "battery".to_string(),
                "speed".to_string(),
            ],
        };
        let dns = Tile {
            id: ID_DNS.to_string(),
            label: "DNS".to_string(),
            badge: None,
            big: Some(BigValue {
                value: "20".to_string(),
                unit: Some("ms".to_string()),
                tone: Tone::Muted,
            }),
            trend: None,
            facts: None,
            note: None,
        };
        let arranged = arrange(&layout, Platform::Android, vec![dns]);
        assert_eq!(arranged.len(), 6);
        assert_eq!(arranged[0].id, "today");
        assert_eq!(arranged[0].big.as_ref().unwrap().value, "--");
        assert_eq!(arranged[1].id, "dns");
        assert_eq!(arranged[1].big.as_ref().unwrap().value, "20");
        assert_eq!(arranged[2].id, "signal");
        assert_eq!(
            arranged[2].facts.as_ref().unwrap().lines,
            vec!["No data yet"]
        );
    }

    #[test]
    fn a_tile_the_layout_does_not_place_is_left_out() {
        let layout = Layout {
            grid: Grid::Six,
            wide: WideAt::Top,
            slots: vec![
                "dns".to_string(),
                "today".to_string(),
                "signal".to_string(),
                "heat".to_string(),
                "battery".to_string(),
                "range".to_string(),
            ],
        };
        let extra = Tile {
            id: "speed".to_string(),
            label: "SPEED".to_string(),
            badge: None,
            big: None,
            trend: None,
            facts: Some(Facts::new(vec!["x".to_string()])),
            note: None,
        };
        let arranged = arrange(&layout, Platform::Android, vec![extra]);
        assert_eq!(arranged.len(), 6);
        assert!(!arranged.iter().any(|tile| tile.id == "speed"));
    }
}
