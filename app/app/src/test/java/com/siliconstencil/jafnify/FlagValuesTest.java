package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

/** The effective value of a flag: standard rows on, the merged row migrated. */
public class FlagValuesTest {
    private static final class MapStore implements Autostart.Store {
        final java.util.Map<String, Boolean> values = new java.util.HashMap<>();

        @Override
        public boolean getBoolean(String key, boolean def) {
            Boolean stored = values.get(key);
            return stored == null ? def : stored;
        }

        @Override
        public void putBoolean(String key, boolean value) {
            values.put(key, value);
        }

        @Override
        public boolean contains(String key) {
            return values.containsKey(key);
        }
    }

    private static Set<String> standard(String... ids) {
        return new HashSet<>(Arrays.asList(ids));
    }

    @Test
    public void a_standard_row_is_on_even_when_an_old_value_says_off() {
        MapStore store = new MapStore();
        store.values.put("tile_home", false);
        assertTrue(FlagValues.enabled(standard("tile_home"), store, "tile_home"));
        assertTrue(FlagValues.enabled(standard("phone_stats"), store, "phone_stats"));
        assertFalse(FlagValues.enabled(Collections.<String>emptySet(), store,
                "tile_home"));
    }

    @Test
    public void a_setting_reads_the_stored_choice_and_defaults_off() {
        MapStore store = new MapStore();
        Set<String> standard = standard("phone_stats");
        assertFalse(FlagValues.enabled(standard, store, "data_budget"));
        store.values.put("data_budget", true);
        assertTrue(FlagValues.enabled(standard, store, "data_budget"));
    }

    @Test
    public void the_merged_row_keeps_either_old_switch_on() {
        boolean[][] table = {
            // hasNew, newValue, oldReport, oldSignal, expected
            {false, false, false, false, false},
            {false, false, false, true, true},
            {false, false, true, false, true},
            {false, false, true, true, true},
            {true, false, false, false, false},
            {true, false, true, true, false},
            {true, true, false, false, true},
            {true, true, true, true, true},
        };
        for (boolean[] row : table) {
            String where = Arrays.toString(row);
            org.junit.Assert.assertEquals(where, row[4],
                    FlagValues.mergedNetworkDetails(row[0], row[1], row[2], row[3]));
        }
    }

    @Test
    public void a_new_choice_beats_the_old_keys() {
        MapStore store = new MapStore();
        store.values.put("network_report", true);
        store.values.put("network_details", false);
        assertFalse(FlagValues.networkDetails(store));
        store.values.put("network_details", true);
        assertTrue(FlagValues.networkDetails(store));
    }

    @Test
    public void no_store_has_no_merged_row() {
        assertFalse(FlagValues.networkDetails(null));
    }
}
