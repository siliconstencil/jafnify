package com.siliconstencil.jafnify;

import java.text.SimpleDateFormat;
import java.util.Locale;
import java.util.TimeZone;

/**
 * Pure mapping and formatting for the sharer screen and notification. No
 * Android types, so it runs as a plain JVM unit test (UiTextTest). The native
 * status strings (app/wgserv/src/joiner.rs status_text) are mapped here and
 * nowhere else; the user-facing wording lives in strings.xml.
 */
final class UiText {
    private UiText() {}

    /** Where the sharer is, from the first line of the native status. */
    enum Status { IDLE, SEARCHING, PAIRING, CONNECTING, CONNECTED, RETRYING, UNKNOWN }

    /** What kind of failure an error text describes. */
    enum Problem { NONE, PORT_BUSY, NO_NETWORK, OTHER }

    /** The nine lines of Native.sharerStatus(). */
    static final class Parsed {
        final String line;
        final String peerId;
        final String peerName;
        final long rx;
        final long tx;
        final String endpoint;
        /** Bumped by native when the peer started a new session while this
         *  side stayed Connected; 0 when absent. */
        final long session;
        /** The phone's own connected time in ms, or 0 when not connected or
         *  from an older native library. */
        final long connectedMs;
        /** Connected but no authenticated peer activity: the display is a
         *  neutral "Reconnecting...". Line 0 stays Connected for the session
         *  tracker. */
        final boolean neutral;

        /** Five-line form: an older native library has no endpoint line. */
        Parsed(String line, String peerId, String peerName, long rx, long tx) {
            this(line, peerId, peerName, rx, tx, "", 0, 0, false);
        }

        Parsed(String line, String peerId, String peerName, long rx, long tx,
                String endpoint) {
            this(line, peerId, peerName, rx, tx, endpoint, 0, 0, false);
        }

        Parsed(String line, String peerId, String peerName, long rx, long tx,
                String endpoint, long session) {
            this(line, peerId, peerName, rx, tx, endpoint, session, 0, false);
        }

        Parsed(String line, String peerId, String peerName, long rx, long tx,
                String endpoint, long session, long connectedMs, boolean neutral) {
            this.line = line;
            this.peerId = peerId;
            this.peerName = peerName;
            this.rx = rx;
            this.tx = tx;
            this.endpoint = endpoint == null ? "" : endpoint;
            this.session = session;
            this.connectedMs = connectedMs;
            this.neutral = neutral;
        }
    }

    /**
     * "status\npeer id\npeer name\nrx\ntx\nendpoint\nsession\nconnected ms\n
     * neutral"; missing or null parts read as empty / 0 / false. An older
     * native library's five- or seven-line status still parses.
     */
    static Parsed parse(String raw) {
        String[] p = raw == null ? new String[0] : raw.split("\n", 9);
        return new Parsed(part(p, 0), part(p, 1), part(p, 2),
                parseLong(part(p, 3)), parseLong(part(p, 4)), part(p, 5),
                parseLong(part(p, 6)), parseLong(part(p, 7)),
                "1".equals(part(p, 8).trim()));
    }

    /** The neutral display word; the native and Linux sides use the same. */
    static final String RECONNECTING = "Reconnecting...";

    /**
     * The status the screen shows. The neutral flag is a display fact: the
     * phone is still connected for its session tracker, so line 0 stays the
     * core's `Connected`.
     */
    static Status displayStatus(Parsed p) {
        if (p != null && p.neutral) {
            return Status.CONNECTING;
        }
        return status(p == null ? null : p.line);
    }

    /** The status words the screen shows; neutral reads "Reconnecting...". */
    static String displayLine(Parsed p) {
        if (p == null) {
            return "";
        }
        return p.neutral ? RECONNECTING : p.line;
    }

    /**
     * The elapsedRealtime anchor for the connected clock. The phone's own
     * native value is the authority; without one (an older native library)
     * the current local anchor wins, and a fresh session starts at `now`.
     */
    static long connectedSince(long nowElapsedMs, long nativeConnectedMs,
            long currentSinceMs) {
        if (nativeConnectedMs > 0) {
            return Math.max(0, nowElapsedMs - nativeConnectedMs);
        }
        if (currentSinceMs != 0) {
            return currentSinceMs;
        }
        return nowElapsedMs;
    }

