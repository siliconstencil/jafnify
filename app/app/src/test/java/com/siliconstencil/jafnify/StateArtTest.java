package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Every state is shown with its own glyph name and a shape, never with colour
 * alone. The names come from jafnify-core's snapshot::state_glyph vocabulary;
 * this test mirrors that vocabulary and proves the drawables exist.
 */
public class StateArtTest {
    private static final List<String> CORE_SHAPES = Arrays.asList(
            "no-arc-hollow-dot",
            "one-arc-hollow-dot",
            "one-arc-filled-dot",
            "two-arcs-filled-dot",
            "one-arc-hollow-dot-slash",
            "exclamation");

    private static File resDir() {
        File dir = new File(System.getProperty("user.dir", ".")).getAbsoluteFile();
        for (int i = 0; i < 10 && dir != null; i++) {
            for (String prefix : new String[] {"", "app/app/"}) {
                File found = new File(dir, prefix + "src/main/res");
                if (found.exists()) {
                    return found;
                }
            }
            dir = dir.getParentFile();
        }
        throw new AssertionError("cannot find src/main/res");
    }

    @Test
    public void everyStateHasItsOwnGlyphNameAndAShape() {
        Set<String> names = new HashSet<>();
        for (UiText.Status status : new UiText.Status[] {
                UiText.Status.IDLE, UiText.Status.SEARCHING, UiText.Status.PAIRING,
                UiText.Status.CONNECTING, UiText.Status.CONNECTED,
                UiText.Status.RETRYING}) {
            StateArt art = StateArt.of(status);
            assertNotNull(status + " has an icon", art);
            assertTrue(status + " has a glyph name", !art.glyphName.isEmpty());
            assertTrue(status + " has a core shape name",
                    CORE_SHAPES.contains(art.shapeName));
            assertTrue(status + " is not colour alone", art.glyphName != null);
            assertNotEquals(status + " is not bare colour", art.glyphName, art.shapeName);
            assertTrue(status + " has a distinct glyph name", names.add(art.glyphName));
        }

        StateArt error = StateArt.error();
        assertEquals("error", error.glyphName);
        assertEquals("exclamation", error.shapeName);
        assertTrue("the error glyph is distinct", names.add(error.glyphName));
    }

    @Test
    public void everyGlyphHasADrawableNamedAfterItsCoreShape() {
        File drawableDir = new File(resDir(), "drawable");
        for (UiText.Status status : UiText.Status.values()) {
            StateArt art = StateArt.of(status);
            assertEquals(status + " drawable follows the shape name",
                    "ic_state_" + art.shapeName.replace('-', '_'), art.drawableName);
            assertTrue(art.drawableName + " exists",
                    new File(drawableDir, art.drawableName + ".xml").exists());
        }
        assertTrue("the error drawable exists",
                new File(drawableDir, StateArt.error().drawableName + ".xml").exists());
    }

    @Test
    public void theErrorGlyphScalesWithTheText() {
        // The same helper sizes both the error and warning glyphs.
        int small = UiText.statusGlyphSizePx(17f);
        int large = UiText.statusGlyphSizePx(17f * 2f);
        assertEquals(20, small);
        assertEquals(39, large);
        assertTrue("the icon bounds grow with the text size", large > small);
        assertTrue("the icon grows with the font scale", large >= 2 * small - 2);
        assertEquals("a tiny text still gets a visible icon", 16,
                UiText.statusGlyphSizePx(4f));
    }
}
