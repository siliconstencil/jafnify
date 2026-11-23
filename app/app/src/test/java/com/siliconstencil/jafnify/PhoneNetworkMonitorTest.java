package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

public class PhoneNetworkMonitorTest {
    // The raw numbers the modem reports (android.telephony.TelephonyDisplayInfo,
    // checked with javap on android-35). 0.2.0 had 2 and 5 swapped, so a 5G UW
    // phone (NR_ADVANCED = 5) was reported as 4G+.
    @Test
    public void theOverrideNumbersMapToTheirGeneration() {
        int lte = 13;
        int nr = 20;
        assertEquals("lte_ca", PhoneNetworkMonitor.kindFor(1, lte));
        assertEquals("lte_advanced_pro", PhoneNetworkMonitor.kindFor(2, lte));
        assertEquals("nr_nsa", PhoneNetworkMonitor.kindFor(3, lte));
        assertEquals("nr_nsa_mmwave", PhoneNetworkMonitor.kindFor(4, lte));
        assertEquals("nr_advanced", PhoneNetworkMonitor.kindFor(5, lte));
        assertEquals("lte", PhoneNetworkMonitor.kindFor(0, lte));
        assertEquals("nr", PhoneNetworkMonitor.kindFor(0, nr));
        assertNull(PhoneNetworkMonitor.kindFor(0, 0));
    }

    // The dBm of the first cell strength that has one. The modem reports
    // CellInfo.UNAVAILABLE for a strength it does not know, and that value is
    // Integer.MAX_VALUE (the mockable android.jar of the unit tests has no
    // real field values, so the number is spelled out here).
    @Test
    public void theDbmPickSkipsUnavailableEntries() {
        assertEquals("the modem's unknown value is the Android one",
                Integer.MAX_VALUE, BestSpot.UNAVAILABLE);
        assertEquals("the first valid entry wins",
                -95, PhoneNetworkMonitor.pickDbm(new int[] {
                        Integer.MAX_VALUE, Integer.MAX_VALUE, -95, -90}));
        assertEquals("a single valid entry is the value",
                -95, PhoneNetworkMonitor.pickDbm(new int[] {-95}));
        assertEquals("no valid entry means no dBm",
                -1, PhoneNetworkMonitor.pickDbm(new int[] {Integer.MAX_VALUE}));
        assertEquals("no list means no dBm", -1, PhoneNetworkMonitor.pickDbm(null));
    }
}
