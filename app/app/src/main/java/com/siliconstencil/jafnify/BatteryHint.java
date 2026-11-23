package com.siliconstencil.jafnify;

/**
 * Whether the home shows the battery-optimisation hint. Doze suspends app
 * network access when the phone is unplugged, stationary and screen-off, and
 * the exemption list names a companion app that gives a peripheral internet
 * access. The hint is one sentence, only while sharing on battery without the
 * exemption, and it is dismissible for the rest of the sharing run.
 */
final class BatteryHint {
    private BatteryHint() {
    }

    /**
     * @param sharing            the sharer service is running
     * @param charging           the phone is on external power (unknown counts
     *                           as charging, so an unknown state never nags)
     * @param ignoringOptimisations the app is already on the exemption list
     * @param dismissedForRun    the user dismissed it for this sharing run
     */
    static boolean show(boolean sharing, boolean charging,
            boolean ignoringOptimisations, boolean dismissedForRun) {
        return sharing && !charging && !ignoringOptimisations && !dismissedForRun;
    }
}
