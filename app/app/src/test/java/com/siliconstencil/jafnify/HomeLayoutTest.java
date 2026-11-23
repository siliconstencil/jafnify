package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.Arrays;

/** The saved home layout text: parse, defaults and the round trip. */
public class HomeLayoutTest {
    private static final String SEVEN =
            "grid = 7\nwide = \"bottom\"\nslots = [\"signal\", \"dns\"]\n";

    @Test
    public void a_saved_text_round_trips() {
        HomeLayout layout = HomeLayout.parse(SEVEN);
        assertEquals(7, layout.grid);
        assertEquals(HomeLayout.BOTTOM, layout.wide);
        assertEquals(Arrays.asList("signal", "dns"), layout.slots);
        assertEquals(SEVEN, layout.toText());
    }

    @Test
    public void a_seven_grid_has_one_wide_slot() {
        assertEquals(0, HomeLayout.parse(
                "grid = 7\nwide = \"top\"\nslots = []\n").wideSlot());
        assertEquals(6, HomeLayout.parse(SEVEN).wideSlot());
        assertEquals(-1, HomeLayout.parse(
                "grid = 8\nwide = \"top\"\nslots = []\n").wideSlot());
        assertEquals(-1, HomeLayout.parse(
                "grid = 6\nwide = \"top\"\nslots = []\n").wideSlot());
    }

    @Test
    public void a_partial_text_keeps_the_defaults() {
        HomeLayout layout = HomeLayout.parse("grid = 6\n");
        assertEquals(6, layout.grid);
        assertEquals(HomeLayout.TOP, layout.wide);
        assertTrue(layout.slots.isEmpty());
        assertEquals(6, layout.slotCount());
    }

    @Test
    public void a_bad_grid_falls_back_to_eight_and_a_bad_wide_to_top() {
        HomeLayout layout = HomeLayout.parse(
                "grid = 9\nwide = \"sideways\"\nslots = [\"dns\"]\n");
        assertEquals(8, layout.grid);
        assertEquals(HomeLayout.TOP, layout.wide);
        assertEquals(Arrays.asList("dns"), layout.slots);
    }

    @Test
    public void null_and_empty_have_no_slots() {
        for (String text : new String[] {null, ""}) {
            HomeLayout layout = HomeLayout.parse(text);
            assertEquals(8, layout.grid);
            assertEquals(HomeLayout.TOP, layout.wide);
            assertTrue(layout.slots.isEmpty());
        }
    }

    @Test
    public void a_saved_text_is_exact() {
        HomeLayout layout = new HomeLayout();
        layout.grid = 8;
        layout.slots.add("avg");
        layout.slots.add("today");
        assertEquals("grid = 8\nwide = \"top\"\nslots = [\"avg\", \"today\"]\n",
                layout.toText());
    }
}
