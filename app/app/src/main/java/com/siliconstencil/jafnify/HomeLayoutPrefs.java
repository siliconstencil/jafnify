package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.ArrayList;
import java.util.List;

/**
 * Stores the home layout in the app's private preferences and asks the core
 * for the default and the validation. The words of the choices stay in the
 * core; this only keeps the saved text and hands it back.
 */
public final class HomeLayoutPrefs {
    /** The key of the saved text; the value is the core's own format. */
    static final String KEY = "home_layout";

    /** One placeable tile: the core id and the word the picker shows. */
    public static final class Option {
        public final String id;
        public final String label;

        Option(String id, String label) {
            this.id = id;
            this.label = label;
        }
    }

    private HomeLayoutPrefs() {
    }

    /** The saved text, or the core default when nothing is stored yet. */
    public static String loadText(Context context) {
        String text = preferences(context).getString(KEY, null);
        if (text == null || text.isEmpty()) {
            text = defaultText();
        }
        return text == null ? "" : text;
    }

    /** Stores the text the core produced. */
    public static void saveText(Context context, String text) {
        preferences(context).edit().putString(KEY, text == null ? "" : text).apply();
    }

    /** The default layout of the phone, from the core. */
    public static String defaultText() {
        try {
            String text = Native.homeLayoutDefault();
            return text == null ? "" : text;
        } catch (Throwable ignored) {
            return "";
        }
    }

    /** Lets the core validate and fill a choice, and returns the saved text. */
    public static String normalize(int grid, String wide, List<String> ids) {
        StringBuilder csv = new StringBuilder();
        if (ids != null) {
            for (String id : ids) {
                if (id == null || id.isEmpty()) {
                    continue;
                }
                if (csv.length() > 0) {
                    csv.append(',');
                }
                csv.append(id);
            }
        }
        try {
            String text = Native.homeLayoutNormalize(String.valueOf(grid), wide,
                    csv.toString());
            if (text != null && !text.isEmpty()) {
                return text;
            }
        } catch (Throwable ignored) {
            // A plain JVM test has no native library; the caller falls back.
        }
        HomeLayout layout = new HomeLayout();
        layout.grid = grid;
        layout.wide = BOTTOM.equals(wide) ? BOTTOM : TOP;
        if (ids != null) {
            for (String id : ids) {
                if (id != null && !id.isEmpty() && !layout.slots.contains(id)) {
                    layout.slots.add(id);
                }
            }
        }
        return layout.toText();
    }

    /** The placeable tiles the picker offers, in the core's order. */
    public static List<Option> catalog() {
        try {
            return parseCatalog(Native.homeTileCatalog());
        } catch (Throwable ignored) {
            return new ArrayList<>();
        }
    }

    /** Parses id TAB label lines; a malformed line is skipped. */
    static List<Option> parseCatalog(String text) {
        List<Option> options = new ArrayList<>();
        if (text == null || text.isEmpty()) {
            return options;
        }
        for (String line : text.split("\n", -1)) {
            String[] fields = line.split("\t", -1);
            if (fields.length != 2 || fields[0].isEmpty() || fields[1].isEmpty()) {
                continue;
            }
            options.add(new Option(fields[0], fields[1]));
        }
        return options;
    }

    private static final String TOP = "top";
    private static final String BOTTOM = "bottom";

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(Autostart.FILE, Context.MODE_PRIVATE);
    }
}
