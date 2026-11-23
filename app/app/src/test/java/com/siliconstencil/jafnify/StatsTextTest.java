package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.TimeZone;

/**
 * The Java twin of the Linux details/history wording. Exact strings are pinned
 * here so the two platforms cannot drift.
 */
public class StatsTextTest {
    private static SessionRecord record() {
        SessionRecord record = new SessionRecord();
        record.id = "a";
        record.role = "joiner";
        record.startedAtMs = 1_700_000_000_000L;
        record.lastActivityAtMs = 1_700_000_001_000L;
        record.endedAtMs = 1_700_000_002_000L;
        record.connectedMs = 2_000;
        record.upBytes = 80;
        record.downBytes = 300;
        record.avgUpBps = 40;
        record.avgDownBps = 150;
        record.peakUpBps = 50;
        record.peakDownBps = 200;
        record.drops = 1;
        record.linkDowns = 0;
        record.linkUps = 0;
        SessionRecord.Reconnect reconnect = new SessionRecord.Reconnect();
        reconnect.disconnectedAtMs = 1_700_000_000_500L;
        reconnect.detectedAtMs = 1_700_000_001_000L;
        reconnect.connectedAtMs = 1_700_000_001_500L;
        reconnect.durationMs = 1_000;
        reconnect.reason = "no data for 2 s";
        record.reconnects = new ArrayList<>(Collections.singletonList(reconnect));
        record.peerEndpoint = "10.1.0.2:9797";
        record.endReason = "stopped";
        return record;
    }

    @Test
    public void linkCountsLineMatchesTheLinuxText() {
        assertEquals("Link losses: 2   Link restores: 1", StatsText.linkCountsLine(2, 1));
        assertEquals("Link losses: 0   Link restores: 0", StatsText.linkCountsLine(0, 0));
    }

    @Test
    public void elapsedIsMinutesSecondsThenHours() {
        assertEquals("0:00", StatsText.formatElapsed(0));
        assertEquals("0:59", StatsText.formatElapsed(59));
        assertEquals("12:34", StatsText.formatElapsed(12 * 60 + 34));
        assertEquals("1:02:03", StatsText.formatElapsed(3600 + 2 * 60 + 3));
        assertEquals("0:00", StatsText.formatElapsed(-5));
    }

    @Test
    public void bytesUseB_KB_MB() {
        assertEquals("0 B", StatsText.formatBytes(0));
        assertEquals("1023 B", StatsText.formatBytes(1023));
        assertEquals("1 KB", StatsText.formatBytes(1024));
        assertEquals("340 KB", StatsText.formatBytes(340 * 1024));
        assertEquals("1.5 MB", StatsText.formatBytes(1536 * 1024));
    }

    @Test
    public void ratesAddThePerSecondSuffix() {
        assertEquals("0 B/s", StatsText.formatRate(0));
        assertEquals("150 B/s", StatsText.formatRate(150));
        assertEquals("1.5 MB/s", StatsText.formatRate(1536 * 1024));
    }

    @Test
    public void zeroTimestampsRenderNoneAndRealOnesCarryTheZone() {
        TimeZone utc = TimeZone.getTimeZone("UTC");
        TimeZone pst = TimeZone.getTimeZone("America/Los_Angeles");
        assertEquals("none", StatsText.formatTimestampOrNone(0, utc));
        assertEquals("none", StatsText.formatTimestampOrNone(-1, utc));
        assertEquals("2023-11-14 22:13:20 UTC",
                StatsText.formatTimestampOrNone(1_700_000_000_000L, utc));
        assertEquals("2026-10-23 14:05:00 UTC",
                StatsText.formatTimestampOrNone(1_792_764_300_000L, utc));
        assertEquals("2023-11-14 14:13:20 PST",
                StatsText.formatTimestampOrNone(1_700_000_000_000L, pst));
    }

    @Test
    public void detailsMatchTheLinuxLinesInTheSameOrder() {
        assertEquals(Arrays.asList(
                "Started: 2023-11-14 22:13:20 UTC",
                "Connected: 0:02",
                "Up (laptop upload): 300 B",
                "Down (laptop download): 80 B",
                "Average up: 150 B/s   Average down: 40 B/s",
                "Peak up: 200 B/s   Peak down: 50 B/s",
                "Drops: 1   Reconnects: 1",
                "Link losses: 0   Link restores: 0",
                "Peer endpoint: 10.1.0.2:9797"),
                StatsText.details(record(), TimeZone.getTimeZone("UTC")));
    }

    @Test
    public void phoneSessionRowsArePinnedLabelValuePairs() {
        java.util.List<String[]> rows =
                StatsText.sessionRows(record(), TimeZone.getTimeZone("UTC"));
        StringBuilder text = new StringBuilder();
        for (String[] r : rows) {
            assertEquals(2, r.length);
            text.append(r[0]).append(" | ").append(r[1]).append('\n');
        }
        assertEquals(
                "Started | 2023-11-14 22:13:20 UTC\n"
                        + "Connected for | 0:02\n"
                        + "Up (laptop upload) | 300 B\n"
                        + "Down (laptop download) | 80 B\n"
                        + "Average | 150 B/s up, 40 B/s down\n"
                        + "Peak | 200 B/s up, 50 B/s down\n"
                        + "Drops | 1\n"
                        + "Reconnects | 1\n"
                        + "Link losses | 0 lost, 0 restored\n"
                        + "Laptop address | 10.1.0.2:9797\n",
                text.toString());
    }

    @Test
    public void anEmptyEndpointReadsUnknown() {
        SessionRecord record = record();
        record.peerEndpoint = "";
        assertEquals("Peer endpoint: unknown", StatsText.details(record).get(8));
    }

    @Test
    public void detailsUseZeroReconnectsWhenThereAreNone() {
        SessionRecord record = record();
        record.reconnects.clear();
        assertEquals("Drops: 1   Reconnects: 0", StatsText.details(record).get(6));
    }

    @Test
    public void historyLineIsOneCompactPinnedRow() {
        assertEquals(
                "2023-11-14 22:13:20 UTC   0:02   Drops: 1   Reconnects: 1"
                        + "   Up 300 B   Down 80 B   10.1.0.2:9797",
                StatsText.historyLine(record(), TimeZone.getTimeZone("UTC")));
    }

    @Test
    public void historyLineUsesUnknownForAMissingEndpoint() {
        SessionRecord record = record();
        record.peerEndpoint = "";
        assertEquals(
                "2023-11-14 22:13:20 UTC   0:02   Drops: 1   Reconnects: 1"
                        + "   Up 300 B   Down 80 B   unknown",
                StatsText.historyLine(record, TimeZone.getTimeZone("UTC")));
    }

    @Test
    public void theHomeTileGetsTheLaptopsDirectionLikeTheOtherTiles() {
        // The phone SESSION tile said "Down 1.5 MB" for what
        // the laptop sent, while the laptop and the phone TODAY tile called it
        // up. Tunnel rx (downBytes) is the laptop's upload.
        assertEquals("up=300 down=80 peak=50 start=1700000000000",
                StatsText.tileSessionLine(record()));
    }
}

