package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

/** The battery help labels and the no-exemption promise. */
public class BatteryHelpTest {
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
        throw new AssertionError("not found: " + relative);
    }

    private static String read(String relative) {
        try {
            return new String(Files.readAllBytes(locate(relative).toPath()),
                    StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + relative, e);
        }
    }

    @Test
    public void the_two_states_have_distinct_labels() {
        assertNotEquals(BatteryHelp.stateRes(true), BatteryHelp.stateRes(false));
        assertTrue(BatteryHelp.stateRes(true) != 0);
        assertTrue(BatteryHelp.stateRes(false) != 0);
    }

    @Test
    public void the_strings_are_the_two_fixed_words() {
        String xml = read("app/app/src/main/res/values/strings.xml");
        assertTrue(xml.contains(">Unrestricted<"));
        assertTrue(xml.contains(">Optimised<"));
    }

    @Test
    public void the_app_never_requests_the_battery_exemption() {
        String manifest = read("app/app/src/main/AndroidManifest.xml");
        assertFalse("no exemption permission",
                manifest.contains("REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"));
        String activity = read("app/app/src/main/java/com/siliconstencil/jafnify/SettingsActivity.java");
        assertFalse("no exemption request action",
                activity.contains("ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"));
        assertTrue("the app details page is the action",
                activity.contains("ACTION_APPLICATION_DETAILS_SETTINGS"));
    }
}
