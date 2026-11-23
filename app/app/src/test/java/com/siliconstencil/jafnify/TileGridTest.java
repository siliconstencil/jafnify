package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class TileGridTest {
    @Test
    public void sixTilesUseThreeRowsOfTwo() {
        assertEquals(3, TileGrid.rows(6));
    }

    @Test
    public void fiveTilesUseThreeRows() {
        assertEquals(3, TileGrid.rows(5));
    }

    @Test
    public void fourTilesUseTwoRows() {
        assertEquals(2, TileGrid.rows(4));
    }

    @Test
    public void noTilesUseNoRows() {
        assertEquals(0, TileGrid.rows(0));
    }

    @Test
    public void theSlotBoundsOfARowNeverOverlap() {
        assertEquals(0, TileGrid.rowStart(0));
        assertEquals(2, TileGrid.rowStart(1));
        assertEquals(4, TileGrid.rowStart(2));
        assertEquals(2, TileGrid.rowEnd(0, 6));
        assertEquals(4, TileGrid.rowEnd(1, 6));
        assertEquals(6, TileGrid.rowEnd(2, 6));
        // Five tiles: the last row holds only the fifth.
        assertEquals(5, TileGrid.rowEnd(2, 5));
    }

    @Test
    public void theBiggestNumberFitsTheColumnAtTheTitleSize() {
        // A 360 dp screen: 12 dp of padding on each side, an 8 dp gap, so a
        // column is 164 dp wide; the tile pads in 12 dp on each side.
        int text = (360 - 2 * 12 - 8) / 2 - 2 * 12;
        assertEquals(140, text);
        assertTrue(TileGrid.fits("999.9 MB", 22f, text));
    }

    @Test
    public void theDisplaySizeWouldClipTheBiggestNumber() {
        int text = (360 - 2 * 12 - 8) / 2 - 2 * 12;
        assertFalse(TileGrid.fits("999.9 MB", 34f, text));
    }
}