    /**
     * An empty status means the native side has not reported yet: the search
     * is starting, so it reads as SEARCHING.
     */
    static Status status(String line) {
        if (line == null || line.isEmpty()) {
            return Status.SEARCHING;
        }
        if (line.startsWith("Idle") || line.startsWith("Not sharing")) {
            return Status.IDLE;
        }
        if (line.startsWith("Searching")) {
            return Status.SEARCHING;
        }
        if (line.startsWith("Peer found") || line.startsWith("Pairing")) {
            return Status.PAIRING;
        }
        if (line.startsWith("Connecting")) {
            return Status.CONNECTING;
        }
        if (line.startsWith("Connected")) {
            return Status.CONNECTED;
        }
        if (line.startsWith("Retrying") || line.startsWith("Connection lost")) {
            return Status.RETRYING;
        }
        return Status.UNKNOWN;
    }

    /** The reason after the retry prefix; "" for any other status. */
    static String retryReason(String line) {
        if (line == null) {
            return "";
        }
        if (line.startsWith("Connection lost")) {
            // "Connection lost - retrying in N s" carries no reason; the
            // reason is the third segment when it is there.
            int first = line.indexOf(" - ");
            if (first < 0) {
                return "";
            }
            int second = line.indexOf(" - ", first + 3);
            return second < 0 ? "" : line.substring(second + 3);
        }
        if (!line.startsWith("Retrying")) {
            return "";
        }
        int i = line.indexOf(" - ");
        return i < 0 ? "" : line.substring(i + 3);
    }

    /** The sharing setup needs an internet uplink and a non-cellular LAN. */
    static boolean noNetwork(boolean uplinkUp) {
        return !uplinkUp;
    }

    /** The core connection detail lines, one per row, without empty ones. */
    static String[] connectionLines(String text) {
        if (text == null || text.isEmpty()) {
            return new String[0];
        }
        java.util.List<String> out = new java.util.ArrayList<>();
        for (String line : text.split("\n")) {
            String trimmed = line.trim();
            if (!trimmed.isEmpty()) {
                out.add(trimmed);
            }
        }
        return out.toArray(new String[0]);
    }

    /** Ask once, before the system prompt; never again after the user answered. */
    static boolean shouldAskNotifications(boolean supported, boolean granted, boolean prompted) {
        return supported && !granted && !prompted;
    }

    /** Show the quiet note once the permission was asked and is still missing. */
    static boolean notificationsOffNote(boolean supported, boolean granted, boolean prompted) {
        return supported && !granted && prompted;
    }

    /** Back closes the expanded Details before it leaves the activity. */
    static boolean backCollapsesDetails(boolean detailsVisible) {
        return detailsVisible;
    }

    /** Classifies a start error or a retry reason; matches the Rust error texts. */
    static Problem problem(String text) {
        if (text == null || text.isEmpty()) {
            return Problem.NONE;
        }
        String t = text.toLowerCase(Locale.US);
        if (t.contains("port in use")) {
            return Problem.PORT_BUSY;
        }
        if (t.contains("bind failed") || t.contains("socket error") || t.contains("unreachable")) {
            return Problem.NO_NETWORK;
        }
        return Problem.OTHER;
    }

    /**
     * The pairing code as one unspaced run. The visible spacing is the
     * TextView's letterSpacing, so the code never contains literal spaces.
     */
    static String codeDisplay(String code) {
        return code == null ? "" : code;
    }

    /** The code's letter tracking in em; the fit rule below uses the same value. */
    static final float CODE_TRACKING = 0.2f;

    /**
     * The largest text size <= 48sp that keeps the four digits on one line at
     * the given width and font scale, with a 20sp floor. A monospace glyph is
     * about 0.6 em wide and letterSpacing adds [CODE_TRACKING] em after each of
     * the four glyphs, so the line is {@code 4 * (0.6 + CODE_TRACKING) * size *
     * fontScale} dp wide.
     */
    static int codeTextSizeSp(float availableWidthDp, float fontScale) {
        float scale = fontScale > 0f ? fontScale : 1f;
        float perGlyph = 0.6f + CODE_TRACKING;
        int byWidth = (int) Math.floor(availableWidthDp / (4f * perGlyph * scale));
        if (byWidth > 48) {
            return 48;
        }
        return Math.max(20, byWidth);
    }

    /**
     * The side of a status glyph in px, from the text size it sits next to.
     * The text size already includes the font scale, so the icon grows with
     * the 200 % setting instead of staying at a fixed dp.
     */
    static int statusGlyphSizePx(float textSizePx) {
        return Math.max(16, Math.round(textSizePx * 1.15f));
    }

