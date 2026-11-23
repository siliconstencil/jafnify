package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * The "start sharing automatically when Jafnify opens" choice.
 *
 * <p>The decision and persistence paths need no Android framework, so unit
 * tests exercise them with a plain map. {@link #shared} adapts the real
 * private preferences file.
 */
public final class Autostart {
    /** Private preferences file for UI settings (shared with the theme). */
    static final String FILE = "jafnify_ui";
    /** Stable key; an absent value means OFF (existing installs). */
    static final String PREF_KEY = "start_on_open";
    /** Stable key for the opt-in resume after a system kill; absent is OFF. */
    static final String RESUME_KEY = "resume_if_stopped";
    /** Stable key for the opt-in boot start; the core id is the key. */
    static final String START_AFTER_RESTART_KEY = "start_after_restart";

    /** The operations the tested path needs; SharedPreferences adapts. */
    public interface Store {
        boolean getBoolean(String key, boolean def);

        void putBoolean(String key, boolean value);

        /**
         * Whether a value was ever stored under a key. The merged Network
         * details row needs it to tell "never chosen" from "chosen off"; a
         * store that does not know has no value, which is the old path.
         */
        default boolean contains(String key) {
            return false;
        }
    }

    private Autostart() {
    }

    /** The stored choice; an empty or missing value means OFF. */
    public static boolean load(Store store) {
        return store != null && store.getBoolean(PREF_KEY, false);
    }

    /** Stores the choice. Turning it on never starts a session by itself. */
    public static void save(Store store, boolean on) {
        if (store != null) {
            store.putBoolean(PREF_KEY, on);
        }
    }

    /** The stored resume choice; an empty or missing value means OFF. */
    public static boolean loadResume(Store store) {
        return store != null && store.getBoolean(RESUME_KEY, false);
    }

    /** Stores the resume choice. Turning it on never starts a session. */
    public static void saveResume(Store store, boolean on) {
        if (store != null) {
            store.putBoolean(RESUME_KEY, on);
        }
    }

    /** The stored boot-start choice; an empty or missing value means OFF. */
    public static boolean loadStartAfterRestart(Store store) {
        return store != null && store.getBoolean(START_AFTER_RESTART_KEY, false);
    }

    /** The production Store, backed by the private jafnify_ui preferences. */
    public static Store shared(Context context) {
        final SharedPreferences prefs =
                context.getSharedPreferences(FILE, Context.MODE_PRIVATE);
        return new Store() {
            @Override
            public boolean getBoolean(String key, boolean def) {
                return prefs.getBoolean(key, def);
            }

            @Override
            public void putBoolean(String key, boolean value) {
                prefs.edit().putBoolean(key, value).apply();
            }

            @Override
            public boolean contains(String key) {
                return prefs.contains(key);
            }
        };
    }
}
