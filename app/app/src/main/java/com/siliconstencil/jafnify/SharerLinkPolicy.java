package com.siliconstencil.jafnify;

/**
 * Pure link state for the sharer service. A session that had a usable IPv4
 * set goes to Waiting when that set vanishes; any non-empty set that comes
 * back causes exactly one discovery restart on it, even when the addresses
 * are the same (the old multicast membership died with the interface).
 *
 * <p>No Android types, so the state table runs in the JVM tests.
 */
final class SharerLinkPolicy {
    enum Action { NONE, WAIT, RESTART }

    enum Recovery { NONE, FALLBACK, STOP, WAIT }

    /**
     * A still-listed interface is ignored for this long after the system says
     * the hotspot is going away, so the dying interface cannot cause a
     * restart; the empty poll that follows keeps the session waiting.
     */
    static final long HOTSPOT_OFF_HOLD_MS = 1500;

    private final IfaceRefresh refresh = new IfaceRefresh();
    private boolean started;
    private boolean waiting;
    /** The session was started with no hotspot and has not seen one yet. */
    private boolean waitingFromStart;
    private boolean stopped;
    private boolean restoring;
    private boolean codeSession;
    private String restartSet = "";
    private String stopReason = "";
    private long holdUntilMs = 0;
    /**
     * The usable set seen when the system reported the hotspot off. While it
     * is still the current set the hotspot is not back, whatever the leftover
     * interface is; only an on report or a different set releases the wait.
     */
    private String hotspotOffSet = null;

    void start(String ifaces, boolean codeSession) {
        refresh.reset(ifaces);
        started = true;
        waiting = !usableSet(ifaces);
        waitingFromStart = waiting;
        stopped = false;
        restoring = false;
        this.codeSession = codeSession;
        restartSet = "";
        stopReason = "";
        holdUntilMs = 0;
        hotspotOffSet = null;
    }

    /**
     * The usable interface set is the gate for a start: with no hotspot LAN
     * there is nothing to discover on, so the session waits for one. An AP
     * state (when the system gives one) can only veto a non-empty set; an
     * AP-on hint alone never starts a share without a usable address.
     */
    static boolean hotspotReady(boolean usableSet, Boolean apOn) {
        return usableSet && !Boolean.FALSE.equals(apOn);
    }

    /**
     * True when the set has at least one address that can be a hotspot LAN.
     * The CLAT (192.0.0.x) and link-local leftovers are not one; the poll
     * already excludes them, and this keeps a raw set safe too.
     */
    static boolean usableSet(String ifaces) {
        String norm = IfaceRefresh.normalize(ifaces);
        if (norm.isEmpty()) {
            return false;
        }
        for (String address : norm.split(",")) {
            if (NetIfaces.hotspotAddress(address)) {
                return true;
            }
        }
        return false;
    }

    Action tick(String current, long nowMs, boolean connected) {
        restartSet = "";
        if (!started || stopped) {
            return Action.NONE;
        }
        String next = IfaceRefresh.normalize(current);
        boolean nextUsable = usableSet(next);
        if (waiting) {
            if (!nextUsable) {
                return Action.NONE;
            }
            if (nowMs < holdUntilMs) {
                // The system said the hotspot is going away; the interface
                // may still be listed for a moment. Never restart on it.
                return Action.NONE;
            }
            if (next.equals(hotspotOffSet)) {
                // The same set is still there although the system said the
                // hotspot is off: it is a leftover interface (the CLAT was
                // one), not the hotspot coming back. Keep waiting.
                return Action.NONE;
            }
            hotspotOffSet = null;
            // Any usable set is enough; reset also drops the old baseline, so
            // the same addresses still count as a restore.
            refresh.reset(next);
            waiting = false;
            waitingFromStart = false;
            restoring = true;
            restartSet = next;
            return Action.RESTART;
        }
        if (!nextUsable) {
            if (refresh.active().isEmpty()) {
                // No usable LAN yet; nothing to wait for or restart.
                return Action.NONE;
            }
            // Empty always waits, even while the native side still says
            // Connected: the peer was on the interface that just vanished.
            if (!refresh.update("", nowMs, false)) {
                return Action.NONE;
            }
            waiting = true;
            waitingFromStart = false;
            return Action.WAIT;
        }
        if (refresh.update(next, nowMs, connected)) {
            restartSet = next;
            return Action.RESTART;
        }
        return Action.NONE;
    }

    /**
     * The system reported the hotspot disabling or disabled: wait at once,
     * without waiting for the interface poll. Returns true only on the
     * transition, so the caller logs and updates the screen once.
     */
    boolean hotspotDisabled(long nowMs, String currentIfaces) {
        if (!started || stopped) {
            return false;
        }
        holdUntilMs = nowMs + HOTSPOT_OFF_HOLD_MS;
        String currentSet = IfaceRefresh.normalize(currentIfaces);
        hotspotOffSet = currentSet.isEmpty() ? null : currentSet;
        if (waiting) {
            return false;
        }
        waiting = true;
        waitingFromStart = false;
        restoring = false;
        return true;
    }

    /**
     * The system reported the hotspot enabled. The existing poll and restart
     * path does the restore; clearing the off set and hold lets that poll
     * accept the set again, even when the address is the same.
     */
    void hotspotEnabled() {
        holdUntilMs = 0;
        hotspotOffSet = null;
    }

    String restartSet() {
        return restartSet;
    }

    boolean isWaiting() {
        return waiting;
    }

    /** Waiting because Share was pressed while the hotspot was off. */
    boolean waitingFromStart() {
        return waiting && waitingFromStart;
    }

    boolean isCodeSession() {
        return codeSession;
    }

    /** True between a restore request and its health check. */
    boolean isRestoring() {
        return restoring;
    }

    /** Hold the partial wake lock only while there is a live link to serve. */
    boolean wantsWakeLock() {
        return started && !waiting && !stopped;
    }

    /** A successful restart; the set in {@link #restartSet()} is now live. */
    void restartSucceeded() {
        restoring = false;
    }

    /**
     * A restart on a non-empty set failed. A restore from Waiting must not
     * fall back to the old set: that set vanished, and calling the native
     * restart with it is the bug this change removes. Wait for a new set
     * instead, and let the service reject this one until it changes.
     * A normal change keeps today's fallback.
     */
    Recovery restartFailed(String error, String lastIfaces) {
        if (restoring) {
            restoring = false;
            waiting = true;
            return Recovery.WAIT;
        }
        if (IfaceRefresh.normalize(lastIfaces).isEmpty()) {
            stopped = true;
            waiting = false;
            stopReason = reason(error);
            return Recovery.STOP;
        }
        return Recovery.FALLBACK;
    }

    void fallbackSucceeded(String ifaces) {
        refresh.reset(ifaces);
        waiting = false;
        restoring = false;
    }

    Recovery fallbackFailed(String error) {
        stopped = true;
        waiting = false;
        restoring = false;
        stopReason = reason(error);
        return Recovery.STOP;
    }

    String stopReason() {
        return stopReason;
    }

    void userStop() {
        stopped = true;
        waiting = false;
        waitingFromStart = false;
        restoring = false;
        stopReason = "";
        holdUntilMs = 0;
        hotspotOffSet = null;
    }

    boolean isStopped() {
        return stopped;
    }

    private static String reason(String error) {
        return error == null || error.isEmpty()
                ? "could not restore discovery"
                : error;
    }
}
