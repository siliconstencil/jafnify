package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The link state machine that keeps a session alive while the hotspot is off. */
public class SharerLinkPolicyTest {
    private static final String LAN = "10.0.0.1";

    /** A sharing policy whose set has just gone empty and stable. */
    private static SharerLinkPolicy waitingPolicy() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("", 1000, true));
        assertEquals(SharerLinkPolicy.Action.WAIT, p.tick("", 3000, true));
        return p;
    }

    @Test
    public void anEmptySetWhileConnectedWaitsWithoutARestartOrStop() {
        SharerLinkPolicy p = waitingPolicy();
        assertTrue(p.isWaiting());
        assertFalse(p.isStopped());
        assertFalse(p.wantsWakeLock());
        assertEquals("", p.restartSet());
    }

    @Test
    public void emptyThenTheSameSetRestartsExactlyOnce() {
        SharerLinkPolicy p = waitingPolicy();
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 4000, false));
        assertEquals(LAN, p.restartSet());
        assertFalse(p.isWaiting());
        assertTrue(p.wantsWakeLock());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 5000, false));
    }

    @Test
    public void emptyThenADifferentSetRestartsOnceOnTheNewSet() {
        SharerLinkPolicy p = waitingPolicy();
        String next = "10.0.0.2";
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(next, 4000, false));
        assertEquals(next, p.restartSet());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(next, 5000, false));
    }

    @Test
    public void aUserStopWhileWaitingStopsAndDropsTheWakeLock() {
        SharerLinkPolicy p = waitingPolicy();
        p.userStop();
        assertTrue(p.isStopped());
        assertFalse(p.isWaiting());
        assertFalse(p.wantsWakeLock());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 4000, false));
    }

    @Test
    public void aRestartFailureFallsBackAndTheFinalReasonIsTheError() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        assertEquals(SharerLinkPolicy.Recovery.FALLBACK,
                p.restartFailed("bind failed", LAN));
        assertEquals(SharerLinkPolicy.Recovery.STOP,
                p.fallbackFailed("bind failed"));
        assertEquals("bind failed", p.stopReason());
        assertNotEquals("stopped", p.stopReason());
        assertTrue(p.isStopped());
        assertFalse(p.wantsWakeLock());
    }

    @Test
    public void aRestoreFailureWaitsAgainInsteadOfUsingTheVanishedSet() {
        SharerLinkPolicy p = waitingPolicy();
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick("10.0.0.2", 4000, false));
        assertEquals(SharerLinkPolicy.Recovery.WAIT,
                p.restartFailed("bind failed", LAN));
        assertTrue(p.isWaiting());
        assertFalse(p.isStopped());
        assertFalse(p.wantsWakeLock());
        assertEquals("", p.stopReason());
    }

    @Test
    public void aCodeSessionWaitsAndRestoresWithoutANewCode() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, true);
        p.tick(LAN, 0, true);
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("", 1000, true));
        assertEquals(SharerLinkPolicy.Action.WAIT, p.tick("", 3000, true));
        assertTrue(p.isCodeSession());
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 4000, false));
        assertEquals(LAN, p.restartSet());
        assertTrue(p.isCodeSession());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 5000, false));
    }

    @Test
    public void anEmptySetAtStartWaitsForTheHotspot() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start("", false);
        assertTrue(p.isWaiting());
        assertTrue(p.waitingFromStart());
        assertFalse(p.wantsWakeLock());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("", 3000, true));
        assertTrue(p.isWaiting());
    }

    @Test
    public void aSetThatAppearsAfterAStartWithoutHotspotRestartsOnce() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start("", false);
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("", 1000, false));
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 2000, false));
        assertEquals(LAN, p.restartSet());
        assertFalse(p.isWaiting());
        assertFalse(p.waitingFromStart());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 3000, false));
    }

    @Test
    public void aClatOnlySetAtStartCountsAsTheHotspotBeingOff() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start("192.0.0.4", false);
        assertTrue(p.isWaiting());
        assertTrue(p.waitingFromStart());
        // A CLAT address is not a hotspot LAN, so the same leftover keeps the
        // session waiting instead of restarting on it.
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("192.0.0.4", 4000, false));
        assertTrue(p.isWaiting());
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 5000, false));
        assertEquals(LAN, p.restartSet());
    }

    @Test
    public void aCodeSessionThatStartsWithoutAHotspotWaitsAndRestores() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start("", true);
        assertTrue(p.isWaiting());
        assertTrue(p.isCodeSession());
        assertTrue(p.waitingFromStart());
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 2000, false));
        assertEquals(LAN, p.restartSet());
        assertTrue(p.isCodeSession());
        assertFalse(p.isWaiting());
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 3000, false));
    }

    @Test
    public void aSystemApOffReportVetoesANonEmptySet() {
        assertTrue(SharerLinkPolicy.hotspotReady(true, null));
        assertTrue(SharerLinkPolicy.hotspotReady(true, Boolean.TRUE));
        assertFalse(SharerLinkPolicy.hotspotReady(true, Boolean.FALSE));
        // An AP-on hint never starts sharing without a usable set.
        assertFalse(SharerLinkPolicy.hotspotReady(false, Boolean.TRUE));
        assertFalse(SharerLinkPolicy.hotspotReady(false, null));
    }

    @Test
    public void aHotspotOffReportWaitsBeforeTheNextInterfacePoll() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        assertTrue(p.hotspotDisabled(1000, LAN));
        assertTrue(p.isWaiting());
        assertFalse(p.wantsWakeLock());
        assertFalse(p.hotspotDisabled(2000, LAN));
    }

    @Test
    public void aStillListedInterfaceCannotRestartDuringTheOffHold() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        p.hotspotDisabled(1000, LAN);
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 1500, true));
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick("", 1800, true));
        assertTrue(p.isWaiting());
    }

    @Test
    public void anUnknownLeftoverInterfaceCannotEndTheWaitAfterTheHold() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        p.hotspotDisabled(1000, LAN);
        // The hold is over, but the set is the same one seen at the off
        // report: it is a leftover interface, not the hotspot coming back.
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 3000, false));
        assertTrue(p.isWaiting());
        // A different usable set is a real restore on the broadcast path.
        assertEquals(SharerLinkPolicy.Action.RESTART,
                p.tick("192.168.1.5", 4000, false));
        assertFalse(p.isWaiting());
    }

    @Test
    public void anOnReportLetsTheNextPollRestore() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        p.hotspotDisabled(1000, LAN);
        p.hotspotEnabled();
        assertEquals(SharerLinkPolicy.Action.RESTART, p.tick(LAN, 1500, false));
        assertEquals(LAN, p.restartSet());
        assertFalse(p.isWaiting());
    }

    @Test
    public void aUserStopClearsTheHotspotOffHold() {
        SharerLinkPolicy p = new SharerLinkPolicy();
        p.start(LAN, false);
        p.tick(LAN, 0, true);
        p.hotspotDisabled(1000, LAN);
        p.userStop();
        assertEquals(SharerLinkPolicy.Action.NONE, p.tick(LAN, 1500, false));
        assertTrue(p.isStopped());
    }
}
