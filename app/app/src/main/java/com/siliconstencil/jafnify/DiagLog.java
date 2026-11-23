package com.siliconstencil.jafnify;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.TimeZone;

/**
 * The diagnostic journal (flight recorder): {@code diag-0.log} plus four
 * rotations.
 *
 * <p>One record is one line:
 * {@code ts=<UTC ISO-8601 ms> kind=STATS|EVENT key=value ...}. Values are
 * redacted with {@link ConnectionLog#redact(String)} and every whitespace
 * character is replaced with {@code _}, so each value is one whitespace-free
 * token. The map's iteration order is preserved.
 *
 * <p>The active file is {@code diag-0.log}; before a record would push it past
 * {@link #MAX_BYTES} the journal shifts {@code diag-3} to {@code diag-4}, and
 * so on down to {@code diag-0} to {@code diag-1}, discards the old
 * {@code diag-4} and reopens a fresh {@code diag-0}. Every record is flushed
 * immediately.
 *
 * <p>Pure Java: the directory is a plain {@link File}, so it runs as a plain
 * JVM unit test. I/O failures surface as {@link IllegalStateException}; nothing
 * here reads a clock or touches the network.
 */
final class DiagLog {
    /** Rotate before a record would grow the active file past this many bytes. */
    static final long MAX_BYTES = 2L * 1024 * 1024;

    /** Files kept including the active one. */
    static final int KEEP = 5;

    private static final String ACTIVE_NAME = "diag-0.log";

    private final File dir;
    private final File active;
    private FileOutputStream out;
    private long size;

    /** Create {@code dir} if needed and append to its active journal file. */
    DiagLog(File dir) {
        if (dir == null) {
            throw new IllegalArgumentException("dir");
        }
        this.dir = dir.getAbsoluteFile();
        ensureDirectory(this.dir);
        this.active = new File(this.dir, ACTIVE_NAME);
        try {
            openActive();
        } catch (IOException e) {
            throw new IllegalStateException("cannot open " + active, e);
        }
    }

    /** Append one {@code kind=STATS} record. */
    synchronized void stats(long nowMs, Map<String, String> fields) {
        writeRecord(formatLine(nowMs, "STATS", fields));
    }

    /** Append one {@code kind=EVENT event=<name>} record. */
    synchronized void event(long nowMs, String name, Map<String, String> fields) {
        Map<String, String> all = new LinkedHashMap<>();
        all.put("event", name);
        if (fields != null) {
            all.putAll(fields);
        }
        writeRecord(formatLine(nowMs, "EVENT", all));
    }

    /** Flush and close the active file. Safe to call more than once. */
    synchronized void close() {
        closeActive();
    }

    /** The directory holding {@code diag-0.log}..{@code diag-4.log}. */
    File dir() {
        return dir;
    }

    /** One journal line without its trailing newline. */
    static String formatLine(long nowMs, String kind, Map<String, String> fields) {
        StringBuilder line = new StringBuilder();
        line.append("ts=").append(timestamp(nowMs)).append(" kind=").append(kind);
        if (fields != null) {
            for (Map.Entry<String, String> field : fields.entrySet()) {
                line.append(' ').append(field.getKey()).append('=')
                        .append(sanitize(field.getValue()));
            }
        }
        return line.toString();
    }

    /** {@code 2026-10-31T12:34:56.789Z} from UTC epoch milliseconds. */
    static String timestamp(long epochMs) {
        SimpleDateFormat format =
                new SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US);
        format.setTimeZone(TimeZone.getTimeZone("UTC"));
        return format.format(new Date(epochMs));
    }

    /** Redact key-shaped base64, then keep every value one whitespace-free token. */
    private static String sanitize(String value) {
        String redacted = ConnectionLog.redact(value);
        StringBuilder out = new StringBuilder(redacted.length());
        for (int i = 0; i < redacted.length(); i++) {
            char c = redacted.charAt(i);
            out.append(Character.isWhitespace(c) || Character.isSpaceChar(c) ? '_' : c);
        }
        return out.toString();
    }

    private void writeRecord(String line) {
        if (out == null) {
            throw new IllegalStateException("DiagLog is closed");
        }
        byte[] bytes = line.getBytes(StandardCharsets.UTF_8);
        long recordBytes = bytes.length + 1L;
        try {
            if (size + recordBytes > MAX_BYTES) {
                rotate();
            }
            out.write(bytes);
            out.write('\n');
            out.flush();
            size += recordBytes;
        } catch (IOException e) {
            throw new IllegalStateException("cannot write " + active, e);
        }
    }

    private void openActive() throws IOException {
        out = new FileOutputStream(active, true);
        size = active.length();
    }

    /** {@code diag-0 -> diag-1}, ..., {@code diag-3 -> diag-4}; drop old 4. */
    private void rotate() throws IOException {
        closeActive();

        File oldest = file(KEEP - 1);
        if (oldest.exists() && !oldest.delete() && oldest.exists()) {
            throw new IOException("cannot delete " + oldest);
        }
        for (int n = KEEP - 2; n >= 1; n--) {
            File from = file(n);
            if (!from.exists()) {
                continue;
            }
            File to = file(n + 1);
            if (!from.renameTo(to)) {
                throw new IOException("cannot rotate " + from + " to " + to);
            }
        }
        if (active.exists() && !active.renameTo(file(1))) {
            throw new IOException("cannot rotate " + active + " to " + file(1));
        }
        openActive();
    }

    private File file(int n) {
        return new File(dir, "diag-" + n + ".log");
    }

    private static void ensureDirectory(File dir) {
        if (dir.isDirectory()) {
            return;
        }
        if (!dir.mkdirs() && !dir.isDirectory()) {
            IOException cause = new IOException("cannot create " + dir);
            throw new IllegalStateException(cause.getMessage(), cause);
        }
    }

    private void closeActive() {
        FileOutputStream stream = out;
        out = null;
        if (stream == null) {
            return;
        }
        try {
            stream.flush();
        } catch (IOException ignored) {
            // Closing is best effort; the records already reached the OS.
        }
        try {
            stream.close();
        } catch (IOException ignored) {
            // Nothing left to flush.
        }
    }
}
