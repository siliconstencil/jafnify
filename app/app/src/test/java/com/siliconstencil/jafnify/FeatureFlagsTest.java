package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** The six-field flagsMeta line parser, on a plain JVM. */
public class FeatureFlagsTest {
    private static final String WIRED_ANDROID =
            "notify_connection_change\tadvanced\tfalse\t1"
            + "\tNotify when the connection changes"
            + "\tShows a desktop notification when the connection changes.";

    private static final String BUCKETS =
            "settings\tSettings\t0\t\n"
            + "advanced\tAdvanced\t1\t\n"
            + "experimental\tExperimental\t1\tMay change or be removed.";

    private static final String ONE_PER_BUCKET =
            "settings_row\tsettings\tfalse\t1\tSettings row\tSettings help.\n"
            + "advanced_row\tadvanced\tfalse\t1\tAdvanced row\tAdvanced help.\n"
            + "experimental_row\texperimental\tfalse\t1\tExperimental row"
            + "\tExperimental help.";

    /** The test double for the pure store interface: a plain map. */
    private static final class MapStore implements Autostart.Store {
        final Map<String, Boolean> values = new HashMap<>();
        int writes = 0;

        @Override
        public boolean getBoolean(String key, boolean def) {
            Boolean stored = values.get(key);
            return stored == null ? def : stored;
        }

        @Override
        public void putBoolean(String key, boolean value) {
            writes++;
            values.put(key, value);
        }

        @Override
        public boolean contains(String key) {
            return values.containsKey(key);
        }
    }

    @Test
    public void a_well_formed_line_parses() {
        List<FeatureFlags.Flag> flags = FeatureFlags.parse(WIRED_ANDROID);
        assertEquals(1, flags.size());
        FeatureFlags.Flag flag = flags.get(0);
        assertEquals("notify_connection_change", flag.id);
        assertEquals(FeatureFlags.ADVANCED, flag.bucket);
        assertFalse(flag.defaultOn);
        assertEquals(1, flag.platforms);
        assertEquals("Notify when the connection changes", flag.label);
        assertEquals("Shows a desktop notification when the connection changes.",
                flag.help);
        assertTrue(FeatureFlags.isKnownBucket(flag.bucket));
    }

    @Test
    public void malformed_lines_are_skipped_and_a_valid_line_still_parses() {
        String meta = String.join("\n",
                "too\tfew\tfields",
                "bad_bool\tsettings\tmaybe\t1\tLabel\tHelp.",
                "bad_int\tsettings\tfalse\tone\tLabel\tHelp.",
                "\tsettings\tfalse\t1\tLabel\tHelp.",
                "no_label\tsettings\tfalse\t1\t\tHelp.",
                "no_help\tsettings\tfalse\t1\tLabel\t",
                WIRED_ANDROID);
        List<FeatureFlags.Flag> flags = FeatureFlags.parse(meta);
        assertEquals(1, flags.size());
        assertEquals("notify_connection_change", flags.get(0).id);
    }

    @Test
    public void an_unknown_bucket_is_skipped() {
        String meta = "hidden\thidden\tfalse\t1\tHidden\tHidden.\n" + WIRED_ANDROID;
        List<FeatureFlags.Flag> flags = FeatureFlags.parse(meta);
        assertEquals(1, flags.size());
        assertEquals(FeatureFlags.ADVANCED, flags.get(0).bucket);
    }

    @Test
    public void empty_input_has_no_rows() {
        assertTrue(FeatureFlags.parse(null).isEmpty());
        assertTrue(FeatureFlags.parse("").isEmpty());
        assertTrue(FeatureFlags.parse("\n").isEmpty());
    }

    @Test
    public void the_platform_filter_hides_a_linux_only_row() {
        String meta = String.join("\n",
                "android_row\tsettings\tfalse\t1\tAndroid row\tAndroid help.",
                "linux_row\tadvanced\tfalse\t2\tLinux row\tLinux help.");
        List<FeatureFlags.Flag> flags = FeatureFlags.parse(meta);
        assertEquals(2, flags.size());
        List<FeatureFlags.Flag> android = FeatureFlags.forPlatform(flags, 1);
        assertEquals(1, android.size());
        assertEquals("android_row", android.get(0).id);
        List<FeatureFlags.Flag> linux = FeatureFlags.forPlatform(flags, 2);
        assertEquals(1, linux.size());
        assertEquals("linux_row", linux.get(0).id);
    }

    @Test
    public void an_empty_meta_has_no_rows() {
        // The producer omits unwired rows; an empty table is still valid.
        assertTrue(FeatureFlags.parse("").isEmpty());
    }

    @Test
    public void two_rows_keep_the_table_order() {
        String meta = WIRED_ANDROID
                + "\nsecond\texperimental\ttrue\t3\tSecond\tSecond help.";
        List<FeatureFlags.Flag> flags = FeatureFlags.parse(meta);
        assertEquals(2, flags.size());
        assertEquals("notify_connection_change", flags.get(0).id);
        assertEquals("second", flags.get(1).id);
        assertTrue(flags.get(1).defaultOn);
        assertEquals(3, flags.get(1).platforms);
    }

