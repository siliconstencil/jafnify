package com.siliconstencil.jafnify;

import android.content.Context;

/**
 * The clock the user picked on the phone (12-hour in the US by default).
 * Text is always English; only the hour style follows the setting.
 */
final class Clock {
    /** Set from the system setting; tests set it directly. */
    static volatile boolean twelveHour = false;

    private Clock() {}

    /** Read the phone's 12/24-hour setting; called when a screen or the service starts. */
    static void follow(Context context) {
        twelveHour = !android.text.format.DateFormat.is24HourFormat(context);
    }

    /** {@code HH:mm:ss} or {@code h:mm:ss a}. */
    static String seconds() {
        return twelveHour ? "h:mm:ss a" : "HH:mm:ss";
    }

    /** {@code HH:mm} or {@code h:mm a}. */
    static String minutes() {
        return twelveHour ? "h:mm a" : "HH:mm";
    }
}
