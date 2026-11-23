package com.siliconstencil.jafnify;

import java.math.BigInteger;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Tracks one live session from the display state and the session's cumulative
 * rx/tx counters. It counts only Connected time, treats a counter reset as a
 * new baseline, and records drops and reconnects.
 *
 * <p>Pure Java: no Android types, so it runs as a plain JVM unit test. This is
 * the twin of linux/src/session.rs; keep the two behaviours identical.
 */
final class SessionTracker {
    /** The display state that counts as connected. */
    static final String CONNECTED = "Connected";
    /** The u32 counters saturate here, like the Linux {@code u32} fields. */
    private static final long MAX_COUNT = 0xFFFFFFFFL;
    private static final BigInteger THOUSAND = BigInteger.valueOf(1_000);
    private static final BigInteger LONG_MAX = BigInteger.valueOf(Long.MAX_VALUE);
    private static final AtomicLong SEQUENCE = new AtomicLong();

    private final String id;
    private final String role;
    private final long startedAtMs;
    private long lastActivityAtMs;
    private long connectedMs;
    private long upBytes;
    private long downBytes;
    private long peakUpBps;
    private long peakDownBps;
    private long drops;
    private long linkDowns;
    private long linkUps;
    private final ArrayList<SessionRecord.Reconnect> reconnects = new ArrayList<>();
    private String peerEndpoint = "";
    private String lastState;
    private Long lastSampleAtMs;
    private Long prevRx;
    private Long prevTx;
    private boolean finished;
    private long endedAtMs;
    private String endReason = "";
    private String startReason = "";

    /**
     * Start a session. The id is derived from the start time and a process
     * counter only; it carries no key material.
     */
    SessionTracker(String role, long startedAtMs) {
        long sequence = SEQUENCE.getAndIncrement();
        this.id = Long.toHexString(startedAtMs) + "-" + Long.toHexString(sequence);
        this.role = role;
        this.startedAtMs = startedAtMs;
    }

    /** The fixed start reason, copied into the live and finished record. */
    synchronized void setStartReason(String reason) {
        startReason = reason == null ? "" : reason;
    }

