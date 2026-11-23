package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The framework-only theme choice. Only the pure parts are tested here; the
 * Context wrapping itself needs a device.
 */
public class ThemeModeTest {
    @Test
    public void a_missing_or_unknown_key_defaults_to_system() {
        assertEquals(ThemeMode.SYSTEM, ThemeMode.fromKey(null));
        assertEquals(ThemeMode.SYSTEM, ThemeMode.fromKey(""));
        assertEquals(ThemeMode.SYSTEM, ThemeMode.fromKey("sepia"));
    }

    @Test
    public void keys_are_the_lowercase_names() {
        assertEquals("system", ThemeMode.SYSTEM.key());
        assertEquals("dark", ThemeMode.DARK.key());
        assertEquals("light", ThemeMode.LIGHT.key());
    }

    @Test
    public void from_key_accepts_lowercase_uppercase_and_trimmed() {
        assertEquals(ThemeMode.SYSTEM, ThemeMode.fromKey("system"));
        assertEquals(ThemeMode.DARK, ThemeMode.fromKey("dark"));
        assertEquals(ThemeMode.LIGHT, ThemeMode.fromKey("light"));
        assertEquals(ThemeMode.DARK, ThemeMode.fromKey("DARK"));
        assertEquals(ThemeMode.LIGHT, ThemeMode.fromKey("Light"));
        assertEquals(ThemeMode.DARK, ThemeMode.fromKey("  dark  "));
    }

    @Test
    public void labels_are_the_ui_words() {
        assertEquals("System", ThemeMode.SYSTEM.label());
        assertEquals("Dark", ThemeMode.DARK.label());
        assertEquals("Light", ThemeMode.LIGHT.label());
    }

    @Test
    public void only_explicit_modes_need_an_override() {
        assertFalse(ThemeMode.SYSTEM.requiresOverride());
        assertTrue(ThemeMode.DARK.requiresOverride());
        assertTrue(ThemeMode.LIGHT.requiresOverride());
    }
}
