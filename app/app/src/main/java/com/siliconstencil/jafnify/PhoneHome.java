package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;

/**
 * The home model of the phone, parsed from the text the core sends. The core
 * owns every word and every number; this only reads the block into plain
 * fields the screen can draw. A text without the header is not a home.
 */
public final class PhoneHome {
    /** The header the core sends. */
    static final String HEADER = "JFH1 1";

    /** The phone does not share. */
    public static final int IDLE = 0;
    /** The phone tries to share. */
    public static final int TRYING = 1;
    /** The phone shares. */
    public static final int SHARING = 2;

    /**
     * The state of the sharing, as the core wants it: the card of the tile
     * home must not say "sharing" while the tunnel is still coming up.
     */
    public static int stateCode(boolean running, boolean connected) {
        if (!running) {
            return IDLE;
        }
        return connected ? SHARING : TRYING;
    }

    /**
     * The hotspot is off: the card carries that sentence itself, not the
     * core's "Trying to share" title with the sentence under it. The tone
     * stays "wait", so the dot keeps the colour of a phone that is waiting.
     * The notice chip is the link's path line ("Direct over hotspot"): it is
     * a claim about the tunnel and would contradict the card, so it goes too.
     */
    public static void showHotspotOff(Model model, String text) {
        if (model == null) {
            return;
        }
        model.status.title = text;
        model.status.subline = "";
        model.status.notice = "";
        model.status.tone = "wait";
    }

    /**
     * The tile ids whose number is measured on the laptop or through the
     * tunnel. Everything else (signal, DNS, heat, battery, today's data) is
     * measured on the phone itself or is a running total, so it may stay.
     */
    static boolean fromLink(String id) {
        return "range".equals(id) || "speed".equals(id);
    }

    /**
     * While the link is not current (the sharer waits for the hotspot or is
     * stopped), a link tile keeps its label but shows no live number. The
     * history line and the peak may stay; a "--" value and the "No link"
     * facts make it clear that nothing of it is current.
     */
    public static void hideLinkValues(Model model) {
        if (model == null) {
            return;
        }
        for (Tile tile : model.tiles) {
            if (fromLink(tile.id)) {
                hideLinkValue(tile,
                        "speed".equals(tile.id) ? "No link" : "No link to the laptop");
            }
        }
    }

    /** Replaces the live number and its graph of one link tile. */
    private static void hideLinkValue(Tile tile, String fact) {
        tile.value = "--";
        tile.unit = "";
        tile.tone = "muted";
        tile.kind = "none";
        tile.points = new int[0];
        tile.hasReference = false;
        tile.reference = 0;
        tile.level = 0;
        tile.levelMax = 0;
        tile.fill = 0;
        tile.fillMax = 0;
        tile.down = 0;
        tile.up = 0;
        tile.cells.clear();
        tile.facts.clear();
        tile.facts.add(fact);
        tile.note = "";
    }

    /** The card at the top of the screen. */
    public static final class Status {
        /** Whether the sharing runs. */
        public boolean connected;
        /** The tone of the card: "ok", "wait", "err" or "muted". */
        public String tone = "muted";
        /** The line at the top. */
        public String title = "";
        /** The line under it. */
        public String subline = "";
        /** Whether the card offers the stop action. */
        public boolean stop;
        /** The word on the button. */
        public String action = "";
        /** The notice chip inside the card. */
        public String notice = "";
    }

    /** One tile of the grid. */
    public static final class Tile {
        /** The id of the tile. */
        public String id = "";
        /** The label of the tile, in caps. */
        public String label = "";
        /** The small word at the top, when the tile has one. */
        public String badge = "";
        /** The tone of the badge. */
        public String badgeTone = "ok";
        /** The number the tile leads with. */
        public String value = "";
        /** The unit of the number. */
        public String unit = "";
        /** The tone of the number. */
        public String tone = "muted";
        /** "line", "split", "fill", "hours" or "none". */
        public String kind = "none";
        /** The points of the line. */
        public int[] points = new int[0];
        /** The dashed line to compare against. */
        public int reference;
        /** Whether the tile has a reference line. */
        public boolean hasReference;
        /** The level of the signal bars. */
        public int level;
        /** How many level bars there are. */
        public int levelMax;
        /** How much of the bar is full. */
        public int fill;
        /** The whole of the bar. */
        public int fillMax;
        /** The bytes that came down. */
        public long down;
        /** The bytes that went up. */
        public long up;
        /** The cells of an hour strip, each one "hour:mark". */
        public final List<String> cells = new ArrayList<>();
        /** The short lines under the drawing. */
        public final List<String> facts = new ArrayList<>();
        /** One line under the facts. */
        public String note = "";
    }

