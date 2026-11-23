package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Decides when the sharer's usable IPv4 set changed and stayed changed long
 * enough to matter. Addresses are compared as a set: enumeration order and
 * duplicates are irrelevant. All calls are on the service's main thread.
 */
final class IfaceRefresh {
    static final long DEBOUNCE_MS = 2000;

    /** How a normalized interface set changed; only empty boundaries count. */
    enum LinkChange { NONE, LOSS, RESTORE, CHANGE }

    private String active = "";
    private String pending = null;
    private long pendingSince = 0;

    /** Set the current set as the baseline without asking for a restart. */
    void reset(String current) {
        active = normalize(current);
        pending = null;
        pendingSince = 0;
    }

    /** The normalized baseline; "" means no usable set was seen yet. */
    String active() {
        return active;
    }

    /**
     * True once the current set has differed from the active set for at
     * least {@link #DEBOUNCE_MS}. A revert or a new set cancels the pending
     * change and starts the debounce clock again.
     */
    boolean update(String current, long nowMs) {
        return update(current, nowMs, false);
    }

    /**
     * As {@link #update(String, long)}, but a connected sharer never asks for
     * a restart: the pending set is kept and the debounce clock keeps running,
     * so the change applies on the first call after the connection is gone.
     */
    boolean update(String current, long nowMs, boolean connected) {
        String next = normalize(current);
        if (next.equals(active)) {
            pending = null;
            pendingSince = 0;
            return false;
        }
        if (!next.equals(pending)) {
            pending = next;
            pendingSince = nowMs;
            return false;
        }
        if (connected) {
            return false;
        }
        if (nowMs - pendingSince < DEBOUNCE_MS) {
            return false;
        }
        active = next;
        pending = null;
        pendingSince = 0;
        return true;
    }

    /**
     * Compare two interface lists as normalized sets. A non-empty set that
     * swaps addresses is {@link LinkChange#CHANGE}: it is logged, but it is
     * neither a link loss nor a restore.
     */
    static LinkChange linkChange(String previous, String current) {
        String old = normalize(previous);
        String next = normalize(current);
        if (old.equals(next)) {
            return LinkChange.NONE;
        }
        if (old.isEmpty()) {
            return LinkChange.RESTORE;
        }
        if (next.isEmpty()) {
            return LinkChange.LOSS;
        }
        return LinkChange.CHANGE;
    }

    static String normalize(String list) {
        if (list == null || list.isEmpty()) {
            return "";
        }
        List<String> parts = new ArrayList<>();
        for (String part : list.split(",")) {
            String p = part.trim();
            if (!p.isEmpty() && !parts.contains(p)) {
                parts.add(p);
            }
        }
        Collections.sort(parts);
        StringBuilder sb = new StringBuilder();
        for (String p : parts) {
            if (sb.length() > 0) {
                sb.append(',');
            }
            sb.append(p);
        }
        return sb.toString();
    }
}
