package com.siliconstencil.jafnify;

/**
 * The system's hotspot on/off broadcast, decoded without Android types so the
 * decision runs in the JVM tests.
 *
 * <p>The action and the AP state values are not public API; Android sends
 * {@code android.net.wifi.WIFI_AP_STATE_CHANGED} with the state in the
 * {@code wifi_state} extra. Only disabling and disabled mean the hotspot is
 * going away. Enabling, enabled and failed are not an off signal: enabled is
 * left to the existing interface poll, which restores the session.
 */
final class HotspotWatch {
    static final String ACTION = "android.net.wifi.WIFI_AP_STATE_CHANGED";

    static final int WIFI_AP_STATE_DISABLING = 10;
    static final int WIFI_AP_STATE_DISABLED = 11;
    static final int WIFI_AP_STATE_ENABLING = 12;
    static final int WIFI_AP_STATE_ENABLED = 13;
    static final int WIFI_AP_STATE_FAILED = 14;

    enum Signal { NONE, OFF, ON }

    private HotspotWatch() {}

    static Signal signal(int wifiState) {
        if (wifiState == WIFI_AP_STATE_DISABLING || wifiState == WIFI_AP_STATE_DISABLED) {
            return Signal.OFF;
        }
        if (wifiState == WIFI_AP_STATE_ENABLED) {
            return Signal.ON;
        }
        return Signal.NONE;
    }

    /**
     * The same state as a start-time hint: true for on, false for off, null
     * when the system did not say. Only a true/false can veto a start; null
     * leaves the decision to the usable interface set.
     */
    static Boolean apOn(int wifiState) {
        Signal signal = signal(wifiState);
        if (signal == Signal.OFF) {
            return Boolean.FALSE;
        }
        if (signal == Signal.ON) {
            return Boolean.TRUE;
        }
        return null;
    }
}
