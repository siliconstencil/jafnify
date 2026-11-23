package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

/** The Android diagnostic journal: exact UTC lines, redaction, rotation. */
public class DiagLogTest {
    /** 2026-10-31 12:34:56.789 UTC. */
    private static final long T = 1_793_450_096_789L;

    @Rule
    public TemporaryFolder temp = new TemporaryFolder();

    private static String read(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }

    private static String fill(char c, int count) {
        char[] chars = new char[count];
        java.util.Arrays.fill(chars, c);
        return new String(chars);
    }

    @Test
    public void aLineIsUtcIsoWithMillisecondsThenKindThenFieldsInOrder() {
        Map<String, String> fields = new LinkedHashMap<>();
        fields.put("udp_rx_packets", "7");
        fields.put("peer", "10.1.0.2:9797");

        assertEquals(
                "ts=2026-10-31T12:34:56.789Z kind=STATS udp_rx_packets=7 peer=10.1.0.2:9797",
                DiagLog.formatLine(T, "STATS", fields));
        assertEquals(
                "ts=1970-01-01T00:00:00.000Z kind=STATS",
                DiagLog.formatLine(0, "STATS", Collections.<String, String>emptyMap()));
        assertEquals(
                "ts=2026-10-31T12:34:56.007Z kind=STATS",
                DiagLog.formatLine(T - 782L, "STATS", Collections.<String, String>emptyMap()));
        assertFalse(DiagLog.formatLine(T, "STATS", fields).contains("\n"));
    }

    @Test
    public void statsAndEventsAppendOneLineEachAndAnEventFollowsTheKind() throws Exception {
        File dir = temp.newFolder("diag");
        DiagLog log = new DiagLog(dir);

        Map<String, String> stats = new LinkedHashMap<>();
        stats.put("udp_rx_packets", "7");
        stats.put("peer", "10.1.0.2:9797");
        log.stats(T, stats);

        Map<String, String> event = new LinkedHashMap<>();
        event.put("reason", "timeout");
        event.put("worker", "3");
        log.event(T, "peer_reset", event);

        String expected =
                "ts=2026-10-31T12:34:56.789Z kind=STATS udp_rx_packets=7 peer=10.1.0.2:9797\n"
                        + "ts=2026-10-31T12:34:56.789Z kind=EVENT event=peer_reset"
                        + " reason=timeout worker=3\n";
        assertEquals(expected, read(new File(log.dir(), "diag-0.log")));
        assertEquals(dir.getAbsoluteFile(), log.dir());
        log.close();
        assertEquals(expected, read(new File(dir, "diag-0.log")));
    }

    @Test
    public void valuesAreRedactedAndEveryWhitespaceCharacterBecomesUnderscore() {
        String key = fill('A', 43) + "=";
        Map<String, String> fields = new LinkedHashMap<>();
        fields.put("peer", key + " lost\tlink\r\nagain\u2003end");

        String line = DiagLog.formatLine(0, "EVENT", fields);
        assertEquals(
                "ts=1970-01-01T00:00:00.000Z kind=EVENT"
                        + " peer=[redacted]_lost_link__again_end",
                line);
        assertFalse(line.contains("AAAAAAAA"));
        assertFalse(line.contains(key));
        assertFalse(line.contains("\n"));
        assertFalse(line.contains("\r"));
        assertFalse(line.contains("\t"));
    }

    @Test
    public void nonBreakingAndFigureSpacesAreSanitizedToo() {
        Map<String, String> fields = new LinkedHashMap<>();
        fields.put("reason", "a\u00A0b\u2007c");

        assertEquals(
                "ts=1970-01-01T00:00:00.000Z kind=EVENT reason=a_b_c",
                DiagLog.formatLine(0, "EVENT", fields));
    }

