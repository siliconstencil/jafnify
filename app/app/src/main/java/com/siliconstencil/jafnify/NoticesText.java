package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;

/**
 * Splits the generated third-party notices into chunks small enough for one
 * TextView. Pure, so the JVM test holds the split; the screen reads the asset
 * off the main thread and shows the chunks in a list.
 */
final class NoticesText {
    private NoticesText() {
    }

    /** Largest chunk, in chars: one TextView stays cheap to lay out. */
    static final int MAX_CHUNK = 16 * 1024;

    /**
     * Line-aligned chunks, never longer than {@link #MAX_CHUNK} unless a single
     * line is longer (then that line is split). The chunks keep every character
     * and their order, so the screen shows the file verbatim.
     */
    static List<String> sections(String raw) {
        List<String> out = new ArrayList<>();
        if (raw == null || raw.isEmpty()) {
            return out;
        }
        StringBuilder chunk = new StringBuilder();
        int start = 0;
        while (start < raw.length()) {
            int newline = raw.indexOf('\n', start);
            int end = newline < 0 ? raw.length() : newline + 1;
            String line = raw.substring(start, end);
            if (chunk.length() > 0 && chunk.length() + line.length() > MAX_CHUNK) {
                out.add(chunk.toString());
                chunk.setLength(0);
            }
            while (line.length() > MAX_CHUNK) {
                out.add(line.substring(0, MAX_CHUNK));
                line = line.substring(MAX_CHUNK);
            }
            chunk.append(line);
            start = end;
        }
        if (chunk.length() > 0) {
            out.add(chunk.toString());
        }
        return out;
    }
}
