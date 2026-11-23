package com.siliconstencil.jafnify;

/** The two battery state labels; the action opens the system list, no request. */
final class BatteryHelp {
    private BatteryHelp() {
    }

    /** The label for the current system state. */
    static int stateRes(boolean ignoringOptimizations) {
        return ignoringOptimizations
                ? R.string.battery_unrestricted
                : R.string.battery_optimised;
    }
}
