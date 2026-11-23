package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Pure JVM palette checks, in the same style as {@link LayoutContractTest}.
 *
 * The dark-theme bug the screenshots showed was a resolution bug, not a colour
 * bug: a colour resource id was passed to obtainStyledAttributes, which expects
 * theme attribute ids, so every programmatic colour silently fell back to
 * black. Light looked fine only because jafnify_fg is near-black there. The
 * first test guards the resolution path; the second proves the night values
 * themselves keep a readable contrast.
 */
public class PaletteTest {
    private static final String[] TEXT_ROLES = {
        "jafnify_fg", "jafnify_muted", "jafnify_wait_text", "jafnify_err", "jafnify_ok",
    };
    private static final String[] BACKGROUNDS = {"jafnify_bg", "jafnify_surface"};

    private static File locate(String relative) {
        File dir = new File(System.getProperty("user.dir", ".")).getAbsoluteFile();
        for (int i = 0; i < 10 && dir != null; i++) {
            for (String prefix : new String[] {"", "app/app/"}) {
                File found = new File(dir, prefix + relative);
                if (found.exists()) {
                    return found;
                }
            }
            dir = dir.getParentFile();
        }
        throw new AssertionError("not found from " + System.getProperty("user.dir")
                + ": " + relative);
    }

    private static String source(String relative) {
        File file = locate(relative);
        try {
            return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    @Test
    public void paletteResolvesColourResourcesNotThemeAttributes() {
        String palette = source("src/main/java/com/siliconstencil/jafnify/Palette.java");
        assertFalse("a colour resource id is not a theme attribute; "
                + "obtainStyledAttributes returns the fallback for it",
                palette.contains(".obtainStyledAttributes("));
        assertTrue("Palette resolves the colour through the resources",
                palette.contains(".getColor("));
    }

    @Test
    public void everyTextRoleKeepsAaContrastInLightAndNight() {
        String[] files = {
            "src/main/res/values/colors.xml",
            "src/main/res/values-night/colors.xml",
        };
        for (String file : files) {
            Map<String, Integer> colours = parseColours(file);
            for (String role : TEXT_ROLES) {
                Integer fg = colours.get(role);
                assertNotNull(file + " defines " + role, fg);
                for (String background : BACKGROUNDS) {
                    Integer bg = colours.get(background);
                    assertNotNull(file + " defines " + background, bg);
                    double ratio = contrast(fg, bg);
                    assertTrue(file + ": " + role + " on " + background
                                    + " is " + String.format("%.2f", ratio) + ":1",
                            ratio >= 4.5);
                }
            }
        }
    }

    private static Map<String, Integer> parseColours(String relative) {
        Map<String, Integer> out = new HashMap<>();
        Pattern p = Pattern.compile(
                "<color name=\"([^\"]+)\">#([0-9A-Fa-f]{6})</color>");
        Matcher m = p.matcher(source(relative));
        while (m.find()) {
            out.put(m.group(1), Integer.parseInt(m.group(2), 16));
        }
        return out;
    }

    /** WCAG 2.x relative luminance of an sRGB colour. */
    private static double luminance(int rgb) {
        double[] channel = new double[3];
        for (int i = 0; i < 3; i++) {
            double c = ((rgb >> (16 - 8 * i)) & 0xFF) / 255.0;
            channel[i] = c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
        }
        return 0.2126 * channel[0] + 0.7152 * channel[1] + 0.0722 * channel[2];
    }

    private static double contrast(int a, int b) {
        double la = luminance(a);
        double lb = luminance(b);
        double hi = Math.max(la, lb);
        double lo = Math.min(la, lb);
        return (hi + 0.05) / (lo + 0.05);
    }
}
