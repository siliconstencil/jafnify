package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

public class NetworkSamplerTest {
    private static final long HOUR = 3_600_000L;

    private final List<String> networkLines = new ArrayList<>();
    private final List<String> signalLines = new ArrayList<>();

    private final NetworkSampler.Bridge bridge = new NetworkSampler.Bridge() {
        @Override
        public String generationWord(String kind) {
            if ("nr".equals(kind)) {
                return "5g";
            }
            if ("lte".equals(kind)) {
                return "4g";
            }
            return "";
        }

        @Override
        public boolean generationSwitch(String previous, String current) {
            return !previous.isEmpty() && !current.isEmpty() && !previous.equals(current);
        }

        @Override
        public String bucketLine(String fields) {
            return "JFN1 1 " + fields;
        }

        @Override
        public String signalSample(long atMs, int level, long lastAtMs) {
            if (level < 0 || level > 4) {
                return "";
            }
            if (lastAtMs > 0 && atMs - lastAtMs < 300_000L) {
                return "";
            }
            return "JSG1 1 at=" + atMs + " level=" + level;
        }

        @Override
        public long saneMah(long counter) {
            return counter > 0 ? counter : -1;
        }

        @Override
        public long[] networkSeed(String line) {
            if (line.startsWith("JFN1 1 hour=0 ")) {
                return new long[] {
                    0, 5, 1000, 2000, 1, 1_200_000, 20, 40, -1, 0, 1, 0, 1_800_000,
                };
            }
            return new long[0];
        }
    };

    private final FakeStore store = new FakeStore(networkLines, signalLines);

    private static final class FakeStore implements NetworkSampler.Store {
        final List<String> networkLines;
        final List<String> signalLines;
        String storedNetwork = "";

        FakeStore(List<String> networkLines, List<String> signalLines) {
            this.networkLines = networkLines;
            this.signalLines = signalLines;
        }

        @Override
        public void upsertNetwork(String line) {
            networkLines.add(line);
        }

        @Override
        public void upsertSignal(String line) {
            signalLines.add(line);
        }

        @Override
        public String loadNetworkHour(long hourStartMs) {
            return storedNetwork;
        }
    }

    private NetworkSampler sampler() {
        return new NetworkSampler(bridge, store);
    }

    @Test
    public void aGenerationChangeIsOneSwitchAndTheTimeIsCountedPerGeneration() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onGeneration("nr", 0);
        sampler.onGeneration("nr", 1000);
        sampler.onGeneration("lte", 2000);
        sampler.onTick(2000, 0, 0, true, 80, 2400, false);
        sampler.onStop(3000);

