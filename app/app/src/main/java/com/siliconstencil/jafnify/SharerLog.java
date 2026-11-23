package com.siliconstencil.jafnify;

/**
 * Decides which lines the Activity's log receives. A line is an event: the
 * pairing code once per sharer run, and a native status line only when it
 * changes. Traffic counters change every poll and are shown in the card, never
 * in the log. Binding to a running sharer again (screen on/off) is not a new
 * run, so it adds nothing.
 */
final class SharerLog {
    private long run = Long.MIN_VALUE;
    private String status = null;
    private boolean codeLogged = false;

    /** True the first time this run id is seen; resets the per-run state. */
    boolean start(long runId) {
        if (runId == run) {
            return false;
        }
        run = runId;
        status = null;
        codeLogged = false;
        return true;
    }

    /** The code line for this run, once; null when absent or already logged. */
    String code(String code) {
        if (codeLogged || code == null || code.isEmpty()) {
            return null;
        }
        codeLogged = true;
        return code;
    }

    /** The native status line when it changed, otherwise null. */
    String status(UiText.Parsed parsed) {
        if (parsed == null || parsed.line == null || parsed.line.isEmpty()
                || parsed.line.equals(status)) {
            return null;
        }
        status = parsed.line;
        return parsed.line;
    }
}
