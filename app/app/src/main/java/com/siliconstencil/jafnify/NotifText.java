package com.siliconstencil.jafnify;

/**
 * The text the ongoing notification is showing. {@link #shown} records a text
 * pushed outside the state poll (a foreground start), so the next poll cannot
 * mistake it for the last state and leave the notification stale.
 */
final class NotifText {
    private String shown = null;

    /** Record a text that was just shown (startForeground). */
    void shown(String text) {
        shown = text;
    }

    /** The text to post now, or null when it is already showing. */
    String next(String text) {
        if (text == null || text.equals(shown)) {
            return null;
        }
        shown = text;
        return text;
    }
}