        assertEquals(1, networkLines.size());
        String line = networkLines.get(0);
        assertTrue(line, line.contains("switches=1"));
        assertTrue(line, line.contains("gen=5g:2000,4g:1000"));
        assertTrue(line, line.contains("up=0"));
        assertTrue(line, line.contains("samples=1"));
    }

    @Test
    public void anUnknownKindIsIgnored() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onGeneration("", 0);
        sampler.onGeneration("nr", 1000);
        sampler.onTick(1000, 0, 0, true, 80, 2400, false);
        sampler.onStop(2000);

        assertTrue(networkLines.get(0), networkLines.get(0).contains("switches=0"));
    }

    @Test
    public void theReportOffStoresNothing() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(false, false);
        sampler.onGeneration("nr", 0);
        sampler.onSignal(3, 0);
        sampler.onTick(1000, 0, 0, true, 80, 2400, false);
        sampler.onStop(2000);

        assertTrue(networkLines.isEmpty());
        assertTrue(signalLines.isEmpty());
    }

    @Test
    public void theDrainUsesMilliAmpHoursWhenTheCounterIsSane() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(0, 0, 0, true, 80, 2400, false);
        sampler.onTick(1000, 0, 0, true, 78, 2380, false);
        sampler.onStop(2000);

        assertTrue(networkLines.get(0), networkLines.get(0).contains("batt_mah=20"));
    }

    @Test
    public void theDrainFallsBackToPercentWhenTheCounterIsMissing() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(0, 0, 0, true, 80, -1, false);
        sampler.onTick(1000, 0, 0, true, 78, -1, false);
        sampler.onStop(2000);

        String line = networkLines.get(0);
        assertFalse(line, line.contains("batt_mah"));
        assertTrue(line, line.contains("batt_pct=2"));
    }

    @Test
    public void aChargingHourIsMarkedSoTheCoreCanLeaveOutTheDrain() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(0, 0, 0, true, 80, 2400, true);
        sampler.onStop(1000);

        String line = networkLines.get(0);
        assertTrue(line, line.contains("charging=1"));
    }

    @Test
    public void theSignalSampleIsStoredOnlyWhenTheGateAllowsIt() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(false, true);
        sampler.onSignal(3, 1000);
        assertEquals(1, signalLines.size());
        assertEquals("JSG1 1 at=1000 level=3", signalLines.get(0));

        sampler.onSignal(2, 2000);
        assertEquals(1, signalLines.size());

        sampler.onSignal(1, 1000 + 300_000L);
        assertEquals(2, signalLines.size());
        assertEquals("JSG1 1 at=301000 level=1", signalLines.get(1));
    }

    @Test
    public void theHourRollsAndKeepsOneBucketPerHour() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(0, 0, 0, true, 80, 2400, false);
        sampler.onTick(HOUR + 1000, 100, 200, true, 80, 2400, false);
        sampler.onStop(HOUR + 2000);

        assertEquals(2, networkLines.size());
        assertTrue(networkLines.get(0),
                networkLines.get(0).contains("hour=" + 0));
        assertTrue(networkLines.get(1),
                networkLines.get(1).contains("hour=" + HOUR));
        assertTrue(networkLines.get(1), networkLines.get(1).contains("up=100"));
        assertTrue(networkLines.get(1), networkLines.get(1).contains("down=200"));
    }

    @Test
    public void connectedTimeBeforeTheHourBoundaryStaysInTheEndingHour() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(HOUR - 10_000, 0, 0, true, 80, 2400, false);
        sampler.onTick(HOUR + 5_000, 0, 0, true, 80, 2400, false);
        sampler.onStop(HOUR + 5_000);

        // 10 s belong to the hour that ended, 5 s to the next one.
        assertTrue(networkLines.get(0), networkLines.get(0).contains("connected_ms=10000"));
        assertTrue(networkLines.get(1), networkLines.get(1).contains("connected_ms=5000"));
    }

    @Test
    public void the_in_progress_hour_is_written_every_minute() {
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(0, 0, 0, true, 80, -1, false);
        sampler.onTick(60_000, 100, 200, true, 80, -1, false);
        assertEquals(1, networkLines.size());
        assertTrue(networkLines.get(0), networkLines.get(0).contains("hour=0"));
        sampler.onTick(120_000, 300, 600, true, 80, -1, false);
        assertEquals(2, networkLines.size());
    }

    @Test
    public void a_restart_mid_hour_keeps_the_earlier_part() {
        store.storedNetwork = "JFN1 1 hour=0 switches=5 gen=4g:1800000 up=1000"
                + " down=2000 drops=1 batt_mah=40 charging=0 connected_ms=1200000"
                + " samples=20";
        NetworkSampler sampler = sampler();
        sampler.setFlags(true, false);
        sampler.onTick(30 * 60_000L, 0, 0, true, 80, -1, false);
        sampler.onTick(30 * 60_000L + 1000, 500, 700, true, 80, -1, false);
        sampler.onTick(HOUR, 900, 1200, true, 80, -1, false);

        String last = networkLines.get(networkLines.size() - 1);
        assertTrue(last, last.contains("switches=5"));
        assertTrue(last, last.contains("up=1500"));
        assertTrue(last, last.contains("down=2700"));
        assertTrue(last, last.contains("samples=22"));
    }
}
