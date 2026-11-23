package com.siliconstencil.jafnify;

/**
 * The phone permission state machine. It is pure so every branch is testable
 * on the JVM: what the system says, what we asked and what we do next.
 */
final class PhonePermission {
    enum State {
        /** The user has not turned the feature on yet. */
        NOT_ASKED,
        /** The user allowed the phone's network permission. */
        GRANTED,
        /** The user said no once; the system would ask again. */
        DENIED,
        /** The user said no for good; only the system settings can change it. */
        DONT_ASK_AGAIN,
        /** The permission was taken away while the app was running. */
        REVOKED,
        /** The system is too old for a runtime permission. */
        UNSUPPORTED
    }

    private PhonePermission() {
    }

    /**
     * The listener for the generation and the signal (TelephonyCallback) exists
     * from API 31; below that the permission would give nothing, so it is not
     * asked at all.
     */
    static final int MIN_SDK = 31;

    /** The state before the user turns the switch on. */
    static State initial(int sdkInt) {
        return sdkInt < MIN_SDK ? State.UNSUPPORTED : State.NOT_ASKED;
    }

    /** The state after the system dialog was answered. */
    static State afterRequest(int sdkInt, boolean granted, boolean shouldShowRationale) {
        if (sdkInt < MIN_SDK) {
            return State.UNSUPPORTED;
        }
        if (granted) {
            return State.GRANTED;
        }
        return shouldShowRationale ? State.DENIED : State.DONT_ASK_AGAIN;
    }

    /** The state right now, from what the system says and what we asked. */
    static State current(int sdkInt, boolean granted, boolean shouldShowRationale, boolean asked) {
        if (sdkInt < MIN_SDK) {
            return State.UNSUPPORTED;
        }
        if (granted) {
            return State.GRANTED;
        }
        if (!asked) {
            return State.NOT_ASKED;
        }
        return shouldShowRationale ? State.DENIED : State.DONT_ASK_AGAIN;
    }

    /** The permission was taken away while the app was running. */
    static State revoked() {
        return State.REVOKED;
    }

    /** The user has to open the system settings to grant it. */
    static boolean needsSettings(State state) {
        return state == State.DONT_ASK_AGAIN || state == State.REVOKED;
    }

    /** A switch that is on must turn off when the permission is gone. */
    static boolean shouldTurnOff(boolean granted, boolean reportOn, boolean signalOn) {
        return !granted && (reportOn || signalOn);
    }

    /** The level M0 may carry: only while a switch is on and the permission is granted. */
    static int gatedSignal(boolean granted, boolean reportOn, boolean signalOn, int level) {
        return gatedSignal(granted, reportOn || signalOn, level);
    }

    /**
     * The level M0 may carry: the best-spot switch opens the same gate, so
     * the phone can measure its own spots while the report switches are off.
     */
    static int gatedSignal(boolean granted, boolean reportOn, boolean signalOn,
            boolean bestSpotOn, int level) {
        return gatedSignal(granted, reportOn || signalOn || bestSpotOn, level);
    }

    /** The dBm M0 may carry: the same gates as the signal level. */
    static int gatedDbm(boolean granted, boolean reportOn, boolean signalOn, int dbm) {
        return gatedDbm(granted, reportOn || signalOn, dbm);
    }

    /**
     * The dBm M0 may carry: the same gates as the signal level, so the
     * strength travels exactly when the level travels.
     */
    static int gatedDbm(boolean granted, boolean reportOn, boolean signalOn,
            boolean bestSpotOn, int dbm) {
        return gatedDbm(granted, reportOn || signalOn || bestSpotOn, dbm);
    }

    /** The level for one open gate. */
    private static int gatedSignal(boolean granted, boolean switchOn, int level) {
        if (!granted || !switchOn) {
            return -1;
        }
        return level >= 0 && level <= 4 ? level : -1;
    }

    /** The strength for one open gate. */
    private static int gatedDbm(boolean granted, boolean switchOn, int dbm) {
        if (!granted || !switchOn) {
            return -1;
        }
        return dbm >= -200 && dbm <= -2 ? dbm : -1;
    }

    /** The generation is only readable from API 30 on. */
    static boolean generationAvailable(int sdkInt) {
        return sdkInt >= MIN_SDK;
    }

    /** The one line to show under the switch for this state. */
    static String reasonLine(State state, String denied, String dontAskAgain, String revoked,
            String unsupported) {
        switch (state) {
            case DENIED:
                return denied;
            case DONT_ASK_AGAIN:
                return dontAskAgain;
            case REVOKED:
                return revoked;
            case UNSUPPORTED:
                return unsupported;
            default:
                return "";
        }
    }
}
