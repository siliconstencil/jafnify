package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/**
 * The exit-reason report: the SDK guard, the recent-only window and the
 * silence on any failure. No Android and no JNI.
 */
public class ExitReasonsTest {

    /** A reader that records whether it was called. */
    private static final class FakeReader implements ExitReasons.Reader {
        final long[] value;
        boolean called;

        FakeReader(long[] value) {
            this.value = value;
        }

        @Override
        public long[] read() {
            called = true;
            return value;
        }
    }

    private static final class RecordingSink implements ExitReasons.Sink {
        final List<long[]> reports = new ArrayList<>();

        @Override
        public void report(int reason, long ageMs) {
            reports.add(new long[] {reason, ageMs});
        }
    }

    private static final long NOW = 1_700_000_000_000L;

    @Test
    public void belowApi30TheReasonIsNeverRead() {
        FakeReader reader = new FakeReader(new long[] {16, NOW - 5_000});
        RecordingSink sink = new RecordingSink();
        ExitReasons.reportOnce(29, reader, sink, NOW);
        assertFalse(reader.called);
        assertTrue(sink.reports.isEmpty());
    }

    @Test
    public void aRecentExitIsReportedWithItsAge() {
        FakeReader reader = new FakeReader(new long[] {16, NOW - 5_000});
        RecordingSink sink = new RecordingSink();
        ExitReasons.reportOnce(30, reader, sink, NOW);
        assertTrue(reader.called);
        assertEquals(1, sink.reports.size());
        assertEquals(16, sink.reports.get(0)[0]);
        assertEquals(5_000, sink.reports.get(0)[1]);
    }

    @Test
    public void anOldExitBelongsToAnEarlierRun() {
        FakeReader reader = new FakeReader(new long[] {16, NOW - ExitReasons.RECENT_MS - 1});
        RecordingSink sink = new RecordingSink();
        ExitReasons.reportOnce(34, reader, sink, NOW);
        assertTrue(reader.called);
        assertTrue(sink.reports.isEmpty());
    }

    @Test
    public void theWindowEdgesAreExact() {
        assertTrue(ExitReasons.within(0));
        assertTrue(ExitReasons.within(ExitReasons.RECENT_MS));
        assertFalse(ExitReasons.within(ExitReasons.RECENT_MS + 1));
        assertFalse(ExitReasons.within(-1));
        // A timestamp in the future says nothing about this session.
        FakeReader future = new FakeReader(new long[] {16, NOW + 60_000});
        RecordingSink sink = new RecordingSink();
        ExitReasons.reportOnce(30, future, sink, NOW);
        assertTrue(sink.reports.isEmpty());
    }

    @Test
    public void aPackageInstallThatStartedThisProcessIsAnUpdate() {
        long start = NOW;
        assertTrue(ExitReasons.updatedSince(start - 5_000, start));
        assertTrue(ExitReasons.updatedSince(start - ExitReasons.UPDATE_SLACK_MS, start));
        assertFalse(ExitReasons.updatedSince(start - ExitReasons.UPDATE_SLACK_MS - 1, start));
        assertTrue(ExitReasons.updatedSince(start + 1_000, start));
        assertFalse(ExitReasons.updatedSince(0, start));
        assertFalse(ExitReasons.updatedSince(start, 0));
        // An update while the app was closed, opened an hour later, is not
        // this start's update.
        assertFalse(ExitReasons.updatedSince(start - 3_600_000L, start));
    }

    @Test
    public void theProcessStartComesFromTheTwoMonotonicClocks() {
        assertEquals(NOW - 7_000,
                ExitReasons.processStartWall(NOW, 500_000, 493_000));
        assertEquals(NOW, ExitReasons.processStartWall(NOW, 500_000, 500_000));
        assertEquals(0, ExitReasons.processStartWall(NOW, 500_000, 500_001));
        assertEquals(0, ExitReasons.processStartWall(NOW, 500_000, -1));
    }

    @Test
    public void aMissingOrBrokenReaderIsSilence() {
        RecordingSink sink = new RecordingSink();
        ExitReasons.reportOnce(30, null, sink, NOW);
        assertTrue(sink.reports.isEmpty());
        ExitReasons.reportOnce(30, new FakeReader(null), sink, NOW);
        assertTrue(sink.reports.isEmpty());
        ExitReasons.reportOnce(30, new FakeReader(new long[] {16}), sink, NOW);
        assertTrue(sink.reports.isEmpty());
        ExitReasons.reportOnce(30, new ExitReasons.Reader() {
            @Override
            public long[] read() {
                throw new IllegalStateException("no activity manager");
            }
        }, sink, NOW);
        assertTrue(sink.reports.isEmpty());
    }
}
