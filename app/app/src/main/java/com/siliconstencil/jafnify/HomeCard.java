package com.siliconstencil.jafnify;

/**
 * What the card of the tile home shows, in one place: the state, the two
 * lines of words, the word on the button and whether the pairing code is on
 * screen. Pure Java, no Android classes: the tests check every state without
 * a device, and the screen only draws the fields.
 *
 * <p>The core still owns the words. This model only reconciles the core's
 * card with what the service and the pairing code say, so no line can
 * contradict another: an idle card never says "Not sharing" twice, a code is
 * shown only while a pairing runs, and a stopped service never keeps a
 * leftover code or a "Stop" button.
 */
public final class HomeCard {
    /** The phone does not share. */
    public static final int IDLE = 0;
    /** The phone tries to share, or a start was requested. */
    public static final int TRYING = 1;
    /** The tunnel is up. */
    public static final int SHARING = 2;

    /** The state the card draws. */
    public final int state;
    /** The line at the top, from the core. */
    public final String title;
    /** The line under it; "" when it would only repeat the title. */
    public final String subline;
    /** The word on the one primary button. */
    public final String action;
    /** Whether the button stops the sharing. */
    public final boolean stop;
    /** Whether the pairing code belongs on the card. */
    public final boolean showCode;
    /** Whether the code has an expiry line to show. */
    public final boolean showExpiry;
    /** Whether a fresh code can be asked for. */
    public final boolean showNewCode;

    private HomeCard(int state, String title, String subline, String action,
            boolean stop, boolean showCode, boolean showExpiry, boolean showNewCode) {
        this.state = state;
        this.title = title == null ? "" : title;
        this.subline = subline == null ? "" : subline;
        this.action = action == null ? "" : action;
        this.stop = stop;
        this.showCode = showCode;
        this.showExpiry = showExpiry;
        this.showNewCode = showNewCode;
    }

    /**
     * The state of the card from three facts of the Activity, not from the
     * core text: a start that was requested already shows "trying", so the
     * button says Stop while the service is coming up. A failed start is
     * idle again, never a "trying" card with a "Share" button.
     */
    public static int stateFor(boolean running, boolean starting, boolean connected,
            boolean failed) {
        if (running) {
            return connected ? SHARING : TRYING;
        }
        if (starting && !failed) {
            return TRYING;
        }
        return IDLE;
    }

    /**
     * Reconciles the core's card with the code on screen. The code is shown
     * only while the card says the sharing runs and the tunnel is not up, so
     * a connected card never carries a code and an idle card never shows a
     * stale one. A subline equal to the title is dropped: one sentence, once.
     */
    public static HomeCard of(PhoneHome.Status status, String code, String expiry) {
        if (status == null) {
            return new HomeCard(IDLE, "", "", "", false, false, false, false);
        }
        int state = status.connected ? SHARING : (status.stop ? TRYING : IDLE);
        String subline = status.subline == null ? "" : status.subline;
        if (subline.equals(status.title)) {
            subline = "";
        }
        boolean showCode = status.stop && !status.connected
                && code != null && !code.isEmpty();
        return new HomeCard(state, status.title, subline, status.action, status.stop,
                showCode, showCode && expiry != null && !expiry.isEmpty(), showCode);
    }
}
