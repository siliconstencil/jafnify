//! The laptop's home layout, kept in `home-layout.toml` next to `state.toml`.
//!
//! The shape and the validation live in `jafnify_core::home_layout`; this
//! module owns the private file. It is a separate file on purpose: a build
//! that does not know the layout never reads or writes it, so a downgrade is
//! lossless. The layout is local only: it is never sent to the peer.

use std::fs;
use std::io::Write;
use std::path::Path;

use jafnify_core::home_layout::{Catalog, Layout, Platform, FILE_NAME};

/// The laptop's placeable tiles.
pub fn catalog() -> Catalog {
    jafnify_core::home_layout::catalog(Platform::Linux)
}

/// The default layout of the laptop.
pub fn default_layout() -> Layout {
    Layout::default_for(Platform::Linux)
}

/// The stored layout. A missing, unreadable or corrupt file is the default;
/// the loaded layout is normalized, so a hand-made file cannot leave a hole.
pub fn load(path: &Path) -> Layout {
    let Ok(text) = fs::read_to_string(path) else {
        return default_layout();
    };
    Layout::parse(&text)
        .unwrap_or_else(default_layout)
        .normalize(Platform::Linux)
}

/// The columns of the tile grid.
pub const COLUMNS: usize = 2;

/// The height of one tile row, in points. Every row of every grid uses it, so
/// the tiles in a row and the rows of the grid line up; AVERAGE is no taller
/// than its neighbour. It is the smallest a row may be: a taller tile raises
/// the whole grid so no row ever overlaps the next.
pub const TILE_HEIGHT: f32 = 152.0;

/// The narrowest a column may become before the window stops shrinking.
pub const MIN_COLUMN_WIDTH: f32 = 120.0;

/// One row of the grid: the slots it holds and whether it is the wide row of
/// a 7 grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowPlan {
    pub start: usize,
    pub end: usize,
    pub wide: bool,
}

impl RowPlan {
    /// The number of tiles on the row.
    pub fn slots(&self) -> usize {
        self.end - self.start
    }
}

/// The height one tile needs for its six stacked widgets at the font metrics
/// of the window: the label row, the value row, two gaps and the trend, the
/// facts and the note on the caption line, the item spacing between them and
/// the frame on both sides.
pub fn tile_height(
    label_row: f32,
    value_row: f32,
    trend_row: f32,
    fact_lines: usize,
    gap: f32,
    item_gap: f32,
    margin: f32,
) -> f32 {
    let text = label_row + value_row + 2.0 * gap + trend_row + fact_lines as f32 * label_row;
    text + 5.0 * item_gap + 2.0 * margin
}

/// The one height every row of the grid is drawn at: the tallest tile, and
/// never less than [TILE_HEIGHT].
pub fn grid_row_height(needs: &[f32]) -> f32 {
    needs.iter().copied().fold(TILE_HEIGHT, f32::max)
}

/// The rows a grid of `tiles` tiles needs. A 7 grid gives the wide slot a row
/// of its own, at the top or the bottom; every other row holds at most two.
pub fn row_plans(layout: &Layout, tiles: usize) -> Vec<RowPlan> {
    let wide = layout.wide_slot();
    let mut rows = Vec::new();
    let mut index = 0;
    while index < tiles {
        let is_wide = wide == Some(index);
        let end = if is_wide {
            index + 1
        } else {
            let mut end = (index + COLUMNS).min(tiles);
            if let Some(wide_slot) = wide {
                if wide_slot > index && wide_slot < end {
                    end = wide_slot;
                }
            }
            end
        };
        rows.push(RowPlan {
            start: index,
            end,
            wide: is_wide,
        });
        index = end;
    }
    rows
}

/// The width of one column in a row of two tiles, with `gap` between them.
pub fn column_width(full: f32, gap: f32) -> f32 {
    ((full - gap) / COLUMNS as f32)
        .floor()
        .max(MIN_COLUMN_WIDTH)
}

/// The width of one tile: both columns and the gap for the wide tile, one
/// column otherwise.
pub fn row_width(wide: bool, full: f32, gap: f32) -> f32 {
    if wide {
        full
    } else {
        column_width(full, gap)
    }
}

