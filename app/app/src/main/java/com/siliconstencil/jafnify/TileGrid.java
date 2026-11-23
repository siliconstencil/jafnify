package com.siliconstencil.jafnify;

/**
 * The arithmetic of the tile grid: how many rows a number of tiles needs and
 * whether the biggest number fits a column. Pure Java, no Android classes, so
 * the layout can be checked without a device.
 */
public final class TileGrid {
    /** The columns of the grid. */
    public static final int COLUMNS = 2;
    /** How much of its size a monospace glyph takes across. */
    public static final float MONO_ADVANCE = 0.6f;

    private TileGrid() {
    }

    /** The rows a number of tiles needs; the last row may hold one tile. */
    public static int rows(int tiles) {
        if (tiles <= 0) {
            return 0;
        }
        return (tiles + COLUMNS - 1) / COLUMNS;
    }

    /** The first slot of a row. */
    public static int rowStart(int row) {
        return row * COLUMNS;
    }

    /** The slot after the last of a row, bounded by the tiles there are. */
    public static int rowEnd(int row, int slots) {
        return Math.min(slots, rowStart(row) + COLUMNS);
    }

    /** Whether text fits a width in dp at a type size in sp. */
    public static boolean fits(String text, float textSp, int widthDp) {
        if (text == null || text.isEmpty()) {
            return true;
        }
        return text.length() * textSp * MONO_ADVANCE <= widthDp;
    }
}
