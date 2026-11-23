package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.attribute.PosixFilePermission;
import java.util.List;
import java.util.Set;

/** The phone usage file: atomic, private, capped and tolerant of old lines. */
public class UsageLineStoreTest {
    private static final long HOUR = 3_600_000L;

    private static File newFile() {
        File dir = new File("build/test-tmp/h15-store-" + System.nanoTime());
        assertTrue(dir.mkdirs() || dir.isDirectory());
        return new File(dir, "phone-stats.tsv");
    }

    private static String line(long hour) {
        return "JFU1 1 hour=" + hour + " up=1 down=2 min_up=3 min_down=4"
                + " peak_up=5 peak_down=6 connected_ms=7 samples=8 net=wifi";
    }

    @Test
    public void two_hours_round_trip_in_time_order() {
        File file = newFile();
        UsageLineStore store = new UsageLineStore(file);
        store.upsert(line(1_700_000_000_000L + HOUR));
        store.upsert(line(1_700_000_000_000L));

        List<String> lines = store.load();
        assertEquals(2, lines.size());
        assertTrue(lines.get(0).contains("hour=1700000000000 "));
        assertTrue(lines.get(1).contains("hour=1700003600000 "));
    }

    @Test
    public void a_second_bucket_for_the_same_hour_replaces_the_first() {
        File file = newFile();
        UsageLineStore store = new UsageLineStore(file);
        store.upsert(line(1_700_000_000_000L));
        store.upsert(line(1_700_000_000_000L).replace("up=1", "up=99"));

        List<String> lines = store.load();
        assertEquals(1, lines.size());
        assertTrue(lines.get(0).contains("up=99"));
    }

    @Test
    public void the_cap_keeps_the_newest_720_hours() {
        File file = newFile();
        UsageLineStore store = new UsageLineStore(file);
        for (int i = 0; i <= 720; i++) {
            store.upsert(line(1_700_000_000_000L + i * HOUR));
        }
        List<String> lines = store.load();
        assertEquals(720, lines.size());
        assertTrue(lines.get(0).contains("hour=1700003600000 "));
        assertTrue(lines.get(719).contains("hour=1702592000000 "));
    }

    @Test
    public void a_corrupt_file_loads_empty_and_a_write_recovers_it() {
        File file = newFile();
        try (Writer out = new OutputStreamWriter(
                new FileOutputStream(file), StandardCharsets.UTF_8)) {
            out.write("not a usage line\n");
        } catch (Exception e) {
            throw new AssertionError(e);
        }
        UsageLineStore store = new UsageLineStore(file);
        assertTrue(store.load().isEmpty());
        store.upsert(line(1_700_000_000_000L));
        assertEquals(1, store.load().size());
    }

    @Test
    public void an_old_line_without_min_fields_is_preserved() {
        File file = newFile();
        String old = "JFU1 1 hour=1700000000000 up=1 down=2 peak_up=3 peak_down=4"
                + " connected_ms=5 samples=6 net=wifi";
        try (Writer out = new OutputStreamWriter(
                new FileOutputStream(file), StandardCharsets.UTF_8)) {
            out.write(old + "\n");
        } catch (Exception e) {
            throw new AssertionError(e);
        }
        UsageLineStore store = new UsageLineStore(file);
        store.upsert(line(1_700_000_000_000L + HOUR));

        List<String> lines = store.load();
        assertEquals(2, lines.size());
        assertEquals(old, lines.get(0));
    }

    @Test
    public void a_budget_line_survives_a_usage_write() {
        UsageLineStore store = new UsageLineStore(newFile());
        String budget =
                "JFB1 1 allow=10000000000 used=3000000000 reset=12 month=24323 warned=0";
        store.saveBudget(budget);
        store.upsert(line(HOUR));
        assertEquals(budget, store.loadBudget());
        assertEquals(1, store.load().size());
    }

    @Test
    public void saving_the_budget_keeps_every_usage_line() {
        UsageLineStore store = new UsageLineStore(newFile());
        store.upsert(line(2 * HOUR));
        store.upsert(line(HOUR));
        store.saveBudget("JFB1 1 allow=1 used=0 reset=1 month=24323 warned=0");
        assertEquals(2, store.load().size());
        assertEquals("JFB1 1 allow=1 used=0 reset=1 month=24323 warned=0",
                store.loadBudget());
    }

    @Test
    public void a_missing_budget_line_is_null() {
        UsageLineStore store = new UsageLineStore(newFile());
        assertNull(store.loadBudget());
        store.upsert(line(HOUR));
        assertNull(store.loadBudget());
    }

    @Test
    public void the_file_is_owner_only_and_no_temp_file_is_left() throws Exception {
        File file = newFile();
        UsageLineStore store = new UsageLineStore(file);
        store.upsert(line(1_700_000_000_000L));

        Set<PosixFilePermission> mode = Files.getPosixFilePermissions(file.toPath());
        assertFalse(mode.contains(PosixFilePermission.GROUP_READ));
        assertFalse(mode.contains(PosixFilePermission.OTHERS_READ));
        assertFalse(mode.contains(PosixFilePermission.GROUP_WRITE));
        assertFalse(mode.contains(PosixFilePermission.OTHERS_WRITE));
        assertEquals(1, file.getParentFile().listFiles().length);
    }
}