/// Store the layout atomically (temp + fsync + rename), 0600.
pub fn save(path: &Path, layout: &Layout) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        crate::settings::ensure_private_dir(parent)?;
    }
    let text = layout.normalize(Platform::Linux).to_text();
    let tmp = path.with_file_name(format!(".{FILE_NAME}.tmp"));
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    {
        let mut f = jafnify_core::private_fs::open_private_file(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jafnify_core::home_layout::{Grid, WideAt};

    fn layout_of(platform: Platform, grid: Grid, wide: WideAt) -> Layout {
        let mut layout = Layout::default_for(platform);
        layout.grid = grid;
        layout.wide = wide;
        layout.normalize(platform)
    }

    #[test]
    fn every_row_of_every_grid_and_catalog_is_drawn_at_the_grid_height() {
        let need = tile_height(13.2, 26.4, 16.0, 3, 4.0, 3.0, 12.0);
        for platform in [Platform::Android, Platform::Linux] {
            for grid in Grid::ALL {
                for wide in [WideAt::Top, WideAt::Bottom] {
                    let layout = layout_of(platform, grid, wide);
                    let rows = row_plans(&layout, layout.slots.len());
                    assert!(!rows.is_empty());
                    let height = grid_row_height(&[need]);
                    assert_eq!(
                        height, TILE_HEIGHT,
                        "a full tile fits the minimum row height"
                    );
                    // Every row takes the same height, whatever its tiles need.
                    assert_eq!(grid_row_height(&[need, 10.0, 90.0]), height);
                }
            }
        }
    }

    #[test]
    fn a_full_tile_fits_the_minimum_height() {
        // Font row heights close to the real egui metrics: caption 11 and a
        // monospace number at the title size, two gaps, the trend line, two
        // facts and a note, the item spacing between the six widgets, and the
        // frame on both sides.
        let need = tile_height(13.2, 26.4, 16.0, 3, 4.0, 3.0, 12.0);
        assert!(
            need <= TILE_HEIGHT,
            "a full tile needs {need} points but a row is {TILE_HEIGHT}"
        );
    }

    #[test]
    fn the_grid_height_is_the_tallest_tile_never_below_the_minimum() {
        assert_eq!(grid_row_height(&[90.0, 160.0, 120.0]), 160.0);
        assert_eq!(grid_row_height(&[10.0]), TILE_HEIGHT);
        assert_eq!(grid_row_height(&[]), TILE_HEIGHT);
    }

    #[test]
    fn every_row_holds_two_tiles_except_the_one_wide_row() {
        for platform in [Platform::Linux, Platform::Android] {
            for grid in Grid::ALL {
                for wide in [WideAt::Top, WideAt::Bottom] {
                    let layout = layout_of(platform, grid, wide);
                    let rows = row_plans(&layout, layout.slots.len());
                    let wide_slot = layout.wide_slot();
                    let mut slots = 0;
                    for row in &rows {
                        assert!(row.end > row.start);
                        if wide_slot == Some(row.start) {
                            assert!(row.wide, "the wide slot has its own row");
                            assert_eq!(row.slots(), 1);
                        } else {
                            assert!(!row.wide);
                            assert!(row.slots() <= 2);
                        }
                        slots += row.slots();
                    }
                    assert_eq!(slots, layout.slots.len(), "every tile is placed once");
                }
            }
        }
    }

    #[test]
    fn both_columns_are_equal_and_the_wide_row_spans_them() {
        let full = 600.0;
        let gap = 8.0;
        let column = column_width(full, gap);
        assert!((column * 2.0 + gap - full).abs() < 0.001);
        // A wide tile is as wide as both columns and the gap between them.
        assert!((row_width(true, full, gap) - full).abs() < 0.001);
        assert!((row_width(false, full, gap) - column).abs() < 0.001);
        // A very narrow window never collapses a column.
        assert!(column_width(100.0, gap) >= 120.0);
        // A width that does not divide evenly still gives two equal columns.
        assert_eq!(column_width(601.0, gap), column_width(601.0, gap));
        assert!(column_width(601.0, gap) * 2.0 + gap <= 601.0);
    }

    #[test]
    fn a_seven_grid_wide_row_is_first_or_last() {
        for wide in [WideAt::Top, WideAt::Bottom] {
            let layout = layout_of(Platform::Linux, Grid::Seven, wide);
            let rows = row_plans(&layout, layout.slots.len());
            let wide_index = rows.iter().position(|row| row.wide).expect("a wide row");
            if wide == WideAt::Top {
                assert_eq!(wide_index, 0);
            } else {
                assert_eq!(wide_index, rows.len() - 1);
            }
        }
    }
}
