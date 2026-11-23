package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.util.Arrays;

/**
 * The marks of an hour strip: the words the core sends and the colours the
 * phone draws. Pure JVM, like {@link PhoneHomeTest}.
 */
public class HourMarksTest {
    @Test
    public void the_word_outside_has_its_own_faint_role() {
        // An hour outside the window of the tile is not a fault: it is
        // drawn fainter than any hour of the window, never like an absence or a drop.
        assertEquals(HourMarks.ROLE_OUTSIDE, HourMarks.roleOf("outside"));
        org.junit.Assert.assertNotEquals(HourMarks.ROLE_MUTED, HourMarks.roleOf("outside"));
    }

    @Test
    public void the_old_words_keep_their_roles() {
        assertEquals(HourMarks.ROLE_OK, HourMarks.roleOf("quick"));
        assertEquals(HourMarks.ROLE_WAIT, HourMarks.roleOf("busy"));
        assertEquals(HourMarks.ROLE_UNKNOWN, HourMarks.roleOf("unknown"));
        assertEquals(HourMarks.ROLE_ERR, HourMarks.roleOf("drop"));
        assertEquals(HourMarks.ROLE_MUTED, HourMarks.roleOf("away"));
    }

    @Test
    public void a_word_the_phone_does_not_know_is_drawn_as_unknown() {
        // A newer core may send a word the phone does not know: it is drawn
        // as unknown, never as a drop.
        assertEquals(HourMarks.ROLE_UNKNOWN, HourMarks.roleOf("brand-new"));
        assertEquals(HourMarks.ROLE_UNKNOWN, HourMarks.roleOf(""));
        assertEquals(HourMarks.ROLE_UNKNOWN, HourMarks.roleOf(null));
    }

    @Test
    public void the_cells_of_a_strip_keep_their_order() {
        String[] roles = HourMarks.roles(Arrays.asList("15:unknown", "16:drop", "17:outside"));
        assertEquals(3, roles.length);
        assertEquals(HourMarks.ROLE_UNKNOWN, roles[0]);
        assertEquals(HourMarks.ROLE_ERR, roles[1]);
        assertEquals(HourMarks.ROLE_OUTSIDE, roles[2]);
    }

    @Test
    public void an_old_text_draws_the_same_as_before() {
        // A text without the new word must not change: the same cells, the
        // same roles.
        String[] roles = HourMarks.roles(Arrays.asList("15:unknown", "16:drop"));
        assertEquals(HourMarks.ROLE_UNKNOWN, roles[0]);
        assertEquals(HourMarks.ROLE_ERR, roles[1]);
    }

    @Test
    public void the_new_word_is_read_from_the_text() {
        String text = "JFH1 1\n"
                + "s | connected=0 | tone=muted | title=Not sharing | sub= | stop=0 | action=Share | notice=\n"
                + "t | id=drops | label=DROPS 24 H | badge= | btone=muted | value=0 | unit="
                + " | tone=muted | kind=hours | points= | ref= | level= | level_max="
                + " | cells=15:outside,16:drop | fact=No drops in 24 h | note=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        PhoneHome.Tile drops = home.tiles.get(0);
        assertEquals(2, drops.cells.size());
        assertEquals("15:outside", drops.cells.get(0));
        assertEquals("16:drop", drops.cells.get(1));
        String[] roles = HourMarks.roles(drops.cells);
        assertEquals(HourMarks.ROLE_OUTSIDE, roles[0]);
        assertEquals(HourMarks.ROLE_ERR, roles[1]);
    }

    @Test
    public void the_cells_of_a_strip_are_placed_side_by_side() {
        // 24 cells over the width: every bar fits and none overlaps.
        float[] bars = TileTrend.cellBars(24, 240f, 2f);
        assertEquals(48, bars.length);
        float right = 0f;
        for (int i = 0; i < 24; i++) {
            float left = bars[i * 2];
            float width = bars[i * 2 + 1];
            assertEquals("the cells keep their order", i == 0 ? 0f : right + 2f, left, 0.001f);
            assertEquals("no cell is empty", true, width > 0f);
            right = left + width;
        }
        assertEquals("the strip fills the width", 240f, right, 0.001f);
    }
}
