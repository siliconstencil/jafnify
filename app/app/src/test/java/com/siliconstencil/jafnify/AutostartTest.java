package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.HashMap;
import java.util.Map;

/** The autostart decision and its persistence, using a plain HashMap. */
public class AutostartTest {
    /** The test double for SharedPreferences: a plain map. */
    private static final class MapStore implements Autostart.Store {
        final Map<String, Boolean> values = new HashMap<>();

        @Override
        public boolean getBoolean(String key, boolean def) {
            Boolean stored = values.get(key);
            return stored == null ? def : stored;
        }

        @Override
        public void putBoolean(String key, boolean value) {
            values.put(key, value);
        }
    }

    @Test
    public void an_empty_store_means_off() {
        assertFalse(Autostart.load(new MapStore()));
    }

    @Test
    public void on_and_off_round_trip() {
        MapStore store = new MapStore();
        Autostart.save(store, true);
        assertTrue(Autostart.load(store));
        Autostart.save(store, false);
        assertFalse(Autostart.load(store));
    }

    @Test
    public void the_setting_is_one_boolean_under_a_stable_key() {
        MapStore store = new MapStore();
        Autostart.save(store, true);
        assertEquals("start_on_open", Autostart.PREF_KEY);
        assertEquals(1, store.values.size());
        assertEquals(Boolean.TRUE, store.values.get(Autostart.PREF_KEY));
    }

    @Test
    public void an_empty_store_leaves_resume_off() {
        assertFalse(Autostart.loadResume(new MapStore()));
    }

    @Test
    public void the_resume_setting_round_trips_under_its_own_key() {
        MapStore store = new MapStore();
        Autostart.saveResume(store, true);
        assertTrue(Autostart.loadResume(store));
        assertEquals("resume_if_stopped", Autostart.RESUME_KEY);
        assertEquals(Boolean.TRUE, store.values.get(Autostart.RESUME_KEY));
        assertEquals(1, store.values.size());
    }

    @Test
    public void an_empty_store_leaves_start_after_restart_off() {
        assertFalse(Autostart.loadStartAfterRestart(new MapStore()));
    }

    @Test
    public void the_start_after_restart_key_is_the_core_id() {
        MapStore store = new MapStore();
        store.putBoolean(Autostart.START_AFTER_RESTART_KEY, true);
        assertTrue(Autostart.loadStartAfterRestart(store));
        assertEquals("start_after_restart", Autostart.START_AFTER_RESTART_KEY);
    }
}
