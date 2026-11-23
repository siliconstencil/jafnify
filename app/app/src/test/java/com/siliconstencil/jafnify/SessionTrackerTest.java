package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.lang.reflect.Field;

/**
 * The Java twin of linux/tests/session_test.rs: intervals, drops, reconnects,
 * rates, link counters and privacy. Times are UTC epoch milliseconds.
 */
public class SessionTrackerTest {
    private static final long T0 = 1_700_000_000_000L;
    private static final long MAX_COUNT = 0xFFFFFFFFL;

    @Test
    public void emptyTrackerReportsStartedAtAndZeros() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        SessionRecord stats = tracker.current();

        assertEquals("joiner", stats.role);
        assertEquals(T0, stats.startedAtMs);
        assertEquals(0, stats.lastActivityAtMs);
        assertEquals(0, stats.connectedMs);
        assertEquals(0, stats.upBytes);
        assertEquals(0, stats.downBytes);
        assertEquals(0, stats.avgUpBps);
        assertEquals(0, stats.avgDownBps);
        assertEquals(0, stats.peakUpBps);
        assertEquals(0, stats.peakDownBps);
        assertEquals(0, stats.drops);
        assertTrue(stats.reconnects.isEmpty());
        assertEquals("", stats.peerEndpoint);
        assertEquals(0, stats.endedAtMs);
        assertEquals("", stats.endReason);
        assertFalse(stats.id.isEmpty());
        assertTrue(stats.id.matches("[0-9a-f]+-[0-9a-f]+"));
    }

    @Test
    public void oneConnectedIntervalHasTotalsRatesAndPeaks() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "10.1.0.2:9797", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

        SessionRecord stats = tracker.current();
        // rx is received/down; tx is sent/up.
        assertEquals(300, stats.downBytes);
        assertEquals(80, stats.upBytes);
        assertEquals(2_000, stats.connectedMs);
        assertEquals(150, stats.avgDownBps);
        assertEquals(40, stats.avgUpBps);
        assertEquals(200, stats.peakDownBps);
        assertEquals(50, stats.peakUpBps);
        assertEquals(T0 + 2_000, stats.lastActivityAtMs);
        assertEquals("10.1.0.2:9797", stats.peerEndpoint);
    }

    @Test
    public void aDropClosesTheIntervalAndAReconnectRecordsTheGap() {
        SessionTracker tracker = new SessionTracker("sharer", T0);
        tracker.sample(T0, "Connected", 0, 0, "192.0.2.10:9797", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        tracker.sample(T0 + 3_000, "Retry", 200, 50, "", "no data for 2 s");

        SessionRecord dropped = tracker.current();
        assertEquals(1, dropped.drops);
        assertEquals(1, dropped.reconnects.size());
        SessionRecord.Reconnect record = dropped.reconnects.get(0);
        assertEquals(T0 + 1_000, record.disconnectedAtMs);
        assertEquals(T0 + 3_000, record.detectedAtMs);
        assertEquals(0, record.connectedAtMs);
        assertEquals(0, record.durationMs);
        assertEquals("no data for 2 s", record.reason);
        // Connected from T0 to the drop sample, not through the gap.
        assertEquals(3_000, dropped.connectedMs);

        tracker.sample(T0 + 5_000, "Connected", 400, 90, "", "");

        SessionRecord reconnected = tracker.current();
        assertEquals(1, reconnected.drops);
        assertEquals(1, reconnected.reconnects.size());
        record = reconnected.reconnects.get(0);
        assertEquals(T0 + 1_000, record.disconnectedAtMs);
        assertEquals(T0 + 3_000, record.detectedAtMs);
        assertEquals(T0 + 5_000, record.connectedAtMs);
        assertEquals(4_000, record.durationMs);
        // The non-Connected gap does not count toward connectedMs.
        assertEquals(3_000, reconnected.connectedMs);
    }

    @Test
    public void currentIsADeepSnapshotOfTheReconnects() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
        tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");

        SessionRecord held = tracker.current();
        assertEquals(1, held.reconnects.size());
        SessionRecord.Reconnect heldReconnect = held.reconnects.get(0);
        assertEquals(0, heldReconnect.connectedAtMs);

        // Reconnecting mutates the tracker's own pending record. A shallow
        // snapshot would change with it.
        tracker.sample(T0 + 3_000, "Connected", 200, 0, "", "");
        assertEquals(0, heldReconnect.connectedAtMs);
        assertEquals(0, heldReconnect.durationMs);
        assertEquals(T0 + 3_000,
                tracker.current().reconnects.get(0).connectedAtMs);

        // A further drop adds a new internal record; the held list stays one.
        tracker.sample(T0 + 4_000, "Retry", 200, 0, "", "link down");
        assertEquals(1, held.reconnects.size());
        assertEquals(0, held.reconnects.get(0).connectedAtMs);
        assertEquals(2, tracker.current().reconnects.size());
    }

    @Test
    public void aDropBeforeAnyActivityDoesNotMakeADecadesLongReconnect() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        // First sample is only the counter baseline, so there is no activity yet.
        tracker.sample(T0, "Connected", 100, 100, "", "");
        tracker.sample(T0 + 1_000, "Retry", 100, 100, "", "no data for 1 s");

        SessionRecord dropped = tracker.current();
        assertEquals(1, dropped.reconnects.size());
        SessionRecord.Reconnect record = dropped.reconnects.get(0);
        assertEquals(T0 + 1_000, record.disconnectedAtMs);
        assertEquals(T0 + 1_000, record.detectedAtMs);

        tracker.sample(T0 + 3_000, "Connected", 200, 150, "", "");
        SessionRecord reconnected = tracker.current();
        record = reconnected.reconnects.get(0);
        assertEquals(T0 + 3_000, record.connectedAtMs);
        assertEquals(2_000, record.durationMs);
    }

    @Test
    public void aSecondDropClosesAStillPendingReconnect() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
        tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");
        // State is Connected again but there was no new activity, so the
        // reconnect stays pending.
        tracker.sample(T0 + 3_000, "Connected", 100, 0, "", "");
        assertEquals(0, tracker.current().reconnects.get(0).connectedAtMs);

        tracker.sample(T0 + 4_000, "Retry", 100, 0, "", "link down");

        SessionRecord stats = tracker.current();
        assertEquals(2, stats.drops);
        assertEquals(2, stats.reconnects.size());
        SessionRecord.Reconnect first = stats.reconnects.get(0);
        assertEquals(T0 + 4_000, first.connectedAtMs);
        assertEquals(3_000, first.durationMs);
        SessionRecord.Reconnect second = stats.reconnects.get(1);
        assertEquals(T0 + 4_000, second.disconnectedAtMs);
        assertEquals(T0 + 4_000, second.detectedAtMs);
        assertEquals(0, second.connectedAtMs);
        assertEquals(0, second.durationMs);
        assertTrue(first.connectedAtMs <= second.disconnectedAtMs);

        tracker.sample(T0 + 7_000, "Connected", 200, 0, "", "");
        SessionRecord reconnected = tracker.current();
        second = reconnected.reconnects.get(1);
        assertEquals(T0 + 7_000, second.connectedAtMs);
        // The gap starts at the second drop, not the stale pre-first-drop time.
        assertEquals(3_000, second.durationMs);
    }

    @Test
    public void finishClosesAPendingReconnect() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
        tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");
        // Connected again with no new activity leaves the reconnect pending.
        tracker.sample(T0 + 3_000, "Connected", 100, 0, "", "");
        assertEquals(0, tracker.current().reconnects.get(0).connectedAtMs);

        SessionRecord record = tracker.finish(T0 + 4_000, "stopped");
        assertEquals(T0 + 4_000, record.endedAtMs);
        SessionRecord.Reconnect reconnect = record.reconnects.get(0);
        assertEquals(T0 + 4_000, reconnect.connectedAtMs);
        assertEquals(3_000, reconnect.durationMs);

        assertEquals(record.reconnects, tracker.current().reconnects);
    }

    @Test
    public void finishWhileDisconnectedLeavesTheReconnectPending() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 100, 0, "", "");
        tracker.sample(T0 + 2_000, "Retry", 100, 0, "", "no data for 1 s");

        SessionRecord record = tracker.finish(T0 + 3_000, "lost");
        assertEquals(T0 + 3_000, record.endedAtMs);
        assertEquals(1, record.drops);
        SessionRecord.Reconnect reconnect = record.reconnects.get(0);
        assertEquals(T0 + 1_000, reconnect.disconnectedAtMs);
        assertEquals(T0 + 2_000, reconnect.detectedAtMs);
        assertEquals(0, reconnect.connectedAtMs);
        assertEquals(0, reconnect.durationMs);
        assertEquals(record.reconnects, tracker.current().reconnects);
    }

    @Test
    public void aDropUsesOnlyItsOwnSampleReason() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "old reason");
        tracker.sample(T0 + 1_000, "Retry", 0, 0, "", "");
        assertEquals("", tracker.current().reconnects.get(0).reason);

        tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "old reason");
        tracker.sample(T0 + 1_000, "Retry", 0, 0, "", "fresh reason");
        assertEquals("fresh reason", tracker.current().reconnects.get(0).reason);
    }

    @Test
    public void samplesAfterFinishDoNotChangeTheSession() {
        SessionTracker tracker = new SessionTracker("sharer", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

        SessionRecord record = tracker.finish(T0 + 3_000, "stopped");
        SessionRecord after = tracker.current();

        tracker.sample(T0 + 4_000, "Connected", 500, 100, "192.0.2.9:9797", "");
        tracker.sample(T0 + 6_000, "Connected", 700, 150, "", "");
        SessionRecord later = tracker.current();

        assertEquals(after.connectedMs, later.connectedMs);
        assertEquals(after.downBytes, later.downBytes);
        assertEquals(after.upBytes, later.upBytes);
        assertEquals(after.lastActivityAtMs, later.lastActivityAtMs);
        assertEquals(after.peakDownBps, later.peakDownBps);
        assertEquals(after.drops, later.drops);
        assertEquals(after.reconnects, later.reconnects);
        assertEquals(after.peerEndpoint, later.peerEndpoint);

        SessionRecord again = tracker.finish(T0 + 7_000, "stopped");
        assertEquals(record.endedAtMs, again.endedAtMs);
        assertEquals(record.connectedMs, again.connectedMs);
        assertEquals("stopped", again.endReason);
    }

    @Test
    public void aCounterResetStartsANewBaselineWithoutAHugePeak() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 500, 0, "", "");

        tracker.sample(T0 + 2_000, "Connected", 10, 0, "", "");
        SessionRecord reset = tracker.current();
        assertEquals(500, reset.downBytes);
        assertEquals(500, reset.peakDownBps);
        assertEquals(0, reset.drops);

        tracker.sample(T0 + 3_000, "Connected", 60, 0, "", "");
        SessionRecord resumed = tracker.current();
        assertEquals(550, resumed.downBytes);
        assertEquals(500, resumed.peakDownBps);
        assertEquals(0, resumed.drops);
    }

    @Test
    public void finishSetsTheEndTimeAndReasonAndLeavesCurrentCoherent() {
        SessionTracker tracker = new SessionTracker("sharer", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        tracker.sample(T0 + 2_000, "Connected", 300, 80, "", "");

        SessionRecord record = tracker.finish(T0 + 3_000, "stopped");
        assertEquals(T0 + 3_000, record.endedAtMs);
        assertEquals("stopped", record.endReason);
        assertEquals(T0, record.startedAtMs);
        assertEquals(300, record.downBytes);
        assertEquals(80, record.upBytes);
        assertEquals(3_000, record.connectedMs);
        assertEquals(100, record.avgDownBps);
        assertEquals(26, record.avgUpBps);
        assertEquals(200, record.peakDownBps);
        assertEquals(50, record.peakUpBps);

        SessionRecord current = tracker.current();
        assertEquals(record.id, current.id);
        assertEquals(T0, current.startedAtMs);
        assertEquals(300, current.downBytes);
        assertEquals(3_000, current.connectedMs);
    }

    @Test
    public void linkCountersStartAtZeroAndIncrement() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        SessionRecord before = tracker.current();
        assertEquals(0, before.linkDowns);
        assertEquals(0, before.linkUps);

        tracker.linkDown();
        tracker.linkDown();
        tracker.linkUp();
        SessionRecord after = tracker.current();
        assertEquals(2, after.linkDowns);
        assertEquals(1, after.linkUps);
    }

    @Test
    public void finishCopiesTheLinkCounters() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.linkDown();
        tracker.linkUp();
        tracker.linkUp();

        SessionRecord record = tracker.finish(T0 + 1_000, "stopped");
        assertEquals(1, record.linkDowns);
        assertEquals(2, record.linkUps);
        assertEquals(1, tracker.current().linkDowns);
        assertEquals(2, tracker.current().linkUps);
    }

    @Test
    public void linkCountersAreNoOpsAfterFinish() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        tracker.linkDown();
        tracker.finish(T0 + 1_000, "stopped");

        tracker.linkDown();
        tracker.linkUp();
        assertEquals(1, tracker.current().linkDowns);
        assertEquals(0, tracker.current().linkUps);
        SessionRecord record = tracker.finish(T0 + 2_000, "stopped");
        assertEquals(1, record.linkDowns);
        assertEquals(0, record.linkUps);
    }

    @Test
    public void u32CountersSaturateInsteadOfWrapping() throws Exception {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        setLong(tracker, "drops", MAX_COUNT);
        setLong(tracker, "linkDowns", MAX_COUNT);
        setLong(tracker, "linkUps", MAX_COUNT);

        tracker.sample(T0 + 1_000, "Retry", 0, 0, "", "lost");
        tracker.linkDown();
        tracker.linkUp();

        SessionRecord stats = tracker.current();
        assertEquals(MAX_COUNT, stats.drops);
        assertEquals(MAX_COUNT, stats.linkDowns);
        assertEquals(MAX_COUNT, stats.linkUps);
        assertTrue(stats.drops >= 0);
    }

    @Test
    public void byteCountersSaturateAtLongMax() throws Exception {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "", "");
        setLong(tracker, "upBytes", Long.MAX_VALUE - 5);
        setLong(tracker, "downBytes", Long.MAX_VALUE - 5);

        tracker.sample(T0 + 1_000, "Connected", 100, 100, "", "");

        SessionRecord stats = tracker.current();
        assertEquals(Long.MAX_VALUE, stats.upBytes);
        assertEquals(Long.MAX_VALUE, stats.downBytes);
        assertTrue(stats.upBytes >= 0);
    }

    @Test
    public void theIdAndFieldsCarryNoKeyShapedText() {
        SessionTracker tracker = new SessionTracker("joiner", T0);
        tracker.sample(T0, "Connected", 0, 0, "10.1.0.2:9797", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        SessionRecord record = tracker.finish(T0 + 2_000, "app_exit");

        String text = record.id + " " + record.role + " " + record.peerEndpoint
                + " " + record.endReason;
        assertFalse(text.matches(".*[A-Za-z0-9+/]{42,44}=.*"));
    }

    private static void setLong(Object target, String name, long value) throws Exception {
        Field field = SessionTracker.class.getDeclaredField(name);
        field.setAccessible(true);
        field.setLong(target, value);
    }

    @Test
    public void aPeerRestartCountsOneDropThatReconnectedAtOnce() {
        SessionTracker tracker = new SessionTracker("sharer", T0);
        tracker.sample(T0, "Connected", 0, 0, "192.0.2.10:9797", "");
        tracker.sample(T0 + 1_000, "Connected", 200, 50, "", "");
        tracker.peerRestart(T0 + 1_500, "Peer restarted");

        SessionRecord stats = tracker.current();
        assertEquals(1, stats.drops);
        assertEquals(1, stats.reconnects.size());
        SessionRecord.Reconnect record = stats.reconnects.get(0);
        assertEquals(T0 + 1_000, record.disconnectedAtMs);
        assertEquals(T0 + 1_500, record.detectedAtMs);
        assertEquals(T0 + 1_500, record.connectedAtMs);
        assertEquals(500, record.durationMs);
        assertEquals("Peer restarted", record.reason);

        tracker.finish(T0 + 2_000, "stop");
        tracker.peerRestart(T0 + 3_000, "Peer restarted");
        assertEquals(1, tracker.current().drops);
    }

    @Test
    public void theStartReasonIsCopiedToTheLiveAndFinishedRecord() {
        SessionTracker tracker = new SessionTracker("sharer", T0);
        tracker.setStartReason("boot");
        assertEquals("boot", tracker.current().startReason);
        assertEquals("boot", tracker.finish(T0 + 1_000, "stopped").startReason);
    }
}