    /** The card and the tiles. */
    public static final class Model {
        /** The card at the top. */
        public Status status = new Status();
        /** The tiles of the grid. */
        public final List<Tile> tiles = new ArrayList<>();
    }

    private PhoneHome() {
    }

    /** Parses the text of the core; an unknown text gives an empty model. */
    public static Model parse(String text) {
        Model model = new Model();
        if (text == null || text.isEmpty()) {
            return model;
        }
        String[] lines = text.split("\n");
        if (lines.length == 0 || !lines[0].trim().equals(HEADER)) {
            return model;
        }
        for (int i = 1; i < lines.length; i++) {
            String line = lines[i].trim();
            if (line.startsWith("s ")) {
                readStatus(model.status, fields(line));
            } else if (line.startsWith("t ")) {
                model.tiles.add(readTile(fields(line)));
            }
        }
        return model;
    }

    /** The fields of one line: the pairs between the separators. */
    private static List<String[]> fields(String line) {
        List<String[]> out = new ArrayList<>();
        for (String part : line.split(" \\| ")) {
            int equals = part.indexOf('=');
            if (equals > 0) {
                out.add(new String[] {part.substring(0, equals).trim(),
                        part.substring(equals + 1).trim()});
            }
        }
        return out;
    }

    private static void readStatus(Status status, List<String[]> fields) {
        for (String[] field : fields) {
            String key = field[0];
            String value = field[1];
            if ("connected".equals(key)) {
                status.connected = "1".equals(value);
            } else if ("tone".equals(key)) {
                status.tone = value;
            } else if ("title".equals(key)) {
                status.title = value;
            } else if ("sub".equals(key)) {
                status.subline = value;
            } else if ("stop".equals(key)) {
                status.stop = "1".equals(value);
            } else if ("action".equals(key)) {
                status.action = value;
            } else if ("notice".equals(key)) {
                status.notice = value;
            }
        }
    }

    private static Tile readTile(List<String[]> fields) {
        Tile tile = new Tile();
        for (String[] field : fields) {
            String key = field[0];
            String value = field[1];
            if ("id".equals(key)) {
                tile.id = value;
            } else if ("label".equals(key)) {
                tile.label = value;
            } else if ("badge".equals(key)) {
                tile.badge = value;
            } else if ("btone".equals(key)) {
                tile.badgeTone = value;
            } else if ("value".equals(key)) {
                tile.value = value;
            } else if ("unit".equals(key)) {
                tile.unit = value;
            } else if ("tone".equals(key)) {
                tile.tone = value;
            } else if ("kind".equals(key)) {
                tile.kind = value;
            } else if ("points".equals(key)) {
                tile.points = points(value);
            } else if ("ref".equals(key)) {
                tile.hasReference = !value.isEmpty();
                tile.reference = number(value);
            } else if ("level".equals(key)) {
                tile.level = number(value);
            } else if ("level_max".equals(key)) {
                tile.levelMax = number(value);
            } else if ("fill".equals(key)) {
                tile.fill = number(value);
            } else if ("fill_max".equals(key)) {
                tile.fillMax = number(value);
            } else if ("down".equals(key)) {
                tile.down = count(value);
            } else if ("up".equals(key)) {
                tile.up = count(value);
            } else if ("cells".equals(key)) {
                cells(tile, value);
            } else if ("fact".equals(key)) {
                tile.facts.add(value);
            } else if ("note".equals(key)) {
                tile.note = value;
            }
        }
        return tile;
    }

    /** The cells of an hour strip: the comma separated "hour:mark" entries. */
    private static void cells(Tile tile, String value) {
        if (value.isEmpty()) {
            return;
        }
        for (String cell : value.split(",")) {
            String clean = cell.trim();
            if (!clean.isEmpty()) {
                tile.cells.add(clean);
            }
        }
    }

    /** The points of a line: the comma separated numbers. */
    private static int[] points(String value) {
        if (value.isEmpty()) {
            return new int[0];
        }
        String[] parts = value.split(",");
        int[] out = new int[parts.length];
        for (int i = 0; i < parts.length; i++) {
            out[i] = number(parts[i]);
        }
        return out;
    }

    /** A number of the text; a word that does not parse is zero. */
    private static int number(String value) {
        try {
            return Integer.parseInt(value.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /**
     * A count of bytes: the day's totals pass two billion, so a byte count
     * needs the wide type. A word that does not parse is zero.
     */
    private static long count(String value) {
        try {
            return Long.parseLong(value.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }
}
