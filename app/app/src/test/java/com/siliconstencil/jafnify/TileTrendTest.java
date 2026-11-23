package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The geometry of the small drawings under the numbers of the tiles. Pure
 * Java: no Android classes, so the shapes can be checked without a device.
 */
public class TileTrendTest {
    @Test
    public void an_empty_line_has_no_points() {
        assertEquals(0, TileTrend.linePoints(new int[0], 100f, 40f).length);
        assertEquals(0, TileTrend.linePoints(null, 100f, 40f).length);
    }

    @Test
    public void one_point_is_one_point() {
        float[] points = TileTrend.linePoints(new int[] { 5 }, 100f, 40f);
        assertEquals(2, points.length);
    }

    @Test
    public void many_points_keep_the_line_short() {
        int[] values = new int[500];
        for (int i = 0; i < values.length; i++) {
            values[i] = i;
        }
        float[] points = TileTrend.linePoints(values, 100f, 40f);
        assertTrue(points.length <= 120);
        assertTrue(points.length >= 4);
    }

    @Test
    public void the_line_stays_inside_the_box() {
        float[] points = TileTrend.linePoints(new int[] { 10, 20, 30 }, 100f, 40f);
        for (int i = 0; i < points.length; i += 2) {
            assertTrue(points[i] >= 0f && points[i] <= 100f);
            assertTrue(points[i + 1] >= 0f && points[i + 1] <= 40f);
        }
    }

    @Test
    public void the_reference_line_sits_inside_the_range() {
        // The line runs from 10 to 30; the threshold of 20 is in the middle.
        assertEquals(20f, TileTrend.referenceY(new int[] { 10, 30 }, 20, 40f), 0.01f);
        // A threshold above the line sits at the top.
        assertEquals(0f, TileTrend.referenceY(new int[] { 10, 30 }, 50, 40f), 0.01f);
        // A threshold below the line sits at the bottom.
        assertEquals(40f, TileTrend.referenceY(new int[] { 10, 30 }, 5, 40f), 0.01f);
    }

    @Test
    public void the_bar_is_a_percentage() {
        assertEquals(50, TileTrend.barPercent(50, 100));
        assertEquals(0, TileTrend.barPercent(0, 100));
        assertEquals(0, TileTrend.barPercent(5, 0));
        assertEquals(100, TileTrend.barPercent(120, 100));
    }

    @Test
    public void the_levels_count_the_filled_bars() {
        // One entry per level; the filled ones come first.
        assertEquals(4, TileTrend.levelBars(2, 4).length);
        assertEquals(4, TileTrend.levelBars(4, 4).length);
        assertEquals(4, TileTrend.levelBars(0, 4).length);
        assertTrue(TileTrend.levelBars(2, 4)[0]);
        assertTrue(TileTrend.levelBars(2, 4)[1]);
        assertTrue(!TileTrend.levelBars(2, 4)[2]);
        assertTrue(!TileTrend.levelBars(0, 4)[0]);
    }

    @Test
    public void theSplitBarGivesTheDownPartItsShare() {
        float[] half = TileTrend.splitParts(50, 50, 200f);
        assertEquals(100f, half[0], 0.01f);
        assertEquals(200f, half[1], 0.01f);
    }

    @Test
    public void aSplitBarWithNoUpPartIsAllDown() {
        float[] down = TileTrend.splitParts(100, 0, 200f);
        assertEquals(200f, down[0], 0.01f);
        assertEquals(200f, down[1], 0.01f);
    }

    @Test
    public void aSplitBarWithNoDownPartIsAllUp() {
        float[] up = TileTrend.splitParts(0, 100, 200f);
        assertEquals(0f, up[0], 0.01f);
        assertEquals(200f, up[1], 0.01f);
    }

    @Test
    public void anEmptySplitBarHasNoWidth() {
        float[] none = TileTrend.splitParts(0, 0, 200f);
        assertEquals(0f, none[1], 0.01f);
    }
}