    @Test
    public void constructionFailureIsAnApi21SafeIllegalStateException() throws Exception {
        // The parent is a regular file, so creating the journal directory must fail.
        File notADirectory = temp.newFile("blocked");
        File dir = new File(notADirectory, "diag");

        try {
            new DiagLog(dir);
            fail("expected the journal construction to fail");
        } catch (RuntimeException expected) {
            assertEquals(IllegalStateException.class, expected.getClass());
            assertNotNull(expected.getMessage());
            assertTrue(expected.getCause() instanceof IOException);
        }
    }

    @Test
    public void theConstructorCreatesParentDirectoriesAndEveryRecordIsFlushed() throws Exception {
        File dir = new File(temp.getRoot(), "nested/deeper/diag");
        assertFalse(dir.exists());

        DiagLog log = new DiagLog(dir);
        assertTrue(dir.isDirectory());
        log.stats(T, Collections.<String, String>emptyMap());

        File active = new File(dir, "diag-0.log");
        assertTrue(active.isFile());
        String expected = "ts=2026-10-31T12:34:56.789Z kind=STATS\n";
        // Visible while the writer is still open: every record was flushed.
        assertEquals(expected, read(active));
        log.close();
        assertEquals(expected, read(active));
    }

    @Test
    public void rotationKeepsFiveFilesAndTheNextRecordGoesToAFreshActiveFile() throws Exception {
        File dir = temp.newFolder("diag");
        DiagLog log = new DiagLog(dir);
        String filler = fill('x', 16 * 1024);

        int written = 0;
        while (!new File(dir, "diag-4.log").exists()) {
            log.stats(written, Collections.singletonMap("filler", filler));
            written++;
            if (written > 10_000) {
                fail("rotation never reached diag-4.log");
            }
        }

        log.stats(written, Collections.singletonMap("marker", "after_rotation"));
        log.close();

        String active = read(new File(dir, "diag-0.log"));
        assertTrue(active.contains("marker=after_rotation"));
        assertFalse(read(new File(dir, "diag-1.log")).contains("marker=after_rotation"));
        assertFalse(new File(dir, "diag-5.log").exists());

        String[] names = dir.list();
        assertTrue(names != null);
        int logs = 0;
        for (String name : names) {
            if (name.matches("diag-[0-4]\\.log")) {
                logs++;
            }
        }
        assertEquals(DiagLog.KEEP, logs);
        for (int i = 0; i < DiagLog.KEEP; i++) {
            File file = new File(dir, "diag-" + i + ".log");
            assertTrue(file.isFile());
            assertTrue(file.length() <= DiagLog.MAX_BYTES);
        }
    }

    @Test
    public void aRecordLargerThanTwoMebibytesIsWrittenAndTheNextRecordRotatesIt() throws Exception {
        File dir = temp.newFolder("diag");
        DiagLog log = new DiagLog(dir);
        String big = fill('x', (int) DiagLog.MAX_BYTES + 17);

        log.stats(T, Collections.singletonMap("big", big));
        File active = new File(dir, "diag-0.log");
        assertTrue(active.length() > DiagLog.MAX_BYTES);
        assertTrue(read(active).contains("big=" + big));

        log.stats(T + 1L, Collections.singletonMap("after", "rotation"));
        assertEquals(
                "ts=2026-10-31T12:34:56.790Z kind=STATS after=rotation\n",
                read(active));
        File rotated = new File(dir, "diag-1.log");
        assertTrue(rotated.length() > DiagLog.MAX_BYTES);
        assertTrue(read(rotated).contains("big="));
        log.close();
    }

    @Test
    public void twoJournalsDoNotShareState() throws Exception {
        DiagLog first = new DiagLog(temp.newFolder("first"));
        DiagLog second = new DiagLog(temp.newFolder("second"));

        first.stats(0, Collections.singletonMap("which", "first"));
        second.stats(1, Collections.singletonMap("which", "second"));

        assertEquals(
                "ts=1970-01-01T00:00:00.000Z kind=STATS which=first\n",
                read(new File(first.dir(), "diag-0.log")));
        assertEquals(
                "ts=1970-01-01T00:00:00.001Z kind=STATS which=second\n",
                read(new File(second.dir(), "diag-0.log")));
        first.close();
        second.close();
    }
}