    /** True when the text actually changed, so an unchanged tick stays silent. */
    static boolean textChanged(String previous, String next) {
        return previous == null || !previous.equals(next);
    }

    /**
     * The coarse key a screen reader announces on: state, health verdict and
     * peer identity. The moving seconds (the handshake age is in Details; the
     * retry countdown) are deliberately not part of it, so the visible text
     * can tick while TalkBack stays quiet.
     */
    static String announceKey(Status status, String healthVerdict, String peer) {
        String state = status == null ? "unknown" : status.name();
        String who = peer == null ? "" : peer;
        return state + "|" + healthVerdict(healthVerdict) + "|" + who;
    }

    /**
     * The verdict of a health line. The native core now returns the verdict
     * alone; an older native library still prefixes the moving
     * "Last handshake N s ago - ", so that prefix is dropped if present.
     */
    static String healthVerdict(String healthLine) {
        if (healthLine == null) {
            return "";
        }
        int dash = healthLine.indexOf(" - ");
        return dash < 0 ? healthLine : healthLine.substring(dash + 3);
    }

    /**
     * The core's status wording with the peer's own name appended. The words
     * stay the core's; only the name, separated by a dash, is added here.
     */
    static String withPeer(String status, String peer) {
        String words = status == null ? "" : status;
        if (peer == null || peer.isEmpty()) {
            return words;
        }
        return words + " - " + peer;
    }

    /**
     * The link path line under the status ("Direct over hotspot"). It is a
     * claim about the tunnel, so while the sharer is stopped or waits for the
     * hotspot it is not current and is not shown; the card's own state line
     * already says what is happening.
     */
    static String linkLine(String line, boolean running, boolean waiting) {
        if (!running || waiting) {
            return "";
        }
        return line == null ? "" : line;
    }

    /**
     * The one line of the ongoing notification. A phone waiting for the
     * hotspot says why and nothing about the link; that wins over a native
     * "Connected" that is still there while the hotspot is gone.
     */
    static String notificationText(boolean waiting, String waitingText,
            boolean connected, String connectedText, String statusLine,
            String startingText) {
        if (waiting) {
            return waitingText == null ? "" : waitingText;
        }
        if (connected) {
            return connectedText == null ? "" : connectedText;
        }
        if (statusLine != null && !statusLine.isEmpty()) {
            return statusLine;
        }
        return startingText == null ? "" : startingText;
    }

    /** "m:ss", or "h:mm:ss" from one hour; negative reads as zero. */
    static String formatElapsed(long seconds) {
        long s = Math.max(0, seconds);
        long h = s / 3600;
        long m = (s % 3600) / 60;
        long sec = s % 60;
        if (h > 0) {
            return String.format(Locale.US, "%d:%02d:%02d", h, m, sec);
        }
        return String.format(Locale.US, "%d:%02d", m, sec);
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

    /**
     * The phone card's traffic line. rx is what came from the laptop, tx what
     * went to it; the words say the direction instead of the laptop's Up/Down.
     */
    static String phoneTraffic(long rx, long tx) {
        return "To laptop " + formatBytes(tx) + " | From laptop " + formatBytes(rx);
    }

    /** First 12 characters of a peer key (public); for the details view only. */
    static String keyPrefix(String peerId) {
        if (peerId == null) {
            return "";
        }
        return peerId.length() <= 12 ? peerId : peerId.substring(0, 12);
    }

    /** "Oct 23, 14:05" (or "Oct 23, 2:05 PM") in the given zone, always English. */
    static String formatWhen(long millis, TimeZone zone) {
        SimpleDateFormat f = new SimpleDateFormat("MMM d, " + Clock.minutes(), Locale.US);
        f.setTimeZone(zone);
        return f.format(new java.util.Date(millis));
    }

    private static String part(String[] parts, int i) {
        return i < parts.length ? parts[i] : "";
    }

    private static long parseLong(String s) {
        try {
            return Long.parseLong(s.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /**
     * The value for the Details "Last handshake" row: the core detail without
     * its own "Last handshake " lead ("30 s ago"); other text is kept whole.
     */
    static String handshakeAge(String detail) {
        String lead = "Last handshake ";
        if (detail != null && detail.startsWith(lead)) {
            return detail.substring(lead.length());
        }
        return detail == null ? "" : detail;
    }
}
