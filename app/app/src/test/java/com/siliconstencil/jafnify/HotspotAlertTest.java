package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** One alerting notification per off period, never more. */
public class HotspotAlertTest {
    @Test
    public void theFirstOffSignalAlertsAndLaterOnesDoNot() {
        HotspotAlert alert = new HotspotAlert();
        assertTrue(alert.off());
        assertTrue(alert.isAlerted());
        assertFalse(alert.off());
        assertFalse(alert.off());
    }

    @Test
    public void theHotspotComingBackAllowsOneAlertForTheNextPeriod() {
        HotspotAlert alert = new HotspotAlert();
        assertTrue(alert.off());
        alert.clear();
        assertFalse(alert.isAlerted());
        assertTrue(alert.off());
        assertFalse(alert.off());
    }

    @Test
    public void clearingWithoutAnOffPeriodChangesNothing() {
        HotspotAlert alert = new HotspotAlert();
        alert.clear();
        assertFalse(alert.isAlerted());
        assertTrue(alert.off());
    }
}
