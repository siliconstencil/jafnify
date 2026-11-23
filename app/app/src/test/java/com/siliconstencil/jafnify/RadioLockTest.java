package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The Wi-Fi lock mode follows the API: LOW_LATENCY from Android 10. */
public class RadioLockTest {
    @Test
    public void low_latency_is_used_from_api_29() {
        assertEquals(RadioLock.MODE_HIGH_PERF, RadioLock.mode(28));
        assertEquals(RadioLock.MODE_LOW_LATENCY, RadioLock.mode(29));
        assertEquals(RadioLock.MODE_LOW_LATENCY, RadioLock.mode(35));
    }

    @Test
    public void older_phones_keep_high_performance() {
        assertEquals(RadioLock.MODE_HIGH_PERF, RadioLock.mode(21));
        assertEquals(RadioLock.MODE_HIGH_PERF, RadioLock.mode(23));
    }

    @Test
    public void the_modes_are_the_platform_values() {
        // WifiManager.WIFI_MODE_FULL_HIGH_PERF = 3, ..._LOW_LATENCY = 4.
        assertEquals(3, RadioLock.MODE_HIGH_PERF);
        assertEquals(4, RadioLock.MODE_LOW_LATENCY);
    }
}
