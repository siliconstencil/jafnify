package com.siliconstencil.jafnify;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The event-driven network report. It counts only what the phone already
 * knows: the generation the modem reports, the signal level it reports, the
 * tunnel counters and the battery counter. Nothing is polled and no probe is
 * sent. The wording and the line format come from the core through the
 * bridge, so the phone and the laptop say the same thing.
 */
final class NetworkSampler {
    interface Bridge {
        /** The core wire word of a modem kind; "" when the kind is unknown. */
        String generationWord(String kind);

        /** True when the generation changed between two kinds. */
        boolean generationSwitch(String previous, String current);

        /** The canonical JFN1 line for the given fields; "" when it does not parse. */
        String bucketLine(String fields);

        /** One signal sample line; "" when the level is unknown or too soon. */
        String signalSample(long atMs, int level, long lastAtMs);

        /** The charge counter in mAh when it is sane; -1 when it is not. */
        long saneMah(long counter);

        /** A stored same-hour bucket as counters; empty when the line has no bucket. */
        long[] networkSeed(String line);
    }

    interface Store {
        /** Save the hourly network line. */
        void upsertNetwork(String line);

        /** Save one signal sample line. */
        void upsertSignal(String line);

        /** The stored network line of one hour, or null. */
        String loadNetworkHour(long hourStartMs);
    }

    private static final long HOUR_MS = 3_600_000L;
    private static final long CHECKPOINT_MS = 60_000L;
    private static final String[] GENERATION_WORDS = {
        "4g", "4g-ca", "4g+", "5g", "5g-nsa", "5g-mmwave", "5g-adv",
    };

    private final Bridge bridge;
    private final Store store;

    private boolean reportOn;
    private boolean signalOn;

    private boolean hasKind;
    private String kind = "";
    private long kindSinceMs;
    private final Map<String, Long> generationMs = new LinkedHashMap<>();
    private int switches;

    private boolean hasCounters;
    private long upBytes;
    private long downBytes;
    private long lastUpBytes;
    private long lastDownBytes;
    private int drops;
    private long connectedMs;
    private long samples;

    private boolean hasHour;
    private long hourStartMs;
    private boolean hasTick;
    private long lastTickMs;

    private boolean charging;
    private boolean hasMah;
    private long firstMah;
    private long currentMah = -1;
    private boolean hasPercent;
    private int firstPercent;
    private int currentPercent = -1;
    private long drainBaseMah;
    private boolean hasDrainBaseMah;
    private int drainBasePct;
    private boolean hasDrainBasePct;
    private long lastWriteMs;

    private long lastSignalAtMs;

    NetworkSampler(Bridge bridge, Store store) {
        this.bridge = bridge;
        this.store = store;
    }

    /** Turn the report and the signal history on or off. */
    /** The switches counted in the current hour, for the M0 summary. */
    public int switchesThisHour() {
        return switches;
    }

    void setFlags(boolean report, boolean signal) {
        reportOn = report;
        signalOn = signal;
        if (!reportOn) {
            resetHour();
        }
    }

    /** The modem reported a generation; a repeat is not a switch. */
    void onGeneration(String newKind, long nowMs) {
        if (!reportOn || newKind == null || newKind.isEmpty()) {
            return;
        }
        if (hasKind && bridge.generationSwitch(kind, newKind)) {
            switches++;
        }
        if (hasKind && !kind.equals(newKind)) {
            addGenerationTime(nowMs);
        }
        if (!hasKind) {
            kindSinceMs = nowMs;
        }
        hasKind = true;
        kind = newKind;
    }

    /** The modem reported a signal level; the core owns the five minute gate. */
    void onSignal(int level, long nowMs) {
        if (!signalOn) {
            return;
        }
        String line = bridge.signalSample(nowMs, level, lastSignalAtMs);
        if (line.isEmpty()) {
            return;
        }
        lastSignalAtMs = nowMs;
        store.upsertSignal(line);
    }

    /** One tick of the sharing service. */
    void onTick(long nowMs, long upBytesTotal, long downBytesTotal, boolean connected,
            int batteryPercent, long chargeCounterMah, boolean chargingNow) {
        if (!reportOn) {
            return;
        }
        if (!hasHour) {
            hourStartMs = nowMs - (nowMs % HOUR_MS);
            hasHour = true;
            seed(hourStartMs);
            lastWriteMs = nowMs;
        }
        long boundary = hourStartMs + HOUR_MS;
        if (nowMs >= boundary) {
            // The part before the boundary belongs to the hour that ends.
            long elapsedBefore = boundary - Math.max(hourStartMs, lastTickMs);
            if (hasTick && connected && elapsedBefore > 0) {
                connectedMs += elapsedBefore;
            }
            flush(boundary);
            hourStartMs = nowMs - (nowMs % HOUR_MS);
            hasHour = true;
            seed(hourStartMs);
            lastWriteMs = nowMs;
            long elapsedAfter = nowMs - boundary;
            if (connected && elapsedAfter > 0) {
                connectedMs += elapsedAfter;
            }
        } else if (hasTick && connected) {
            connectedMs += nowMs - lastTickMs;
        }
        if (hasCounters) {
            if (upBytesTotal >= lastUpBytes) {
                upBytes += upBytesTotal - lastUpBytes;
            }
            if (downBytesTotal >= lastDownBytes) {
                downBytes += downBytesTotal - lastDownBytes;
            }
        }
        hasCounters = true;
        lastUpBytes = upBytesTotal;
        lastDownBytes = downBytesTotal;
        hasTick = true;
        lastTickMs = nowMs;
        samples++;

        long mah = bridge.saneMah(chargeCounterMah);
        currentMah = mah;
        if (!hasMah && mah >= 0) {
            hasMah = true;
            firstMah = mah;
        }
        currentPercent = batteryPercent;
        if (!hasPercent && batteryPercent >= 0) {
            hasPercent = true;
            firstPercent = batteryPercent;
        }
        if (chargingNow != charging) {
            charging = chargingNow;
            hasMah = mah >= 0;
            firstMah = mah;
            hasPercent = batteryPercent >= 0;
            firstPercent = batteryPercent;
            hasDrainBaseMah = false;
            hasDrainBasePct = false;
        }
        if (nowMs - lastWriteMs >= CHECKPOINT_MS) {
            write(nowMs);
            lastWriteMs = nowMs;
        }
    }