    @Test
    public void bucket_lines_parse_in_core_order() {
        List<FeatureFlags.BucketInfo> buckets = FeatureFlags.parseBuckets(BUCKETS);
        assertEquals(3, buckets.size());
        assertEquals(FeatureFlags.SETTINGS, buckets.get(0).key);
        assertEquals("Settings", buckets.get(0).title);
        assertFalse(buckets.get(0).collapsedByDefault);
        assertNull(buckets.get(0).note);
        assertTrue(buckets.get(1).collapsedByDefault);
        assertNull(buckets.get(1).note);
        assertTrue(buckets.get(2).collapsedByDefault);
        assertEquals("May change or be removed.", buckets.get(2).note);
    }

    @Test
    public void malformed_bucket_lines_are_skipped() {
        String meta = "too\tfew\n"
                + "unknown\tUnknown\t0\t\n"
                + "\tEmpty\t0\t\n"
                + "settings\t\t0\t\n"
                + "settings\tSettings\tmaybe\t\n"
                + BUCKETS;
        List<FeatureFlags.BucketInfo> buckets = FeatureFlags.parseBuckets(meta);
        assertEquals(3, buckets.size());
        assertEquals(FeatureFlags.EXPERIMENTAL, buckets.get(2).key);
    }

    @Test
    public void an_empty_table_adds_no_section() {
        assertTrue(FeatureFlags.screen("", BUCKETS, new MapStore()).isEmpty());
    }

    @Test
    public void one_row_per_bucket_follows_the_placement() {
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(ONE_PER_BUCKET, BUCKETS, new MapStore());
        assertEquals(3, sections.size());
        assertEquals(FeatureFlags.Placement.MAIN, sections.get(0).placement);
        assertEquals("Settings", sections.get(0).title);
        assertFalse(sections.get(0).collapsedByDefault);
        assertNull(sections.get(0).note);
        assertEquals(FeatureFlags.Placement.GROUP, sections.get(1).placement);
        assertEquals("Advanced", sections.get(1).title);
        assertTrue(sections.get(1).collapsedByDefault);
        assertNull(sections.get(1).note);
        assertEquals(FeatureFlags.Placement.GROUP, sections.get(2).placement);
        assertEquals("Experimental", sections.get(2).title);
        assertTrue(sections.get(2).collapsedByDefault);
        assertEquals("May change or be removed.", sections.get(2).note);
    }

    @Test
    public void a_group_with_no_row_is_not_added() {
        String meta = "settings_row\tsettings\tfalse\t1\tSettings row\tSettings help.";
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(meta, BUCKETS, new MapStore());
        assertEquals(1, sections.size());
        assertEquals(FeatureFlags.Placement.MAIN, sections.get(0).placement);
    }

    @Test
    public void an_empty_store_leaves_every_row_off() {
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(ONE_PER_BUCKET, BUCKETS, new MapStore());
        for (FeatureFlags.Section section : sections) {
            for (FeatureFlags.Row row : section.rows) {
                assertFalse(row.id, row.value);
            }
        }
    }

    @Test
    public void a_missing_key_reads_the_core_default() {
        String meta = "default_on\tsettings\ttrue\t1\tDefault on\tDefault on help.";
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(meta, BUCKETS, new MapStore());
        assertTrue(sections.get(0).rows.get(0).value);
    }

    @Test
    public void a_stored_value_wins_and_unknown_keys_are_untouched() {
        MapStore store = new MapStore();
        store.values.put("settings_row", true);
        store.values.put("unknown_key", true);
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(ONE_PER_BUCKET, BUCKETS, store);
        assertTrue(sections.get(0).rows.get(0).value);
        assertEquals(0, store.writes);
        assertTrue(store.values.get("unknown_key"));
    }

    @Test
    public void the_resume_row_reads_its_existing_key() {
        String meta = "resume_if_stopped\tsettings\tfalse\t1"
                + "\tResume sharing if the system stops the app"
                + "\tStarts sharing again after the system stops the app.";
        MapStore store = new MapStore();
        store.values.put("resume_if_stopped", true);
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(meta, BUCKETS, store);
        assertTrue(sections.get(0).rows.get(0).value);
    }

    @Test
    public void an_unknown_bucket_row_is_not_rendered() {
        String meta = "orphan\thidden\tfalse\t1\tOrphan\tOrphan help.";
        assertTrue(FeatureFlags.screen(meta, BUCKETS, new MapStore()).isEmpty());
    }

    @Test
    public void the_merged_network_row_keeps_an_old_switch_on() {
        String meta = "network_details\tsettings\tfalse\t1"
                + "\tNetwork details\tAdds the network details.";
        MapStore store = new MapStore();
        store.values.put("signal_history", true);
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(meta, BUCKETS, store);
        assertTrue(sections.get(0).rows.get(0).value);
    }

    @Test
    public void a_new_network_choice_wins_over_the_old_keys() {
        String meta = "network_details\tsettings\tfalse\t1"
                + "\tNetwork details\tAdds the network details.";
        MapStore store = new MapStore();
        store.values.put("network_report", true);
        store.values.put("network_details", false);
        List<FeatureFlags.Section> sections =
                FeatureFlags.screen(meta, BUCKETS, store);
        assertFalse(sections.get(0).rows.get(0).value);
    }
}
