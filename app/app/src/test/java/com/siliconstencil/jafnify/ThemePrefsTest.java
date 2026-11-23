package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.util.HashMap;
import java.util.Map;

/** Theme persistence, using a plain HashMap instead of SharedPreferences. */
public class ThemePrefsTest {
    /** The test double for SharedPreferences: a plain map. */
    private static final class MapStore implements ThemePrefs.Store {
        final Map<String, String> values = new HashMap<>();

        @Override
        public String get(String key) {
            return values.get(key);
        }

        @Override
        public void put(String key, String value) {
            values.put(key, value);
        }
    }

    @Test
    public void an_empty_store_means_system() {
        assertEquals(ThemeMode.SYSTEM, ThemePrefs.load(new MapStore()));
    }

    @Test
    public void dark_and_light_round_trip() {
        MapStore store = new MapStore();
        ThemePrefs.save(store, ThemeMode.DARK);
        assertEquals(ThemeMode.DARK, ThemePrefs.load(store));
        ThemePrefs.save(store, ThemeMode.LIGHT);
        assertEquals(ThemeMode.LIGHT, ThemePrefs.load(store));
    }

    @Test
    public void system_round_trips_too() {
        MapStore store = new MapStore();
        ThemePrefs.save(store, ThemeMode.SYSTEM);
        assertEquals(ThemeMode.SYSTEM, ThemePrefs.load(store));
    }

    @Test
    public void an_invalid_stored_value_means_system() {
        MapStore store = new MapStore();
        store.put(ThemePrefs.PREF_KEY, "sepia");
        assertEquals(ThemeMode.SYSTEM, ThemePrefs.load(store));
    }

    @Test
    public void save_writes_the_lowercase_key_under_a_stable_preference_key() {
        MapStore store = new MapStore();
        ThemePrefs.save(store, ThemeMode.DARK);
        assertEquals("theme_mode", ThemePrefs.PREF_KEY);
        assertEquals(1, store.values.size());
        assertEquals("dark", store.values.get(ThemePrefs.PREF_KEY));
        ThemePrefs.save(store, ThemeMode.LIGHT);
        assertEquals("light", store.values.get(ThemePrefs.PREF_KEY));
    }
}
