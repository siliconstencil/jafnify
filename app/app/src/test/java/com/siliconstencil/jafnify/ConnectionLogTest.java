package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import java.util.Arrays;
import java.util.List;
import java.util.TimeZone;

/** The bounded, redacted connection log and its exact line templates. */
public class ConnectionLogTest {
    /** 2023-11-14 22:13:20 UTC. */
    private static final long T = 1_700_000_000_000L;

    @Rule
    public TemporaryFolder tmp = new TemporaryFolder();

    @Test
    public void anAttachedLogShowsEarlierRunsAfterARestart() throws Exception {
        File file = new File(tmp.getRoot(), "connection.log");
        ConnectionLog first = new ConnectionLog();
        first.attach(file);
        assertTrue(first.isEmpty());
        first.push("session", "session started (sharer)", T);
        first.push("drop", "first\nsecond", T + 1_000);

        ConnectionLog second = new ConnectionLog();
        second.attach(file);
        assertEquals(2, second.size());
        assertEquals(ConnectionLog.EARLIER_KIND, second.lastKind());
        assertEquals(first.tail(2), second.tail(2));
        assertTrue(second.tail(1).get(0).endsWith("first second"));
        second.push("session", "session started (sharer)", T + 9_000);

        ConnectionLog third = new ConnectionLog();
        third.attach(file);
        assertEquals(second.tail(3), third.tail(3));
    }

    @Test
    public void anAttachedLogFileStaysBounded() throws Exception {
        File file = new File(tmp.getRoot(), "connection.log");
        ConnectionLog log = new ConnectionLog();
        log.attach(file);
        int total = 3 * ConnectionLog.MAX_LINES + 7;
        for (int i = 0; i < total; i++) {
            log.push("state", "line " + i, T + i);
        }
        int lines = Files.readAllLines(file.toPath(), StandardCharsets.UTF_8).size();
        assertTrue("file grew to " + lines, lines <= 2 * ConnectionLog.MAX_LINES);
        ConnectionLog again = new ConnectionLog();
        again.attach(file);
        assertEquals(ConnectionLog.MAX_LINES, again.size());
        assertEquals(log.tail(ConnectionLog.MAX_LINES), again.tail(ConnectionLog.MAX_LINES));
        assertTrue(again.tail(1).get(0).endsWith("line " + (total - 1)));
    }

    @Test
    public void anUnwritableFileStillLogsOnScreen() throws Exception {
        File dir = tmp.newFolder("connection.log");
        ConnectionLog log = new ConnectionLog();
        log.attach(dir);
        log.push("state", "state Idle -> Searching", T);
        assertEquals(1, log.size());
    }

    @Test
    public void timestampsCarryTheZoneAbbreviation() {
        TimeZone utc = TimeZone.getTimeZone("UTC");
        TimeZone pst = TimeZone.getTimeZone("America/Los_Angeles");
        assertEquals("1970-01-01 00:00:00 UTC", ConnectionLog.timestamp(0, utc));
        assertEquals("2023-11-14 22:13:20 UTC", ConnectionLog.timestamp(T, utc));
        assertEquals("2023-11-14 14:13:20 PST", ConnectionLog.timestamp(T, pst));

        ConnectionLog log = new ConnectionLog();
        log.push("state", "state Connecting -> Connected", T);
        String expected = ConnectionLog.timestamp(T) + " state Connecting -> Connected";
        assertEquals(Arrays.asList(expected), log.tail(1));
        assertEquals(expected, log.text());
        assertEquals(1, log.size());
        assertFalse(log.isEmpty());
    }

    @Test
    public void clearDropsTheOldRun() {
        ConnectionLog log = new ConnectionLog();
        log.push("session", "session started (sharer)", T);
        log.clear();
        assertTrue(log.isEmpty());
        assertEquals("", log.text());
    }

