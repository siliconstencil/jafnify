package com.siliconstencil.jafnify;

/** Pure status decisions for the keep-running block; no Android calls. */
final class KeepRunning {
    static final int UNUSED_HIDDEN = -1;
    static final int UNUSED_CAN_REVOKE = 0;
    static final int UNUSED_OK = 1;

    static final int DATA_BLOCKED = 0;
    static final int DATA_OK = 1;
    static final int DATA_UNKNOWN = 2;

    private KeepRunning() {
    }

    /** API 30 and up can report auto-revoke; below that the row is hidden. */
    static int unused(int sdkInt, boolean whitelisted) {
        if (sdkInt < 30) {
            return UNUSED_HIDDEN;
        }
        return whitelisted ? UNUSED_OK : UNUSED_CAN_REVOKE;
    }

    /**
     * ConnectivityManager RESTRICT_BACKGROUND_STATUS codes: 1 disabled,
     * 2 whitelisted, 3 enabled. Anything else is unknown.
     */
    static int dataSaver(int restrictBackgroundStatus) {
        if (restrictBackgroundStatus == 1 || restrictBackgroundStatus == 2) {
            return DATA_OK;
        }
        if (restrictBackgroundStatus == 3) {
            return DATA_BLOCKED;
        }
        return DATA_UNKNOWN;
    }
}
