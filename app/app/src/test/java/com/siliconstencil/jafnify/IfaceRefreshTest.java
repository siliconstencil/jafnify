package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class IfaceRefreshTest {
    @Test
    public void firstSetIsTheBaselineAndDoesNotRestart() {
        IfaceRefresh r = new IfaceRefresh();
        assertFalse(r.update("10.0.0.1", 0));
    }

    @Test
    public void aStableChangeFiresOnlyAfterTheDebounce() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("10.0.0.1");
        assertFalse(r.update("10.0.0.1,192.168.43.1", 1000));
        assertFalse(r.update("10.0.0.1,192.168.43.1", 2999));
        assertTrue(r.update("10.0.0.1,192.168.43.1", 3000));
        assertFalse(r.update("10.0.0.1,192.168.43.1", 4000));
    }

    @Test
    public void revertingBeforeTheDebounceCancelsTheChange() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("10.0.0.1");
        assertFalse(r.update("10.0.0.1,192.168.43.1", 1000));
        assertFalse(r.update("10.0.0.1", 1500));
        assertFalse(r.update("10.0.0.1,192.168.43.1", 2000));
        assertFalse(r.update("10.0.0.1,192.168.43.1", 3999));
        assertTrue(r.update("10.0.0.1,192.168.43.1", 4000));
    }

    @Test
    public void addressOrderAndDuplicatesDoNotCauseARestart() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("192.168.43.1,10.0.0.1");
        assertFalse(r.update("10.0.0.1,192.168.43.1,10.0.0.1", 1000));
        assertFalse(r.update("10.0.0.1,192.168.43.1,10.0.0.1", 3000));
    }

    @Test
    public void aChangeDuringTheWindowRestartsTheDebounceClock() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("10.0.0.1");
        assertFalse(r.update("10.0.0.1,192.168.43.1", 1000));
        assertFalse(r.update("10.0.0.1,192.168.43.1,192.168.49.1", 2000));
        assertFalse(r.update("10.0.0.1,192.168.43.1,192.168.49.1", 3999));
        assertTrue(r.update("10.0.0.1,192.168.43.1,192.168.49.1", 4000));
    }

    @Test
    public void emptyMeansNoUsableInterfaceAndCanAlsoChange() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("");
        assertFalse(r.update("", 1000));
        assertFalse(r.update("192.168.43.1", 1000));
        assertFalse(r.update("192.168.43.1", 2999));
        assertTrue(r.update("192.168.43.1", 3000));
        assertFalse(r.update("", 4000));
        assertTrue(r.update("", 6000));
    }

    @Test
    public void aStableChangeWaitsWhileConnectedAndAppliesAfterDisconnect() {
        IfaceRefresh r = new IfaceRefresh();
        r.reset("10.0.0.1");
        assertFalse(r.update("10.0.0.1,192.168.43.1", 1000, true));
        assertFalse(r.update("10.0.0.1,192.168.43.1", 3000, true));
        assertTrue(r.update("10.0.0.1,192.168.43.1", 3001, false));
    }

    @Test
    public void normalizationIgnoresOrderDuplicatesAndWhitespace() {
        assertEquals("10.0.0.1,192.168.43.1",
                IfaceRefresh.normalize(" 192.168.43.1 , 10.0.0.1,10.0.0.1 "));
    }

    @Test
    public void normalizedLinkChangesOnlyFireOnARealSetChange() {
        // Enumeration order and duplicates are the same set: no log/counter.
        assertEquals(IfaceRefresh.LinkChange.NONE, IfaceRefresh.linkChange(
                "10.0.0.1,192.168.43.1", "192.168.43.1,10.0.0.1,10.0.0.1"));
        // Only the empty boundary is a loss or a restore.
        assertEquals(IfaceRefresh.LinkChange.RESTORE,
                IfaceRefresh.linkChange("", "192.168.43.1"));
        assertEquals(IfaceRefresh.LinkChange.LOSS,
                IfaceRefresh.linkChange("10.0.0.1,192.168.43.1", ""));
        // A non-empty swap is a logged change, not a loss or a restore.
        assertEquals(IfaceRefresh.LinkChange.CHANGE,
                IfaceRefresh.linkChange("10.0.0.1", "192.168.43.1"));
        assertEquals(IfaceRefresh.LinkChange.CHANGE, IfaceRefresh.linkChange(
                "10.0.0.1,192.168.43.1", "10.0.0.1,192.168.49.1"));
    }
}
