package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.Context;
import android.content.res.Configuration;

/**
 * The app's theme choice: follow the system, or force dark or light. The app
 * uses the framework only (no AppCompat). An explicit mode is applied by
 * {@link #wrap}, which builds a configuration context for every Activity on
 * every API level; SYSTEM returns the base Context and follows the system.
 * The stored choice is in {@link ThemePrefs}.
 */
public enum ThemeMode {
    SYSTEM("system", "System"),
    DARK("dark", "Dark"),
    LIGHT("light", "Light");

    private final String key;
    private final String label;

    ThemeMode(String key, String label) {
        this.key = key;
        this.label = label;
    }

    /** The lowercase key stored in preferences. */
    public String key() {
        return key;
    }

    /** The English word shown in the theme control. */
    public String label() {
        return label;
    }

    /** Stored key -> mode; missing, unknown and invalid values mean SYSTEM. */
    public static ThemeMode fromKey(String key) {
        if (key != null) {
            String trimmed = key.trim();
            for (ThemeMode mode : values()) {
                if (mode.key.equalsIgnoreCase(trimmed)) {
                    return mode;
                }
            }
        }
        return SYSTEM;
    }

    /** True only for an explicit choice; SYSTEM follows the OS. */
    public boolean requiresOverride() {
        return this == DARK || this == LIGHT;
    }

    /**
     * Returns a Context with the requested night mode for an explicit choice,
     * or the original Context for SYSTEM. Activities call this from
     * {@code attachBaseContext}; this is the only theme override path, so a
     * switch back to SYSTEM takes effect on the next recreation.
     */
    public static Context wrap(Context base, ThemeMode mode) {
        if (base == null || mode == null || !mode.requiresOverride()) {
            return base;
        }
        Configuration config = new Configuration(base.getResources().getConfiguration());
        config.uiMode = (config.uiMode & ~Configuration.UI_MODE_NIGHT_MASK)
                | (mode == DARK ? Configuration.UI_MODE_NIGHT_YES
                : Configuration.UI_MODE_NIGHT_NO);
        return base.createConfigurationContext(config);
    }

    /**
     * Saves the choice and recreates the Activity so {@link #wrap} applies it.
     * A no-op when the choice did not change. No application-wide state is
     * changed: switching back to SYSTEM follows the system after recreation.
     */
    public static void apply(Activity activity, ThemeMode mode) {
        if (activity == null) {
            return;
        }
        ThemeMode chosen = mode == null ? SYSTEM : mode;
        ThemePrefs.Store store = ThemePrefs.shared(activity);
        if (ThemePrefs.load(store) == chosen) {
            return;
        }
        ThemePrefs.save(store, chosen);
        activity.recreate();
    }
}
