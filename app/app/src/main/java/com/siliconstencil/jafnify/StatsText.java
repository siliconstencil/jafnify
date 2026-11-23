package com.siliconstencil.jafnify;

import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Date;
import java.util.List;
import java.util.Locale;
import java.util.TimeZone;

/**
 * Pure formatting for session details and history. The wording and templates
 * are the same as the Linux ui_text/view code; tests pin the exact strings so
 * the two platforms cannot drift. No Android types.
 */
final class StatsText {
    private StatsText() {}

    /** Details and history: {@code Link losses: N   Link restores: N}. */
    static String linkCountsLine(long downs, long ups) {
        return "Link losses: " + downs + "   Link restores: " + ups;
    }

    /** "m:ss", or "h:mm:ss" from one hour; negative reads as zero. */
    static String formatElapsed(long seconds) {
        long total = Math.max(0, seconds);
        long hours = total / 3600;
        long minutes = (total % 3600) / 60;
        long secs = total % 60;
        if (hours > 0) {
            return String.format(Locale.US, "%d:%02d:%02d", hours, minutes, secs);
        }
        return String.format(Locale.US, "%d:%02d", minutes, secs);
    }

    static String formatBytes(long bytes) {
        if (bytes < 1024) {
            return bytes + " B";
        }
        if (bytes < 1024 * 1024) {
            return String.format(Locale.US, "%.0f KB", bytes / 1024.0);
        }
        return String.format(Locale.US, "%.1f MB", bytes / (1024.0 * 1024.0));
    }

    /** A byte rate, the same as the Linux {@code rate()} helper. */
    static String formatRate(long bytesPerSecond) {
        return formatBytes(bytesPerSecond) + "/s";
    }

    /** A local timestamp with its zone abbreviation, or {@code none}. */
    static String formatTimestampOrNone(long epochMs) {
        return formatTimestampOrNone(epochMs, TimeZone.getDefault());
    }

    /** The same format in an explicit zone; tests pin UTC this way. */
    static String formatTimestampOrNone(long epochMs, TimeZone zone) {
        if (epochMs <= 0) {
            return "none";
        }
        SimpleDateFormat format = new SimpleDateFormat("yyyy-MM-dd " + Clock.seconds() + " z", Locale.US);
        format.setTimeZone(zone);
        return format.format(new Date(epochMs));
    }

    /**
     * The current-session details, one label per line, in the Linux order:
     * Started, Connected, Up, Down, Average, Peak, Drops/Reconnects, link
     * counts, peer endpoint.
     */
    /**
     * The live session as the home tile line the core reads. The tile speaks
     * of the laptop's internet like every other "up" and "down" here: tunnel
     * tx is the laptop's download, tunnel rx its upload.
     */
    static String tileSessionLine(SessionRecord record) {
        return "up=" + record.downBytes + " down=" + record.upBytes
                + " peak=" + record.peakUpBps + " start=" + record.startedAtMs;
    }

    static List<String> details(SessionRecord record) {
        return details(record, TimeZone.getDefault());
    }

    static List<String> details(SessionRecord record, TimeZone zone) {
        List<String> lines = new ArrayList<>();
        lines.add("Started: " + formatTimestampOrNone(record.startedAtMs, zone));
        lines.add("Connected: " + formatElapsed(record.connectedMs / 1_000));
        // "Up" and "down" always mean the laptop's internet direction. On the
        // phone, tunnel rx is the laptop's upload and tunnel tx is its
        // download, so the stored counters are swapped on display.
        lines.add("Up (laptop upload): " + formatBytes(record.downBytes));
        lines.add("Down (laptop download): " + formatBytes(record.upBytes));
        lines.add("Average up: " + formatRate(record.avgDownBps)
                + "   Average down: " + formatRate(record.avgUpBps));
        lines.add("Peak up: " + formatRate(record.peakDownBps)
                + "   Peak down: " + formatRate(record.peakUpBps));
        lines.add("Drops: " + record.drops + "   Reconnects: " + reconnectCount(record));
        lines.add(linkCountsLine(record.linkDowns, record.linkUps));
        lines.add("Peer endpoint: " + endpoint(record));
        return lines;
    }

    /**
     * The current session as label/value rows for the inline Details table on
     * the phone. Byte directions are named from the phone's side, the same as
     * the status card: what went to the laptop and what came from it.
     */
    static List<String[]> sessionRows(SessionRecord record) {
        return sessionRows(record, TimeZone.getDefault());
    }

    static List<String[]> sessionRows(SessionRecord record, TimeZone zone) {
        List<String[]> rows = new ArrayList<>();
        rows.add(row("Started", formatTimestampOrNone(record.startedAtMs, zone)));
        rows.add(row("Connected for", formatElapsed(record.connectedMs / 1_000)));
        rows.add(row("Up (laptop upload)", formatBytes(record.downBytes)));
        rows.add(row("Down (laptop download)", formatBytes(record.upBytes)));
        rows.add(row("Average", formatRate(record.avgDownBps) + " up, "
                + formatRate(record.avgUpBps) + " down"));
        rows.add(row("Peak", formatRate(record.peakDownBps) + " up, "
                + formatRate(record.peakUpBps) + " down"));
        rows.add(row("Drops", String.valueOf(record.drops)));
        rows.add(row("Reconnects", String.valueOf(reconnectCount(record))));
        rows.add(row("Link losses", record.linkDowns + " lost, " + record.linkUps + " restored"));
        rows.add(row("Laptop address", endpoint(record)));
        return rows;
    }

    static String[] row(String label, String value) {
        return new String[] {label, value};
    }

    /**
     * One compact history row: start, connected duration, drops, reconnects,
     * up/down bytes and the peer endpoint.
     */
    static String historyLine(SessionRecord record) {
        return historyLine(record, TimeZone.getDefault());
    }

    static String historyLine(SessionRecord record, TimeZone zone) {
        return formatTimestampOrNone(record.startedAtMs, zone)
                + "   " + formatElapsed(record.connectedMs / 1_000)
                + "   Drops: " + record.drops
                + "   Reconnects: " + reconnectCount(record)
                + "   Up " + formatBytes(record.downBytes)
                + "   Down " + formatBytes(record.upBytes)
                + "   " + endpoint(record);
    }

    private static int reconnectCount(SessionRecord record) {
        return record.reconnects == null ? 0 : record.reconnects.size();
    }

    private static String endpoint(SessionRecord record) {
        return record.peerEndpoint == null || record.peerEndpoint.isEmpty()
                ? "unknown"
                : record.peerEndpoint;
    }
}
