package com.siliconstencil.jafnify;

/**
 * The once-per-off-period rule for the hotspot alert notification. One off
 * period may alert once; only the hotspot coming back (or a stop) clears it,
 * so a dismissed alert is not raised again until the next off period.
 */
final class HotspotAlert {
    private boolean alerted;

    /** True the first time an off period is observed; false afterwards. */
    boolean off() {
        if (alerted) {
            return false;
        }
        alerted = true;
        return true;
    }

    /** The hotspot is back or sharing stopped: the next off period may alert. */
    void clear() {
        alerted = false;
    }

    boolean isAlerted() {
        return alerted;
    }
}
