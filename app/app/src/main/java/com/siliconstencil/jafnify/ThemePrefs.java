package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * Persists the {@link ThemeMode} in the app's private preferences. The
 * load/save methods use only {@link Store}, so unit tests need no Android
 * framework; {@link #shared} adapts the real SharedPreferences file.
 */
public final class ThemePrefs {
    /** Private preferences file for UI settings (not sharer state). */
    static final String FILE = "jafnify_ui";
    /** Stable key for the chosen theme. */
    static final String PREF_KEY = "theme_mode";

    /** The two operations the pure load/save path needs. */
    public interface Store {
        String get(String key);

        void put(String key, String value);
    }

    private ThemePrefs() {
    }

    /** The stored mode; an empty or invalid value means SYSTEM. */
    public static ThemeMode load(Store store) {
        if (store == null) {
            return ThemeMode.SYSTEM;
        }
        return ThemeMode.fromKey(store.get(PREF_KEY));
    }

    /** Stores the lowercase key of the mode; null means SYSTEM. */
    public static void save(Store store, ThemeMode mode) {
        if (store == null) {
            return;
        }
        store.put(PREF_KEY, (mode == null ? ThemeMode.SYSTEM : mode).key());
    }

    /** The production Store, backed by the private jafnify_ui preferences. */
    public static Store shared(Context context) {
        final SharedPreferences prefs =
                context.getSharedPreferences(FILE, Context.MODE_PRIVATE);
        return new Store() {
            @Override
            public String get(String key) {
                return prefs.getString(key, null);
            }

            @Override
            public void put(String key, String value) {
                prefs.edit().putString(key, value).apply();
            }
        };
    }
}
