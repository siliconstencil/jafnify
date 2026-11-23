package com.siliconstencil.jafnify;

/**
 * The phone's own facts (charging, battery saver) for the tile home. The
 * system answers several binder calls for them and they change rarely, so the
 * screen asks for them once in a while instead of on every poll.
 */
final class PhoneFactsCache {

    /** How long a set of facts is used before it is asked for again. */
    static final long MAX_AGE_MS = 5_000L;

    /** The facts of the last ask, empty when there was none. */
    private String blob = "";

    /** When the facts were asked for, zero when there was no ask. */
    private long at;

    /** Whether the facts have to be asked for again at this moment. */
    boolean stale(long now) {
        return now - at >= MAX_AGE_MS;
    }

    /** The facts of the last ask. */
    String get() {
        return blob;
    }

    /** Keeps the facts of an ask. */
    void put(String blob, long now) {
        this.blob = blob == null ? "" : blob;
        this.at = now;
    }
}
