package com.siliconstencil.jafnify;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.view.View;

/**
 * Draws the small drawing under the number of a tile: the trend line, the
 * fill bar of the battery or the bars of the signal. The shapes come from
 * {@link TileTrend}; the colours come from the generated token resources, so
 * the dark and the light theme both read.
 */
public class TileTrendView extends View {
    private int[] points = new int[0];
    private boolean hasReference;
    private int reference;
    private int fill = -1;
    private int fillMax = 100;
    private int level = -1;
    private int levelMax = 4;
    private boolean split;
    private long splitDown;
    private long splitUp;
    private String[] hours = new String[0];

    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path path = new Path();
    private final int lineColour;
    private final int mutedColour;
    private final int waitColour;
    private final int downColour;
    private final int upColour;
    private final int okColour;
    private final int errColour;
    private final int quietColour;
    private final int barWidth;
    private final int barGap;
    private final int barHeight;
    private final float outsideHeight;
    private final int radius;
    private final int stroke;

    public TileTrendView(Context context) {
        super(context);
        lineColour = Palette.colour(context, R.color.jafnify_fg);
        mutedColour = Palette.colour(context, R.color.jafnify_line);
        waitColour = Palette.colour(context, R.color.jafnify_wait);
        downColour = Palette.colour(context, R.color.jafnify_primary);
        upColour = Palette.colour(context, R.color.jafnify_line);
        okColour = Palette.colour(context, R.color.jafnify_ok);
        errColour = Palette.colour(context, R.color.jafnify_err);
        quietColour = Palette.colour(context, R.color.jafnify_muted);
        int spacingXs = dimen(context, R.dimen.jafnify_spacing_xs);
        barWidth = spacingXs;
        barGap = spacingXs / 2;
        barHeight = dimen(context, R.dimen.jafnify_line_height_caption);
        radius = dimen(context, R.dimen.jafnify_radius_sm);
        stroke = spacingXs / 2;
        outsideHeight = spacingXs / 2f;
    }

    /** The size of a token in pixels. */
    private static int dimen(Context context, int id) {
        return context.getResources().getDimensionPixelSize(id);
    }

    /** Sets the line to draw; an empty array draws nothing. */
    public void setLine(int[] values) {
        points = values == null ? new int[0] : values;
        invalidate();
    }

    /** Sets the dashed reference line, the heat threshold. */
    public void setReference(int value, boolean has) {
        reference = value;
        hasReference = has;
        invalidate();
    }

    /** Sets the fill bar of the battery. */
    public void setFill(int value, int max) {
        fill = value;
        fillMax = max;
        invalidate();
    }

    /** Sets the level bars of the signal. */
    public void setLevels(int level, int max) {
        this.level = level;
        this.levelMax = max;
        invalidate();
    }

    /** Sets the two parts of a split bar: down and up. */
    public void setSplit(long down, long up) {
        split = true;
        splitDown = down;
        splitUp = up;
        invalidate();
    }

    /** Sets the cells of an hour strip; the roles come from the core. */
    public void setHours(String[] roles) {
        hours = roles == null ? new String[0] : roles;
        invalidate();
    }

    /** Forgets every drawing; the tile shows its number alone. */
    public void clear() {
        points = new int[0];
        hasReference = false;
        fill = -1;
        level = -1;
        split = false;
        hours = new String[0];
        invalidate();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        int width = getWidth();
        int height = getHeight();
        if (width <= 0 || height <= 0) {
            return;
        }
        if (split) {
            drawSplit(canvas, width, height);
            return;
        }
        if (hours.length > 0) {
            drawHours(canvas, width, height);
            return;
        }
        if (fill >= 0) {
            drawFill(canvas, width, height);
            return;
        }
        boolean bars = level >= 0 && levelMax > 0;
        boolean line = points.length > 0;
        if (bars && line) {
            // The bars of the signal sit at the right end, the line takes the
            // rest of the width; the two never cross.
            int barsWidth = (int) Math.ceil(levelMax * (barWidth + barGap));
            int lineWidth = Math.max(0, width - barsWidth - (int) barGap * 2);
            drawLine(canvas, lineWidth, height);
            canvas.save();
            canvas.translate(width - barsWidth, 0f);
            drawLevels(canvas, barsWidth, 0, height);
            canvas.restore();
        } else if (bars) {
            drawLevels(canvas, width, 0, height);
        } else {
            drawLine(canvas, width, height);
        }
    }

