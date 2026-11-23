package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;

/**
 * The shared feature-flag table as sent by the native side.
 *
 * <p>A line has six TAB-separated fields: id, bucket, default (true/false),
 * platforms (bit mask), label and help. Malformed lines and unknown buckets
 * are skipped, so one bad row never hides the rest. There are no Android
 * imports here: the parser runs in a plain JVM test.
 */
public final class FeatureFlags {
    /** Bucket keys, in render order. */
    public static final String SETTINGS = "settings";
    public static final String ADVANCED = "advanced";
    public static final String EXPERIMENTAL = "experimental";

    /** One parsed row. */
    public static final class Flag {
        public final String id;
        public final String bucket;
        public final boolean defaultOn;
        public final int platforms;
        public final String label;
        public final String help;

        Flag(String id, String bucket, boolean defaultOn, int platforms,
                String label, String help) {
            this.id = id;
            this.bucket = bucket;
            this.defaultOn = defaultOn;
            this.platforms = platforms;
            this.label = label;
            this.help = help;
        }
    }

    /** One parsed bucket header. */
    public static final class BucketInfo {
        public final String key;
        public final String title;
        public final boolean collapsedByDefault;
        public final String note;

        BucketInfo(String key, String title, boolean collapsedByDefault,
                String note) {
            this.key = key;
            this.title = title;
            this.collapsedByDefault = collapsedByDefault;
            this.note = note;
        }
    }

    /** Whether a section joins the main list or is a collapsed group. */
    public enum Placement {
        MAIN,
        GROUP
    }

    /** One row with its stored value. */
    public static final class Row {
        public final String id;
        public final String label;
        public final String help;
        public final boolean value;

        Row(String id, String label, String help, boolean value) {
            this.id = id;
            this.label = label;
            this.help = help;
            this.value = value;
        }
    }

    /** One rendered section. */
    public static final class Section {
        public final String bucket;
        public final String title;
        public final boolean collapsedByDefault;
        public final String note;
        public final Placement placement;
        public final List<Row> rows;

        Section(String bucket, String title, boolean collapsedByDefault,
                String note, Placement placement, List<Row> rows) {
            this.bucket = bucket;
            this.title = title;
            this.collapsedByDefault = collapsedByDefault;
            this.note = note;
            this.placement = placement;
            this.rows = rows;
        }
    }

    private FeatureFlags() {
    }

    /**
     * Parses a {@code flagsMeta} string. A null or empty string has no rows;
     * malformed lines and unknown buckets are skipped.
     */
    public static List<Flag> parse(String meta) {
        List<Flag> flags = new ArrayList<>();
        if (meta == null || meta.isEmpty()) {
            return flags;
        }
        for (String line : meta.split("\n", -1)) {
            Flag flag = parseLine(line);
            if (flag != null) {
                flags.add(flag);
            }
        }
        return flags;
    }

    /** Whether {@code bucket} is one of the three declared buckets. */
    public static boolean isKnownBucket(String bucket) {
        return SETTINGS.equals(bucket) || ADVANCED.equals(bucket)
                || EXPERIMENTAL.equals(bucket);
    }

    /**
     * Parses a {@code flagsBucketsMeta} string: key, title, collapsed (1/0)
     * and note. Malformed lines and unknown buckets are skipped.
     */
    public static List<BucketInfo> parseBuckets(String meta) {
        List<BucketInfo> buckets = new ArrayList<>();
        if (meta == null || meta.isEmpty()) {
            return buckets;
        }
        for (String line : meta.split("\n", -1)) {
            BucketInfo bucket = parseBucketLine(line);
            if (bucket != null) {
                buckets.add(bucket);
            }
        }
        return buckets;
    }

    /**
     * The screen the Activity renders: sections in bucket order, a bucket with
     * no row omitted, and every row value read by id from {@code store} (the
     * core default when the key is missing). Unknown keys are never touched.
     */
    public static List<Section> screen(String flagsMeta, String bucketsMeta,
            Autostart.Store store) {
        List<Flag> flags = parse(flagsMeta);
        List<Section> sections = new ArrayList<>();
        for (BucketInfo bucket : parseBuckets(bucketsMeta)) {
            List<Row> rows = new ArrayList<>();
            for (Flag flag : flags) {
                if (!bucket.key.equals(flag.bucket)) {
                    continue;
                }
                boolean value;
                if (store == null) {
                    value = flag.defaultOn;
                } else if (FlagValues.NETWORK_DETAILS.equals(flag.id)) {
                    // The merged row: an old install keeps either old choice.
                    value = FlagValues.networkDetails(store);
                } else {
                    value = store.getBoolean(flag.id, flag.defaultOn);
                }
                rows.add(new Row(flag.id, flag.label, flag.help, value));
            }
            if (rows.isEmpty()) {
                continue;
            }
            Placement placement = SETTINGS.equals(bucket.key)
                    ? Placement.MAIN
                    : Placement.GROUP;
            sections.add(new Section(bucket.key, bucket.title,
                    bucket.collapsedByDefault, bucket.note, placement, rows));
        }
        return sections;
    }

    /** The rows that apply to {@code platformMask}, in table order. */
    public static List<Flag> forPlatform(List<Flag> flags, int platformMask) {
        List<Flag> out = new ArrayList<>();
        if (flags == null) {
            return out;
        }
        for (Flag flag : flags) {
            if ((flag.platforms & platformMask) != 0) {
                out.add(flag);
            }
        }
        return out;
    }

    private static BucketInfo parseBucketLine(String line) {
        if (line == null) {
            return null;
        }
        String[] fields = line.split("\t", -1);
        if (fields.length != 4) {
            return null;
        }
        String key = fields[0];
        String title = fields[1];
        if (key.isEmpty() || title.isEmpty() || !isKnownBucket(key)) {
            return null;
        }
        final boolean collapsed;
        if ("1".equals(fields[2])) {
            collapsed = true;
        } else if ("0".equals(fields[2])) {
            collapsed = false;
        } else {
            return null;
        }
        String note = fields[3].isEmpty() ? null : fields[3];
        return new BucketInfo(key, title, collapsed, note);
    }

    private static Flag parseLine(String line) {
        if (line == null) {
            return null;
        }
        String[] fields = line.split("\t", -1);
        if (fields.length != 6) {
            return null;
        }
        String id = fields[0];
        String bucket = fields[1];
        String label = fields[4];
        String help = fields[5];
        if (id.isEmpty() || label.isEmpty() || help.isEmpty()) {
            return null;
        }
        if (!isKnownBucket(bucket)) {
            return null;
        }
        final boolean defaultOn;
        if ("true".equals(fields[2])) {
            defaultOn = true;
        } else if ("false".equals(fields[2])) {
            defaultOn = false;
        } else {
            return null;
        }
        final int platforms;
        try {
            platforms = Integer.parseInt(fields[3]);
        } catch (NumberFormatException e) {
            return null;
        }
        if (platforms <= 0) {
            return null;
        }
        return new Flag(id, bucket, defaultOn, platforms, label, help);
    }
}
