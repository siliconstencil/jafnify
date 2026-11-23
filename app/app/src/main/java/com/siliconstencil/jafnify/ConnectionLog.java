package com.siliconstencil.jafnify;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Date;
import java.util.List;
import java.util.Locale;
import java.util.TimeZone;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Bounded connection log shown, copied and shared by the sharer screen.
 *
 * <p>Each entry is one line: {@code <local timestamp> <text>}. Key-shaped text
 * is redacted before it is stored, so the screen and the copied text carry no
 * key material. The copied and shared text is redacted again at the export
 * boundary, with one pass over the whole log. The ring keeps the newest
 * {@link #MAX_LINES} entries, oldest first.
 *
 * <p>Pure Java: no Android types, so it runs as a plain JVM unit test. The
 * timestamp and event templates match the Linux {@code connection_log}.
 *
 * <p>An attached file keeps the log across service restarts: the newest lines
 * are loaded on {@link #attach(File)} and every new line is appended. The file
 * is compacted to the newest {@link #MAX_LINES} lines once it holds twice as
 * many. File problems never fail a push.
 */
final class ConnectionLog {
    /** The in-memory log is bounded to this many lines. */
    static final int MAX_LINES = 500;

    /** A complete WireGuard key, base64 with its trailing '='. */
    private static final Pattern KEY = Pattern.compile("[A-Za-z0-9+/]{42,44}=");

    /** One entry must stay one line even when the source text has breaks. */
    private static final Pattern LINE_BREAK = Pattern.compile("[\\r\\n]+");

    /** Redacts one whole export text. The native implementation is the core
     *  redactor; tests supply their own so the log runs without JNI. */
    interface ExportRedactor {
        String redact(String text, String aliases);
    }

    private static final ExportRedactor NATIVE_REDACTOR = new ExportRedactor() {
        @Override
        public String redact(String text, String aliases) {
            return Native.redactForExport(text, aliases);
        }
    };

    private static final class Entry {
        final String kind;
        final String line;

        Entry(String kind, String line) {
            this.kind = kind;
            this.line = line;
        }
    }

    /** The kind of the lines loaded from an earlier run. */
    static final String EARLIER_KIND = "earlier";

    private final ArrayDeque<Entry> entries = new ArrayDeque<>();
    private File file;
    private int fileLines;

    /**
     * Back the log with {@code target}: its newest lines replace the current
     * entries, and later pushes are appended to it. A missing or unreadable
     * file starts empty.
     */
    synchronized void attach(File target) {
        file = target;
        entries.clear();
        List<String> lines = new ArrayList<>();
        if (target != null && target.isFile()) {
            try (BufferedReader in = new BufferedReader(new InputStreamReader(
                    new FileInputStream(target), StandardCharsets.UTF_8))) {
                String line;
                while ((line = in.readLine()) != null) {
                    if (!line.trim().isEmpty()) {
                        lines.add(line);
                    }
                }
            } catch (IOException | RuntimeException ignored) {
                lines.clear();
            }
        }
        fileLines = lines.size();
        int skip = Math.max(0, lines.size() - MAX_LINES);
        for (int i = skip; i < lines.size(); i++) {
            entries.addLast(new Entry(EARLIER_KIND, lines.get(i)));
        }
    }

    private void appendToFile(String line) {
        if (file == null) {
            return;
        }
        try {
            if (fileLines >= 2 * MAX_LINES) {
                List<String> kept = new ArrayList<>();
                for (Entry entry : entries) {
                    kept.add(entry.line);
                }
                kept.add(line);
                int skip = Math.max(0, kept.size() - MAX_LINES);
                StringBuilder body = new StringBuilder();
                for (int i = skip; i < kept.size(); i++) {
                    body.append(kept.get(i)).append('\n');
                }
                File tmp = new File(file.getParentFile(), "." + file.getName() + ".tmp");
                try (OutputStream out = new FileOutputStream(tmp, false)) {
                    out.write(body.toString().getBytes(StandardCharsets.UTF_8));
                }
                if (tmp.renameTo(file)) {
                    fileLines = kept.size() - skip;
                }
                return;
            }
            try (OutputStream out = new FileOutputStream(file, true)) {
                out.write((line + "\n").getBytes(StandardCharsets.UTF_8));
            }
            fileLines++;
        } catch (IOException | RuntimeException ignored) {
            // Best effort: the on-screen log keeps working.
        }
    }

    /** Append one event. {@code kind} is metadata; {@code text} is the line after the timestamp. */
    synchronized void push(String kind, String text, long nowMs) {
        if (entries.size() == MAX_LINES) {
            entries.removeFirst();
        }
        String body = text == null ? "" : text;
        body = LINE_BREAK.matcher(body).replaceAll(" ");
        String line = timestamp(nowMs) + " " + redact(body);
        appendToFile(line);
        entries.addLast(new Entry(kind == null ? "" : kind, line));
    }

    /** The last {@code n} lines, oldest first. */
    synchronized List<String> tail(int n) {
        int wanted = Math.max(0, n);
        int skip = Math.max(0, entries.size() - wanted);
        List<String> lines = new ArrayList<>(Math.min(wanted, entries.size()));
        int index = 0;
        for (Entry entry : entries) {
            if (index++ >= skip) {
                lines.add(entry.line);
            }
        }
        return lines;
    }

    /** The same lines as {@link #tail(int)}, joined with a newline and no trailing newline. */
    synchronized String text() {
        StringBuilder out = new StringBuilder();
        for (Entry entry : entries) {
            if (out.length() > 0) {
                out.append('\n');
            }
            out.append(entry.line);
        }
        return out.toString();
    }

    /**
     * A whole log text as it may leave the device: one redaction pass over
     * every line, so one peer keeps one label across the export. The
     * on-screen log keeps its text.
     */
    static String exportText(String text) {
        return exportText(text, "", NATIVE_REDACTOR);
    }

    /** The same pass with the peer aliases the export knows. */
    static String exportText(String text, String aliases) {
        return exportText(text, aliases, NATIVE_REDACTOR);
    }

    static String exportText(String text, ExportRedactor redactor) {
        return exportText(text, "", redactor);
    }

    static String exportText(String text, String aliases, ExportRedactor redactor) {
        if (redactor == null) {
            throw new IllegalArgumentException("redactor");
        }
        return redactor.redact(text == null ? "" : text, aliases == null ? "" : aliases);
    }

    synchronized int size() {
        return entries.size();
    }

    synchronized boolean isEmpty() {
        return entries.isEmpty();
    }

    /** Drop every entry: a new run starts with an empty on-screen log. */
    synchronized void clear() {
        entries.clear();
    }

    synchronized String lastKind() {
        Entry last = entries.peekLast();
        return last == null ? null : last.kind;
    }

    /** {@code yyyy-MM-dd HH:mm:ss z} (or 12-hour) in the device time zone. */
    static String timestamp(long epochMs) {
        return timestamp(epochMs, TimeZone.getDefault());
    }

    /** The same format in an explicit zone; tests pin UTC this way. */
    static String timestamp(long epochMs, TimeZone zone) {
        SimpleDateFormat format = new SimpleDateFormat("yyyy-MM-dd " + Clock.seconds() + " z", Locale.US);
        format.setTimeZone(zone);
        return format.format(new Date(epochMs));
    }

    /** Keys are removed whole, with no prefix left behind. */
    static String redact(String text) {
        if (text == null) {
            return "";
        }
        Matcher matcher = KEY.matcher(text);
        StringBuffer out = new StringBuffer(text.length());
        while (matcher.find()) {
            matcher.appendReplacement(out, Matcher.quoteReplacement("[redacted]"));
        }
        matcher.appendTail(out);
        return out.toString();
    }

    /** {@code state <From> -> <To>}. */
    static String stateLine(String from, String to) {
        return "state " + from + " -> " + to;
    }

    /** {@code disconnected at <event time>: <reason> (detected after <N> s)}. */
    static String dropLine(String reason, long disconnectedAtMs, long detectedAtMs) {
        return dropLine(reason, disconnectedAtMs, detectedAtMs, TimeZone.getDefault());
    }

    static String dropLine(String reason, long disconnectedAtMs, long detectedAtMs,
            TimeZone zone) {
        long delay = Math.max(0, detectedAtMs - disconnectedAtMs) / 1_000L;
        return "disconnected at " + timestamp(disconnectedAtMs, zone) + ": " + reason
                + " (detected after " + delay + " s)";
    }

    /** {@code reconnected at <event time> after <h:mm:ss> (peer <endpoint>)}. */
    static String reconnectLine(long connectedAtMs, long durationMs, String endpoint) {
        return reconnectLine(connectedAtMs, durationMs, endpoint, TimeZone.getDefault());
    }

    static String reconnectLine(long connectedAtMs, long durationMs, String endpoint,
            TimeZone zone) {
        return "reconnected at " + timestamp(connectedAtMs, zone) + " after "
                + durationHms(durationMs) + " (peer " + endpoint + ")";
    }

    /** Always {@code h:mm:ss}; negative reads as zero. */
    static String durationHms(long durationMs) {
        long seconds = Math.max(0, durationMs) / 1_000L;
        long hours = seconds / 3_600L;
        long minutes = (seconds % 3_600L) / 60L;
        long secs = seconds % 60L;
        return String.format(Locale.US, "%d:%02d:%02d", hours, minutes, secs);
    }

    /** {@code link changed: <old> -> <new>}. */
    static String linkChangedLine(String oldIfaces, String newIfaces) {
        return "link changed: " + oldIfaces + " -> " + newIfaces;
    }

    /** {@code session started (<role>)}. */
    static String sessionStartedLine(String role) {
        return "session started (" + role + ")";
    }

    /** {@code started by: <user|boot|system restart>}. */
    static String startedByLine(String reason) {
        return "started by: " + reason;
    }

    /** {@code session ended (<reason>)}. */
    static String sessionEndedLine(String reason) {
        return "session ended (" + reason + ")";
    }
}