    /**
     * Feed one sample. rx/tx are cumulative for this session, state is the
     * display state, endpoint is the last observed peer endpoint, and reason
     * describes a drop if one is detected on this sample.
     */
    synchronized void sample(long nowMs, String state, long rx, long tx, String endpoint,
            String reason) {
        // A finished session no longer accepts activity.
        if (finished) {
            return;
        }
        if (endpoint != null && !endpoint.isEmpty()) {
            peerEndpoint = endpoint;
        }

        boolean wasConnected = CONNECTED.equals(lastState);
        boolean isConnected = CONNECTED.equals(state);
        Long previousSampleAtMs = lastSampleAtMs;
        long lastActivityBefore = lastActivityAtMs;

        // Only the interval spent in Connected is counted.
        if (wasConnected && previousSampleAtMs != null) {
            connectedMs = saturatingAdd(connectedMs, elapsedMs(previousSampleAtMs, nowMs));
        }

        // A reset is a new baseline: this sample contributes no delta.
        Long rxDelta = counterDelta(prevRx, rx);
        Long txDelta = counterDelta(prevTx, tx);
        prevRx = rx;
        prevTx = tx;
        boolean advanced = (rxDelta != null && rxDelta > 0)
                || (txDelta != null && txDelta > 0);

        if (wasConnected && !isConnected) {
            // A reconnect with no activity sample can still be pending. A
            // further drop means it never completed, so close it here rather
            // than leaving a zero connected_at/duration behind.
            Long pendingClosedAt = null;
            SessionRecord.Reconnect pending = lastReconnect();
            if (pending != null && pending.connectedAtMs == 0) {
                pending.connectedAtMs = nowMs;
                pending.durationMs = elapsedMs(pending.disconnectedAtMs, nowMs);
                pendingClosedAt = nowMs;
            }
            // Before the first activity sample there is no last-activity fact.
            // Never anchor a new drop before a just-closed pending reconnect.
            long disconnectedAt = lastActivityBefore > 0 ? lastActivityBefore : nowMs;
            if (pendingClosedAt != null) {
                disconnectedAt = Math.max(disconnectedAt, pendingClosedAt);
            }
            drops = saturatingCount(drops);
            SessionRecord.Reconnect reconnected = new SessionRecord.Reconnect();
            reconnected.disconnectedAtMs = disconnectedAt;
            reconnected.detectedAtMs = nowMs;
            reconnected.connectedAtMs = 0;
            reconnected.durationMs = 0;
            reconnected.reason = reason == null ? "" : reason;
            reconnects.add(reconnected);
        }

        if (rxDelta != null) {
            downBytes = saturatingAdd(downBytes, rxDelta);
        }
        if (txDelta != null) {
            upBytes = saturatingAdd(upBytes, txDelta);
        }
        if (advanced) {
            lastActivityAtMs = nowMs;
        }

        if (previousSampleAtMs != null) {
            long elapsed = elapsedMs(previousSampleAtMs, nowMs);
            if (elapsed > 0) {
                if (rxDelta != null) {
                    peakDownBps = Math.max(peakDownBps, perSecond(rxDelta, elapsed));
                }
                if (txDelta != null) {
                    peakUpBps = Math.max(peakUpBps, perSecond(txDelta, elapsed));
                }
            }
        }

        // The first activity after a drop supplies the reconnect event time.
        if (isConnected && advanced) {
            SessionRecord.Reconnect pending = lastReconnect();
            if (pending != null && pending.connectedAtMs == 0) {
                pending.connectedAtMs = nowMs;
                pending.durationMs = elapsedMs(pending.disconnectedAtMs, nowMs);
            }
        }

        lastState = state;
        lastSampleAtMs = nowMs;
    }

    /**
     * The peer restarted its session while data kept flowing: count one drop
     * that reconnected at once. A pending reconnect is closed first. Twin of
     * linux/src/session.rs peer_restart.
     */
    synchronized void peerRestart(long nowMs, String reason) {
        if (finished) {
            return;
        }
        SessionRecord.Reconnect pending = lastReconnect();
        if (pending != null && pending.connectedAtMs == 0) {
            pending.connectedAtMs = nowMs;
            pending.durationMs = elapsedMs(pending.disconnectedAtMs, nowMs);
        }
        long disconnectedAt = lastActivityAtMs > 0 ? Math.min(lastActivityAtMs, nowMs) : nowMs;
        drops = saturatingCount(drops);
        SessionRecord.Reconnect restarted = new SessionRecord.Reconnect();
        restarted.disconnectedAtMs = disconnectedAt;
        restarted.detectedAtMs = nowMs;
        restarted.connectedAtMs = nowMs;
        restarted.durationMs = elapsedMs(disconnectedAt, nowMs);
        restarted.reason = reason == null ? "" : reason;
        reconnects.add(restarted);
    }

    /** Record one uplink loss. Saturated, and a no-op after finish. */
    synchronized void linkDown() {
        if (!finished) {
            linkDowns = saturatingCount(linkDowns);
        }
    }

    /** Record one uplink restore. Saturated, and a no-op after finish. */
    synchronized void linkUp() {
        if (!finished) {
            linkUps = saturatingCount(linkUps);
        }
    }

    /**
     * The live session. Safe before the first sample. The end fields stay at
     * zero/empty; only {@link #finish} returns the completed record. The
     * reconnect list and every reconnect object are deep copies, so a later
     * sample cannot change a snapshot already handed to the UI.
     */
    synchronized SessionRecord current() {
        SessionRecord record = new SessionRecord();
        record.id = id;
        record.role = role;
        record.startedAtMs = startedAtMs;
        record.lastActivityAtMs = lastActivityAtMs;
        record.endedAtMs = 0;
        record.connectedMs = connectedMs;
        record.upBytes = upBytes;
        record.downBytes = downBytes;
        record.avgUpBps = average(upBytes, connectedMs);
        record.avgDownBps = average(downBytes, connectedMs);
        record.peakUpBps = peakUpBps;
        record.peakDownBps = peakDownBps;
        record.drops = drops;
        record.linkDowns = linkDowns;
        record.linkUps = linkUps;
        record.reconnects = copyReconnects(reconnects);
        record.peerEndpoint = peerEndpoint;
        record.endReason = "";
        record.startReason = startReason;
        return record;
    }

