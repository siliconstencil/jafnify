package com.siliconstencil.jafnify;

/**
 * The Wi-Fi lock mode the sharer holds while it serves. The reference
 * products (PdaNet, PairVPN) hold WIFI_MODE_FULL_HIGH_PERF for the whole
 * serving period; Android 10 added WIFI_MODE_FULL_LOW_LATENCY as the
 * documented replacement for latency-sensitive work, so use it on API 29+
 * and keep HIGH_PERF on older phones. The values are the platform constants
 * (3 and 4); they are repeated here so the choice runs in the JVM tests.
 */
final class RadioLock {
    /** WifiManager.WIFI_MODE_FULL_HIGH_PERF (API 1+, deprecated in API 29). */
    static final int MODE_HIGH_PERF = 3;
    /** WifiManager.WIFI_MODE_FULL_LOW_LATENCY (API 29+). */
    static final int MODE_LOW_LATENCY = 4;

    private RadioLock() {
    }

    /** The lock mode for this API level. */
    static int mode(int sdk) {
        return sdk >= 29 ? MODE_LOW_LATENCY : MODE_HIGH_PERF;
    }
}
