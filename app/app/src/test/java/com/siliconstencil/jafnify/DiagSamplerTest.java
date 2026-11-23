package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The 5 s / 1 s diagnostic sampling cadence. */
public class DiagSamplerTest {
    private static final long BASE = 1_000_000L;

    @Test
    public void theFirstSampleIsAlwaysDue() {
        assertTrue(DiagSampler.statsDue(BASE, null, null));
        assertTrue(DiagSampler.statsDue(BASE, null, BASE));
        assertTrue(DiagSampler.statsDue(BASE, null, BASE - 10_000L));
    }

    @Test
    public void statsRunEveryFiveSecondsWhilePeerRxIsFresh() {
        long lastStats = BASE;
        assertFalse(DiagSampler.statsDue(BASE + 4_999L, lastStats, BASE + 3_999L));
        assertTrue(DiagSampler.statsDue(BASE + 5_000L, lastStats, BASE + 4_000L));
    }

    @Test
    public void statsRunEverySecondWhilePeerRxIsOlderThanThreeSeconds() {
        long lastStats = BASE;
        long lastPeerRx = BASE - 4_000L;

        assertFalse(DiagSampler.statsDue(BASE + 999L, lastStats, lastPeerRx));
        assertTrue(DiagSampler.statsDue(BASE + 1_000L, lastStats, lastPeerRx));
    }

    @Test
    public void statsRunEverySecondWhenPeerRxIsUnknown() {
        long lastStats = BASE;

        assertFalse(DiagSampler.statsDue(BASE + 999L, lastStats, null));
        assertTrue(DiagSampler.statsDue(BASE + 1_000L, lastStats, null));
    }

    @Test
    public void exactlyThreeSecondsOldIsStillFreshSoTheIntervalIsFiveSeconds() {
        long now = BASE;

        // Exactly 3000 ms old is not "older than 3000": 1 s would be due here.
        assertFalse(DiagSampler.statsDue(now, now - 1_000L, now - 3_000L));
        // One millisecond older switches to the 1 s cadence and is due.
        assertTrue(DiagSampler.statsDue(now, now - 1_000L, now - 3_001L));
    }

    @Test
    public void peerRxResumingReturnsTheIntervalToFiveSeconds() {
        long lastStats = BASE + 4_000L;

        long now = lastStats + 1_000L;
        // A 1 s cadence would sample now; the resumed 5 s cadence must not.
        assertFalse(DiagSampler.statsDue(now, lastStats, now - 500L));
        now = lastStats + 4_999L;
        assertFalse(DiagSampler.statsDue(now, lastStats, now - 500L));
        now = lastStats + 5_000L;
        assertTrue(DiagSampler.statsDue(now, lastStats, now - 500L));
    }
}
