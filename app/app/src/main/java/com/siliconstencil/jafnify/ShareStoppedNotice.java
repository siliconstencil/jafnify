package com.siliconstencil.jafnify;

/**
 * The one line under the status on the first app open after the system closed
 * the sharing app. Pure, so the JVM test holds the decision still; the words
 * come from the core through {@link Native#stoppedLine(int, long)}.
 */
final class ShareStoppedNotice {
    private ShareStoppedNotice() {
    }

    /** Android ApplicationExitInfo reasons this decision cares about. */
    static final int EXIT_SELF = 1;
    static final int USER_REQUESTED = 10;
    static final int USER_STOPPED = 11;
    static final int PACKAGE_UPDATED = 16;

    /** The user did it, or knows about the update: nothing to report. */
    static boolean isUserStop(int reason) {
        return reason == EXIT_SELF || reason == USER_REQUESTED
                || reason == USER_STOPPED || reason == PACKAGE_UPDATED;
    }

    /**
     * Show the line only when the app is not sharing now, it was sharing when
     * the process ended (an idle app closed by Android is normal), the exit is recent,
     * the user did not stop it, and this same exit was not already shown.
     * {@code exitTimestampMs} is Android's exit time, so a later system stop
     * shows the line again while the same one stays quiet.
     */
    static boolean shouldShow(boolean sharing, boolean wasSharingAtExit, long ageMs,
            int reason, long exitTimestampMs, long lastShownExitMs) {
        if (sharing || !wasSharingAtExit) {
            return false;
        }
        if (ageMs < 0 || ageMs > ExitReasons.RECENT_MS) {
            return false;
        }
        if (exitTimestampMs <= 0) {
            return false;
        }
        if (isUserStop(reason)) {
            return false;
        }
        return exitTimestampMs != lastShownExitMs;
    }
}
