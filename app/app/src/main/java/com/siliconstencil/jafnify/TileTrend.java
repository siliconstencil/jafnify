package com.siliconstencil.jafnify;

/**
 * The geometry of the small drawings under the numbers of the tiles: the
 * trend line, the fill bar of the battery and the bars of the signal. Pure
 * Java, no Android classes, so the shapes can be checked without a device.
 */
public final class TileTrend {
    /** How many points a drawn line may have. */
    public static final int MAX_POINTS = 60;

    private TileTrend() {
    }

    /**
     * The points of the line as x/y pairs in pixels. An empty line has no
     * points; one sample is a flat line in the middle; many samples are
     * thinned to {@link #MAX_POINTS}.
     */
    public static float[] linePoints(int[] values, float width, float height) {
        if (values == null || values.length == 0) {
            return new float[0];
        }
        int[] sampled = downsample(values);
        int min = Integer.MAX_VALUE;
        int max = Integer.MIN_VALUE;
        for (int value : sampled) {
            min = Math.min(min, value);
            max = Math.max(max, value);
        }
        int span = max - min;
        float[] points = new float[sampled.length * 2];
        for (int i = 0; i < sampled.length; i++) {
            float x = sampled.length == 1 ? 0f
                    : (float) i / (sampled.length - 1) * width;
            float ratio = span == 0 ? 0.5f : (float) (sampled[i] - min) / span;
            points[i * 2] = x;
            points[i * 2 + 1] = height - ratio * height;
        }
        return points;
    }

    /** The mean of every bucket, at most {@link #MAX_POINTS} of them. */
    public static int[] downsample(int[] values) {
        if (values.length <= MAX_POINTS) {
            return values.clone();
        }
        int[] out = new int[MAX_POINTS];
        float bucket = (float) values.length / MAX_POINTS;
        for (int i = 0; i < MAX_POINTS; i++) {
            int start = (int) (i * bucket);
            int end = Math.min(values.length, Math.max(start + 1, (int) ((i + 1) * bucket)));
            long sum = 0;
            int count = 0;
            for (int j = start; j < end; j++) {
                sum += values[j];
                count++;
            }
            out[i] = count == 0 ? 0 : (int) (sum / count);
        }
        return out;
    }

    /** The height of the dashed reference line inside the range of the line. */
    public static float referenceY(int[] values, int reference, float height) {
        int min = Integer.MAX_VALUE;
        int max = Integer.MIN_VALUE;
        if (values != null) {
            for (int value : values) {
                min = Math.min(min, value);
                max = Math.max(max, value);
            }
        }
        min = Math.min(min, reference);
        max = Math.max(max, reference);
        int span = max - min;
        float ratio = span == 0 ? 0.5f : (float) (reference - min) / span;
        return height - ratio * height;
    }

    /** How much of the bar is full, in percent. */
    public static int barPercent(int value, int max) {
        if (max <= 0) {
            return 0;
        }
        return Math.max(0, Math.min(100, (int) ((long) value * 100 / max)));
    }

    /** The bars of the signal: one entry per level, true when it is filled. */
    public static boolean[] levelBars(int level, int max) {
        int count = Math.max(0, max);
        boolean[] bars = new boolean[count];
        for (int i = 0; i < count; i++) {
            bars[i] = i < level;
        }
        return bars;
    }

    /**
     * The two parts of a split bar: where the down part ends and where the
     * whole bar ends, in pixels. A bar with no data has no width.
     */
    public static float[] splitParts(long down, long up, float width) {
        long total = down + up;
        if (total <= 0 || width <= 0) {
            return new float[] {0f, 0f};
        }
        return new float[] {(float) ((double) width * down / total), width};
    }

    /**
     * The bars of an hour strip: the left edge and the width of every cell,
     * in pairs. The cells are placed by their place in the list, so the strip
     * of the drops runs over the last 24 hours and its first cell is the
     * oldest hour.
     */
    public static float[] cellBars(int count, float width, float gap) {
        if (count <= 0 || width <= 0f) {
            return new float[0];
        }
        float cell = (width - gap * (count - 1)) / count;
        if (cell <= 0f) {
            return new float[0];
        }
        float[] out = new float[count * 2];
        for (int i = 0; i < count; i++) {
            out[i * 2] = i * (cell + gap);
            out[i * 2 + 1] = cell;
        }
        return out;
    }
}
