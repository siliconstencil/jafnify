package com.siliconstencil.jafnify;

/**
 * The one start decision. Pure Java: the caller supplies the flag value, the
 * stored pair, the terms and the last user Stop. An automatic trigger (boot
 * or service restart) never wins over a Stop, and no trigger can ask for a
 * pairing code: the only action this class can return is ACTION_START.
 */
public final class StartPolicy {
    /** What caused the app to consider starting a session. */
    public enum Trigger {
        /** The user opened the app from scratch. */
        COLD_OPEN,
        /** The Activity was recreated (rotation or process-state restore). */
        RESTORE,
        /** The system restarted the service after killing the process. */
        SERVICE_RESTART,
        /** The device booted and the user has unlocked it once. */
        BOOT,
        /** The app was replaced by an update while it was sharing. */
        UPDATE
    }

    private StartPolicy() {
    }

    /** Whether this trigger may start a session. */
    public static boolean shouldStart(Trigger trigger, boolean settingOn,
            boolean hasStoredPair, boolean termsAccepted, boolean userStopped) {
        if (trigger == null) {
            return false;
        }
        switch (trigger) {
            case COLD_OPEN:
                // A manual open is the user's own action; a Stop is cleared by
                // the user opening the app, as in 0.1.1.
                return settingOn && hasStoredPair && termsAccepted;
            case SERVICE_RESTART:
            case BOOT:
            case UPDATE:
                return settingOn && hasStoredPair && termsAccepted && !userStopped;
            case RESTORE:
            default:
                return false;
        }
    }

    /**
     * The service action for this decision: ACTION_START or null. There is no
     * branch that returns ACTION_PAIR_NEW.
     */
    public static String actionFor(Trigger trigger, boolean settingOn,
            boolean hasStoredPair, boolean termsAccepted, boolean userStopped) {
        return shouldStart(trigger, settingOn, hasStoredPair, termsAccepted, userStopped)
                ? SharerService.ACTION_START
                : null;
    }
}
