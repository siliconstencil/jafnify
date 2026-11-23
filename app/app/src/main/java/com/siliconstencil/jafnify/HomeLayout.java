package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The saved home layout, parsed from the text the core writes. Pure Java, no
 * Android classes: the tests check the round trip without a device. The core
 * owns the defaults and the validation; this only reads and writes the words.
 */
public final class HomeLayout {
    /** The grids the home offers. */
    public static final int[] GRIDS = {6, 7, 8};
    /** The word of the wide tile at the top of a 7 grid. */
    public static final String TOP = "top";
    /** The word of the wide tile at the bottom of a 7 grid. */
    public static final String BOTTOM = "bottom";

    private static final Pattern QUOTED = Pattern.compile("\"([^\"]*)\"");

    /** The grid size: 6, 7 or 8. */
    public int grid = 8;
    /** Where the wide tile sits in a 7 grid: top or bottom. */
    public String wide = TOP;
    /** One tile id per slot, in the order the user wants. */
    public final List<String> slots = new ArrayList<>();

    /** The number of slots the grid has. */
    public int slotCount() {
        return grid;
    }

    /** The slot of the wide tile, or -1 when the grid has none. */
    public int wideSlot() {
        if (grid != 7) {
            return -1;
        }
        return BOTTOM.equals(wide) ? 6 : 0;
    }

    /**
     * Parses the saved text. A null, empty or partial text keeps the default
     * for every field it does not carry; the core normalizes before saving.
     */
    public static HomeLayout parse(String text) {
        HomeLayout layout = new HomeLayout();
        if (text == null || text.isEmpty()) {
            return layout;
        }
        for (String raw : text.split("\n", -1)) {
            String line = raw.trim();
            if (line.startsWith("grid")) {
                Integer value = integerAfter(line);
                if (value != null && (value == 6 || value == 7 || value == 8)) {
                    layout.grid = value;
                }
            } else if (line.startsWith("wide")) {
                layout.wide = quotedAfter(line);
                if (!BOTTOM.equals(layout.wide)) {
                    layout.wide = TOP;
                }
            } else if (line.startsWith("slots")) {
                layout.slots.clear();
                Matcher matcher = QUOTED.matcher(line);
                while (matcher.find()) {
                    String id = matcher.group(1).trim();
                    if (!id.isEmpty()) {
                        layout.slots.add(id);
                    }
                }
            }
        }
        return layout;
    }

    private static Integer integerAfter(String line) {
        for (int i = 0; i < line.length(); i++) {
            char c = line.charAt(i);
            if (c >= '0' && c <= '9') {
                int end = i;
                while (end < line.length()
                        && line.charAt(end) >= '0' && line.charAt(end) <= '9') {
                    end++;
                }
                try {
                    return Integer.parseInt(line.substring(i, end));
                } catch (NumberFormatException e) {
                    return null;
                }
            }
        }
        return null;
    }

    private static String quotedAfter(String line) {
        Matcher matcher = QUOTED.matcher(line);
        return matcher.find() ? matcher.group(1).trim() : "";
    }

    /** The saved text, in the shape the core parses. */
    public String toText() {
        StringBuilder text = new StringBuilder();
        text.append("grid = ").append(grid).append('\n');
        text.append("wide = \"").append(wide).append("\"\n");
        text.append("slots = [");
        for (int i = 0; i < slots.size(); i++) {
            if (i > 0) {
                text.append(", ");
            }
            text.append('"').append(slots.get(i)).append('"');
        }
        text.append("]\n");
        return text.toString();
    }
}
