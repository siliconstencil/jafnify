package com.siliconstencil.jafnify;

/**
 * Decides when the diagnostic journal samples STATS.
 *
 * <p>Normally every 5 s. While the last peer rx is older than 3 s, or has
 * never been seen, the journal samples every 1 s so a stall is visible at fine
 * resolution. The decision is pure: the caller supplies all timestamps.
 */
final class DiagSampler {
    /** Sample interval while peer rx is fresh. */
    static final long STATS_INTERVAL_MS = 5_000L;

    /** Sample interval while peer rx is flat or unknown. */
    static final long STATS_INTERVAL_FLAT_MS = 1_000L;

    /** Peer rx older than this (strictly) switches to the flat interval. */
    static final long PEER_RX_FLAT_MS = 3_000L;

    private DiagSampler() {
    }

    /**
     * True when a STATS record is due. {@code lastStatsMs} and
     * {@code lastPeerRxMs} are null until first seen; a null last sample is
     * always due, and a null peer rx counts as flat.
     */
    static boolean statsDue(long nowMs, Long lastStatsMs, Long lastPeerRxMs) {
        if (lastStatsMs == null) {
            return true;
        }
        boolean peerIsFlat = lastPeerRxMs == null
                || nowMs - lastPeerRxMs > PEER_RX_FLAT_MS;
        long interval = peerIsFlat ? STATS_INTERVAL_FLAT_MS : STATS_INTERVAL_MS;
        return nowMs - lastStatsMs >= interval;
    }
}
