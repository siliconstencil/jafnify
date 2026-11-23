package com.siliconstencil.jafnify;

/**
 * The use-at-your-own-risk acceptance. The accepted version is stored in the
 * app's preferences; sharing does
 * not start before the current version is accepted. The text itself is in
 * res/values/strings.xml (terms_body), the same words as jafnify-core.
 */
final class Terms {
    /** Bump with the text: every user accepts the new text once. */
    static final int VERSION = 1;
    /** SharedPreferences key (in SharerService.PREFS) for the accepted version. */
    static final String PREF_KEY = "terms_version";

    private Terms() {
    }

    /** True when the stored version (0 = nothing stored) covers the current text. */
    static boolean accepted(int storedVersion) {
        return storedVersion >= VERSION;
    }
}