    /** The session stopped: the partial hour is not lost. */
    void onStop(long nowMs) {
        if (!reportOn) {
            return;
        }
        flush(nowMs);
    }

    /** The tunnel drops are the ones the session already counted. */
    void setDrops(int drops) {
        this.drops = drops;
    }

    private void addGenerationTime(long nowMs) {
        String word = bridge.generationWord(kind);
        if (word.isEmpty()) {
            kindSinceMs = nowMs;
            return;
        }
        long ms = nowMs - kindSinceMs;
        if (ms > 0) {
            Long known = generationMs.get(word);
            generationMs.put(word, (known == null ? 0L : known) + ms);
        }
        kindSinceMs = nowMs;
    }

    private void flush(long nowMs) {
        if (hasKind) {
            addGenerationTime(nowMs);
        }
        write(nowMs);
        resetHour();
    }

    private void write(long nowMs) {
        if (samples == 0 && switches == 0) {
            return;
        }
        String line = bridge.bucketLine(fieldsAt(nowMs));
        if (!line.isEmpty()) {
            store.upsertNetwork(line);
        }
    }

    /** Continue an hour that was stored before a restart. */
    private void seed(long hourStartMs) {
        String stored = store.loadNetworkHour(hourStartMs);
        if (stored == null || stored.isEmpty()) {
            return;
        }
        long[] values = bridge.networkSeed(stored);
        if (values == null || values.length < 11) {
            return;
        }
        switches = (int) values[1];
        upBytes = values[2];
        downBytes = values[3];
        drops = (int) values[4];
        connectedMs = values[5];
        samples = values[6];
        if (values[7] >= 0) {
            drainBaseMah = values[7];
            hasDrainBaseMah = true;
        }
        if (values[8] >= 0) {
            drainBasePct = (int) values[8];
            hasDrainBasePct = true;
        }
        charging = values[9] != 0;
        int count = (int) values[10];
        for (int index = 0; index < count; index++) {
            int ordinal = (int) values[11 + index * 2];
            long ms = values[12 + index * 2];
            if (ordinal >= 0 && ordinal < GENERATION_WORDS.length && ms > 0) {
                String word = GENERATION_WORDS[ordinal];
                Long known = generationMs.get(word);
                generationMs.put(word, (known == null ? 0L : known) + ms);
            }
        }
    }

    private String fieldsAt(long nowMs) {
        StringBuilder text = new StringBuilder();
        text.append("hour=").append(hourStartMs);
        text.append(" switches=").append(switches);
        text.append(" gen=").append(generationTextAt(nowMs));
        text.append(" up=").append(upBytes);
        text.append(" down=").append(downBytes);
        text.append(" drops=").append(drops);
        Long mah = drainMah();
        if (mah != null) {
            text.append(" batt_mah=").append(mah);
        }
        Integer percent = drainPercent();
        if (percent != null) {
            text.append(" batt_pct=").append(percent);
        }
        text.append(" charging=").append(charging ? 1 : 0);
        text.append(" connected_ms=").append(connectedMs);
        text.append(" samples=").append(samples);
        return text.toString();
    }

    private String generationTextAt(long nowMs) {
        Map<String, Long> times = new LinkedHashMap<>(generationMs);
        if (hasKind) {
            String word = bridge.generationWord(kind);
            if (!word.isEmpty() && nowMs > kindSinceMs) {
                Long known = times.get(word);
                times.put(word, (known == null ? 0L : known) + (nowMs - kindSinceMs));
            }
        }
        if (times.isEmpty()) {
            return "none";
        }
        StringBuilder text = new StringBuilder();
        for (Map.Entry<String, Long> entry : times.entrySet()) {
            if (text.length() > 0) {
                text.append(',');
            }
            text.append(entry.getKey()).append(':').append(entry.getValue());
        }
        return text.toString();
    }

    private Long drainMah() {
        Long live = null;
        if (hasMah && currentMah >= 0 && currentMah <= firstMah) {
            live = firstMah - currentMah;
        }
        if (!hasDrainBaseMah && live == null) {
            return null;
        }
        return (hasDrainBaseMah ? drainBaseMah : 0) + (live == null ? 0 : live);
    }

    private Integer drainPercent() {
        if (hasMah || hasDrainBaseMah) {
            return null;
        }
        Integer live = null;
        if (hasPercent && currentPercent >= 0 && currentPercent <= firstPercent) {
            live = firstPercent - currentPercent;
        }
        if (!hasDrainBasePct && live == null) {
            return null;
        }
        return (hasDrainBasePct ? drainBasePct : 0) + (live == null ? 0 : live);
    }

    private void resetHour() {
        switches = 0;
        generationMs.clear();
        upBytes = 0;
        downBytes = 0;
        drops = 0;
        connectedMs = 0;
        samples = 0;
        hasHour = false;
        hasMah = false;
        hasPercent = false;
        hasDrainBaseMah = false;
        hasDrainBasePct = false;
        currentMah = -1;
        currentPercent = -1;
    }
}
