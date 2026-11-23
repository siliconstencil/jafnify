package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * One completed (or live) session, the Java twin of the Linux
 * {@code SessionRecord}. Times are UTC epoch milliseconds; byte counters are
 * cumulative for the session. There is no key, code or public-key field here.
 */
final class SessionRecord {
    String id = "";
    String role = "";
    long startedAtMs;
    long lastActivityAtMs;
    long endedAtMs;
    long connectedMs;
    /** Bytes sent by this side; the peer sees them as down. */
    long upBytes;
    /** Bytes received by this side; the peer sees them as up. */
    long downBytes;
    long avgUpBps;
    long avgDownBps;
    long peakUpBps;
    long peakDownBps;
    long drops;
    long linkDowns;
    long linkUps;
    List<Reconnect> reconnects = new ArrayList<>();
    String peerEndpoint = "";
    String endReason = "";
    /** user, boot or system restart; empty on records written before 0.2. */
    String startReason = "";

    /** One disconnected interval inside a session. */
    static final class Reconnect {
        long disconnectedAtMs;
        long detectedAtMs;
        long connectedAtMs;
        long durationMs;
        String reason = "";

        @Override
        public boolean equals(Object other) {
            if (this == other) {
                return true;
            }
            if (!(other instanceof Reconnect)) {
                return false;
            }
            Reconnect that = (Reconnect) other;
            return disconnectedAtMs == that.disconnectedAtMs
                    && detectedAtMs == that.detectedAtMs
                    && connectedAtMs == that.connectedAtMs
                    && durationMs == that.durationMs
                    && Objects.equals(reason, that.reason);
        }

        @Override
        public int hashCode() {
            return Objects.hash(disconnectedAtMs, detectedAtMs, connectedAtMs,
                    durationMs, reason);
        }
    }

    @Override
    public boolean equals(Object other) {
        if (this == other) {
            return true;
        }
        if (!(other instanceof SessionRecord)) {
            return false;
        }
        SessionRecord that = (SessionRecord) other;
        return startedAtMs == that.startedAtMs
                && lastActivityAtMs == that.lastActivityAtMs
                && endedAtMs == that.endedAtMs
                && connectedMs == that.connectedMs
                && upBytes == that.upBytes
                && downBytes == that.downBytes
                && avgUpBps == that.avgUpBps
                && avgDownBps == that.avgDownBps
                && peakUpBps == that.peakUpBps
                && peakDownBps == that.peakDownBps
                && drops == that.drops
                && linkDowns == that.linkDowns
                && linkUps == that.linkUps
                && Objects.equals(id, that.id)
                && Objects.equals(role, that.role)
                && Objects.equals(reconnects, that.reconnects)
                && Objects.equals(peerEndpoint, that.peerEndpoint)
                && Objects.equals(endReason, that.endReason)
                && Objects.equals(startReason, that.startReason);
    }

    @Override
    public int hashCode() {
        return Objects.hash(id, role, startedAtMs, lastActivityAtMs, endedAtMs,
                connectedMs, upBytes, downBytes, avgUpBps, avgDownBps, peakUpBps,
                peakDownBps, drops, linkDowns, linkUps, reconnects, peerEndpoint,
                endReason, startReason);
    }
}