    @Test
    public void theRingDropsTheOldestPastFiveHundred() {
        ConnectionLog log = new ConnectionLog();
        for (int i = 0; i <= ConnectionLog.MAX_LINES; i++) {
            log.push("state", "event " + i, T + i * 1_000L);
        }
        assertEquals(ConnectionLog.MAX_LINES, log.size());
        List<String> all = log.tail(ConnectionLog.MAX_LINES);
        assertEquals(ConnectionLog.MAX_LINES, all.size());
        assertTrue(all.get(0).endsWith("event 1"));
        assertTrue(all.get(ConnectionLog.MAX_LINES - 1)
                .endsWith("event " + ConnectionLog.MAX_LINES));
        assertFalse(all.get(0).endsWith("event 0"));
    }

    @Test
    public void tailIsChronologicalAndTextHasNoTrailingNewline() {
        ConnectionLog log = new ConnectionLog();
        for (int i = 1; i <= 5; i++) {
            log.push("state", "line " + i, T + i * 1_000L);
        }
        assertEquals(
                Arrays.asList(
                        ConnectionLog.timestamp(T + 3_000L) + " line 3",
                        ConnectionLog.timestamp(T + 4_000L) + " line 4",
                        ConnectionLog.timestamp(T + 5_000L) + " line 5"),
                log.tail(3));
        assertEquals(
                ConnectionLog.timestamp(T + 1_000L) + " line 1\n"
                        + ConnectionLog.timestamp(T + 2_000L) + " line 2\n"
                        + ConnectionLog.timestamp(T + 3_000L) + " line 3\n"
                        + ConnectionLog.timestamp(T + 4_000L) + " line 4\n"
                        + ConnectionLog.timestamp(T + 5_000L) + " line 5",
                log.text());
        assertEquals(0, log.tail(0).size());
        assertEquals(5, log.tail(99).size());
        assertEquals("", new ConnectionLog().text());
        assertTrue(new ConnectionLog().isEmpty());
    }

    @Test
    public void eachEntryIsOneLine() {
        ConnectionLog log = new ConnectionLog();
        log.push("error", "first line\nsecond line\r\nthird", T);

        String line = log.tail(1).get(0);
        assertEquals(ConnectionLog.timestamp(T) + " first line second line third", line);
        assertFalse(line.contains("\n"));
        assertFalse(line.contains("\r"));
    }

    @Test
    public void keyShapedTextIsRedactedOnPush() {
        String key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        ConnectionLog log = new ConnectionLog();
        log.push("error", "peer " + key + " denied", T);

        String line = log.tail(1).get(0);
        assertTrue(line.contains("[redacted]"));
        assertFalse(line.contains("AAAAAAAA"));
        assertFalse(line.contains(key));
        assertFalse(log.text().contains(key));
    }

    @Test
    public void eventLinesArePinned() {
        assertEquals("state Connected -> Retry",
                ConnectionLog.stateLine("Connected", "Retry"));
        TimeZone utc = TimeZone.getTimeZone("UTC");
        assertEquals(
                "disconnected at 2023-11-14 22:13:20 UTC: no data from peer for 35 s"
                        + " (detected after 35 s)",
                ConnectionLog.dropLine("no data from peer for 35 s", T, T + 35_000L, utc));
        assertEquals(
                "reconnected at 2023-11-14 22:14:25 UTC after 0:01:05"
                        + " (peer 10.1.0.2:9797)",
                ConnectionLog.reconnectLine(T + 65_000L, 65_000L, "10.1.0.2:9797", utc));
        assertEquals("0:00:05", ConnectionLog.durationHms(5_000L));
        assertEquals("1:01:01", ConnectionLog.durationHms(3_661_000L));
        assertEquals("link changed: 10.0.0.1 -> 10.0.0.2",
                ConnectionLog.linkChangedLine("10.0.0.1", "10.0.0.2"));
        assertEquals("session started (sharer)",
                ConnectionLog.sessionStartedLine("sharer"));
        assertEquals("started by: boot", ConnectionLog.startedByLine("boot"));
        assertEquals("started by: system restart",
                ConnectionLog.startedByLine("system restart"));
        assertEquals("session ended (stopped)",
                ConnectionLog.sessionEndedLine("stopped"));
    }
}