    /**
     * The cells of an hour strip: one bar per hour, in the order of the
     * cells. An hour outside the window of the tile is muted.
     */
    private void drawHours(Canvas canvas, int width, int height) {
        float[] bars = TileTrend.cellBars(hours.length, width, barGap);
        paint.setStyle(Paint.Style.FILL);
        for (int i = 0; i < hours.length; i++) {
            if (HourMarks.ROLE_OUTSIDE.equals(hours[i])) {
                // Not counted by the tile: a low hairline in the line colour,
                // fainter than an hour without a verdict.
                paint.setColor(mutedColour);
                canvas.drawRect(bars[i * 2], height - outsideHeight,
                        bars[i * 2] + bars[i * 2 + 1], height, paint);
                continue;
            }
            paint.setColor(hourColour(hours[i]));
            canvas.drawRoundRect(bars[i * 2], 0f, bars[i * 2] + bars[i * 2 + 1], height,
                    radius, radius, paint);
        }
    }

    /** The colour of the role of an hour; the tokens carry the roles. */
    private int hourColour(String role) {
        if (HourMarks.ROLE_OK.equals(role)) {
            return okColour;
        }
        if (HourMarks.ROLE_WAIT.equals(role)) {
            return waitColour;
        }
        if (HourMarks.ROLE_ERR.equals(role)) {
            return errColour;
        }
        if (HourMarks.ROLE_MUTED.equals(role)) {
            return quietColour;
        }
        return mutedColour;
    }

    /** The split bar: the down part in the accent, the up part muted. */
    private void drawSplit(Canvas canvas, int width, int height) {
        float[] parts = TileTrend.splitParts(splitDown, splitUp, width);
        if (parts[1] <= 0f) {
            return;
        }
        float barHeight = Math.min(height, radius);
        float top = (height - barHeight) / 2f;
        paint.setStyle(Paint.Style.FILL);
        // One rounded bar in the up colour, then the down part painted over it
        // inside a clip: the two parts meet on a straight seam.
        paint.setColor(upColour);
        canvas.drawRoundRect(0f, top, parts[1], top + barHeight, radius, radius, paint);
        if (parts[0] <= 0f) {
            return;
        }
        paint.setColor(downColour);
        canvas.save();
        canvas.clipRect(0f, top, parts[0], top + barHeight);
        canvas.drawRoundRect(0f, top, parts[1], top + barHeight, radius, radius, paint);
        canvas.restore();
    }

    /** The bar of the battery: the whole bar and the full part of it. */
    private void drawFill(Canvas canvas, int width, int height) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(mutedColour);
        canvas.drawRoundRect(0f, 0f, width, height, radius, radius, paint);
        int percent = TileTrend.barPercent(fill, fillMax);
        if (percent <= 0) {
            return;
        }
        float full = Math.max(radius * 2f, width * percent / 100f);
        paint.setColor(lineColour);
        canvas.drawRoundRect(0f, 0f, full, height, radius, radius, paint);
    }

    /** The bars of the signal: the filled ones grow with their index. */
    private void drawLevels(Canvas canvas, int width, int top, int bottom) {
        boolean[] bars = TileTrend.levelBars(level, levelMax);
        int height = bottom - top;
        if (bars.length == 0 || height <= 0) {
            return;
        }
        paint.setStyle(Paint.Style.FILL);
        for (int i = 0; i < bars.length; i++) {
            float x = i * (barWidth + barGap);
            if (x + barWidth > width) {
                break;
            }
            float barHeight = height * (i + 1) / bars.length;
            paint.setColor(bars[i] ? lineColour : mutedColour);
            canvas.drawRoundRect(x, bottom - barHeight, x + barWidth, bottom,
                    radius, radius, paint);
        }
    }

    /** The trend line, with the dashed threshold it is compared against. */
    private void drawLine(Canvas canvas, int width, int height) {
        float[] line = TileTrend.linePoints(points, width, height);
        if (line.length >= 4) {
            path.reset();
            path.moveTo(line[0], line[1]);
            for (int i = 2; i < line.length; i += 2) {
                path.lineTo(line[i], line[i + 1]);
            }
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeWidth(stroke);
            paint.setColor(lineColour);
            canvas.drawPath(path, paint);
        }
        if (!hasReference) {
            return;
        }
        float y = TileTrend.referenceY(points, reference, height);
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(stroke);
        paint.setColor(waitColour);
        dash(canvas, 0f, y, width, y);
    }

    /** A dashed line; the plain paint cannot dash on API 21. */
    private void dash(Canvas canvas, float from, float y, float to, float y2) {
        float step = barWidth * 2f;
        float on = barWidth;
        for (float x = from; x < to; x += step) {
            canvas.drawLine(x, y, Math.min(x + on, to), y2, paint);
        }
    }
}