    /** Close the session at nowMs. A second finish returns the first result. */
    synchronized SessionRecord finish(long nowMs, String reason) {
        if (!finished) {
            if (CONNECTED.equals(lastState)) {
                if (lastSampleAtMs != null) {
                    connectedMs = saturatingAdd(connectedMs, elapsedMs(lastSampleAtMs, nowMs));
                }
                // A pending reconnect is complete only when the tracker saw a
                // Connected sample after the drop. Finishing while Retry or
                // Searching must leave it at connected_at = 0.
                SessionRecord.Reconnect pending = lastReconnect();
                if (pending != null && pending.connectedAtMs == 0) {
                    pending.connectedAtMs = nowMs;
                    pending.durationMs = elapsedMs(pending.disconnectedAtMs, nowMs);
                }
            }
            finished = true;
            endedAtMs = nowMs;
            endReason = reason == null ? "" : reason;
        }

        SessionRecord record = current();
        record.endedAtMs = endedAtMs;
        record.endReason = endReason;
        return record;
    }

    private SessionRecord.Reconnect lastReconnect() {
        return reconnects.isEmpty() ? null : reconnects.get(reconnects.size() - 1);
    }

    private static ArrayList<SessionRecord.Reconnect> copyReconnects(
            List<SessionRecord.Reconnect> source) {
        ArrayList<SessionRecord.Reconnect> copy = new ArrayList<>(source.size());
        for (SessionRecord.Reconnect reconnect : source) {
            copy.add(copyReconnect(reconnect));
        }
        return copy;
    }

    private static SessionRecord.Reconnect copyReconnect(SessionRecord.Reconnect source) {
        SessionRecord.Reconnect copy = new SessionRecord.Reconnect();
        copy.disconnectedAtMs = source.disconnectedAtMs;
        copy.detectedAtMs = source.detectedAtMs;
        copy.connectedAtMs = source.connectedAtMs;
        copy.durationMs = source.durationMs;
        copy.reason = source.reason;
        return copy;
    }

    private static Long counterDelta(Long previous, long current) {
        if (previous != null && current >= previous) {
            return current - previous;
        }
        // First sample, or a reset: no delta for this tick.
        return null;
    }

    private static long elapsedMs(long previous, long now) {
        if (now <= previous) {
            return 0;
        }
        long elapsed = now - previous;
        return elapsed < 0 ? Long.MAX_VALUE : elapsed;
    }

    private static long perSecond(long bytes, long elapsedMs) {
        if (elapsedMs <= 0 || bytes <= 0) {
            return 0;
        }
        if (bytes <= Long.MAX_VALUE / 1_000) {
            return bytes * 1_000 / elapsedMs;
        }
        BigInteger rate = BigInteger.valueOf(bytes).multiply(THOUSAND)
                .divide(BigInteger.valueOf(elapsedMs));
        return rate.compareTo(LONG_MAX) > 0 ? Long.MAX_VALUE : rate.longValue();
    }

    private static long average(long bytes, long connectedMs) {
        return perSecond(bytes, connectedMs);
    }

    private static long saturatingAdd(long value, long delta) {
        if (delta <= 0) {
            return value;
        }
        return value > Long.MAX_VALUE - delta ? Long.MAX_VALUE : value + delta;
    }

    private static long saturatingCount(long value) {
        return value >= MAX_COUNT ? MAX_COUNT : value + 1;
    }
}
