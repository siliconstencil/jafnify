package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

/** The hotspot broadcast state values, decoded without Android types. */
public class HotspotWatchTest {
    @Test
    public void disablingAndDisabledMeanTheHotspotIsGoingAway() {
        assertEquals(HotspotWatch.Signal.OFF,
                HotspotWatch.signal(HotspotWatch.WIFI_AP_STATE_DISABLING));
        assertEquals(HotspotWatch.Signal.OFF,
                HotspotWatch.signal(HotspotWatch.WIFI_AP_STATE_DISABLED));
    }

    @Test
    public void enabledMeansTheExistingPollMayRestore() {
        assertEquals(HotspotWatch.Signal.ON,
                HotspotWatch.signal(HotspotWatch.WIFI_AP_STATE_ENABLED));
    }

    @Test
    public void enablingFailedAndUnknownStatesAreNotAnOffOrOnReport() {
        assertEquals(HotspotWatch.Signal.NONE,
                HotspotWatch.signal(HotspotWatch.WIFI_AP_STATE_ENABLING));
        assertEquals(HotspotWatch.Signal.NONE,
                HotspotWatch.signal(HotspotWatch.WIFI_AP_STATE_FAILED));
        assertEquals(HotspotWatch.Signal.NONE, HotspotWatch.signal(-1));
        assertEquals(HotspotWatch.Signal.NONE, HotspotWatch.signal(0));
    }

    @Test
    public void theApStateIsAKnownHintOnlyForOffAndOn() {
        assertEquals(Boolean.FALSE,
                HotspotWatch.apOn(HotspotWatch.WIFI_AP_STATE_DISABLED));
        assertEquals(Boolean.FALSE,
                HotspotWatch.apOn(HotspotWatch.WIFI_AP_STATE_DISABLING));
        assertEquals(Boolean.TRUE,
                HotspotWatch.apOn(HotspotWatch.WIFI_AP_STATE_ENABLED));
        assertNull(HotspotWatch.apOn(HotspotWatch.WIFI_AP_STATE_ENABLING));
        assertNull(HotspotWatch.apOn(HotspotWatch.WIFI_AP_STATE_FAILED));
        assertNull(HotspotWatch.apOn(-1));
    }
}
