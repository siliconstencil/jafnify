package com.siliconstencil.jafnify;

import java.util.List;

/**
 * The marks of an hour strip. The core sends the word of every hour; the
 * phone draws it. A word the phone does not know is drawn as unknown, so a
 * newer core never paints a wrong colour. The roles are the colour roles the
 * generated tokens carry.
 */
public final class HourMarks {
    /** An hour that was usually quick. */
    public static final String ROLE_OK = "ok";
    /** An hour that was usually busy. */
    public static final String ROLE_WAIT = "wait";
    /** An hour without a verdict. */
    public static final String ROLE_UNKNOWN = "unknown";
    /** An hour with a drop. */
    public static final String ROLE_ERR = "err";
    /** An hour that was away. */
    public static final String ROLE_MUTED = "muted";
    /**
     * An hour outside the window of the tile: not counted, drawn fainter than
     * an hour without a verdict and never like an absence or a drop.
     */
    public static final String ROLE_OUTSIDE = "outside";

    private HourMarks() {
    }

    /** The role of the word of an hour; an unknown word is unknown. */
    public static String roleOf(String word) {
        if ("quick".equals(word)) {
            return ROLE_OK;
        }
        if ("busy".equals(word)) {
            return ROLE_WAIT;
        }
        if ("drop".equals(word)) {
            return ROLE_ERR;
        }
        if ("away".equals(word)) {
            return ROLE_MUTED;
        }
        if ("outside".equals(word)) {
            return ROLE_OUTSIDE;
        }
        return ROLE_UNKNOWN;
    }

    /** The roles of the cells of a strip, in the order of the cells. */
    public static String[] roles(List<String> cells) {
        if (cells == null) {
            return new String[0];
        }
        String[] out = new String[cells.size()];
        for (int i = 0; i < cells.size(); i++) {
            out[i] = roleOf(wordOf(cells.get(i)));
        }
        return out;
    }

    /** The word of a cell: the part after the colon of "hour:word". */
    public static String wordOf(String cell) {
        if (cell == null) {
            return "";
        }
        int colon = cell.lastIndexOf(':');
        return colon < 0 ? cell.trim() : cell.substring(colon + 1).trim();
    }
}
