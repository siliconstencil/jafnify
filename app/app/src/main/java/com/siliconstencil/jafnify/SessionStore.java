package com.siliconstencil.jafnify;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.net.URLDecoder;
import java.net.URLEncoder;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

/**
 * Completed sessions, newest first, in a private line-oriented TSV file.
 * The current session is never written here.
 *
 * <p>Each record is one line with 19 tab-separated fields. A 0.1.1 line has
 * 18: the missing start reason reads as empty. String fields use
 * {@link URLEncoder} escaping so tabs, newlines and percent signs cannot break
 * the format; reconnect records live in one field as {@code ;}-separated
 * groups of five {@code ,}-separated values. Writes use a same-directory temp
 * file, fsync and rename, like the Linux {@code SessionHistory}.
 *
 * <p>Pure Java: the path is a plain {@link File}, so the tests can inject a
 * temporary file and no Android API is needed.
 */
final class SessionStore {
    /** Completed sessions kept in the file. */
    static final int MAX_SESSIONS = 50;

    private static final int FIELD_COUNT = 19;
    private static final int OLD_FIELD_COUNT = 18;
    private static final int RECONNECT_FIELDS = 5;
    private static final String FIELD = "\t";
    private static final String RECONNECT_SEPARATOR = ";";
    private static final String RECONNECT_FIELD = ",";

    private static final Comparator<SessionRecord> NEWEST_FIRST =
            new Comparator<SessionRecord>() {
                @Override
                public int compare(SessionRecord left, SessionRecord right) {
                    return Long.compare(right.startedAtMs, left.startedAtMs);
                }
            };

    private final File sessionsTsv;

    SessionStore(File sessionsTsv) {
        if (sessionsTsv == null) {
            throw new IllegalArgumentException("sessionsTsv");
        }
        this.sessionsTsv = sessionsTsv.getAbsoluteFile();
    }

    /** Completed sessions, newest first. Missing, bad lines or I/O are empty-safe. */
    List<SessionRecord> load() {
        try {
            return readRecords(false);
        } catch (IOException e) {
            return new ArrayList<>();
        }
    }

    /**
     * Completed sessions, newest first, with errors reported. A missing or
     * empty file is valid and empty; an unreadable, undecodable or malformed
     * existing file throws. append() uses this so a bad read never overwrites
     * history.
     */
    List<SessionRecord> loadStrict() throws IOException {
        return readRecords(true);
    }

    private List<SessionRecord> readRecords(boolean strict) throws IOException {
        List<SessionRecord> records = new ArrayList<>();
        if (!sessionsTsv.exists()) {
            return records;
        }
        if (!sessionsTsv.isFile()) {
            throw new IOException(sessionsTsv + " is not a file");
        }
        String text = readUtf8(sessionsTsv);
        String[] lines = text.split("\n", -1);
        for (int i = 0; i < lines.length; i++) {
            String line = lines[i];
            if (line.endsWith("\r")) {
                line = line.substring(0, line.length() - 1);
            }
            if (line.isEmpty()) {
                continue;
            }
            SessionRecord record = parse(line);
            if (record == null) {
                if (strict) {
                    throw new IOException("malformed session line " + (i + 1));
                }
                continue;
            }
            records.add(record);
        }
        Collections.sort(records, NEWEST_FIRST);
        if (records.size() > MAX_SESSIONS) {
            records.subList(MAX_SESSIONS, records.size()).clear();
        }
        return records;
    }

    /** Write the newest 50 records atomically, owner-only where supported. */
    void save(List<SessionRecord> records) throws IOException {
        List<SessionRecord> newest = new ArrayList<>();
        if (records != null) {
            newest.addAll(records);
        }
        Collections.sort(newest, NEWEST_FIRST);
        if (newest.size() > MAX_SESSIONS) {
            newest.subList(MAX_SESSIONS, newest.size()).clear();
        }

        File parent = sessionsTsv.getParentFile();
        ensurePrivateDir(parent);
        File temp = tempFile();
        if (temp.exists() && !temp.delete()) {
            throw new IOException("cannot remove stale temp file " + temp);
        }

        boolean renamed = false;
        try {
            FileOutputStream out = new FileOutputStream(temp);
            try {
                restrictOwnerOnly(temp, false);
                byte[] text = render(newest).getBytes(StandardCharsets.UTF_8);
                out.write(text);
                out.flush();
                out.getFD().sync();
            } finally {
                out.close();
            }
            renamed = temp.renameTo(sessionsTsv);
            if (!renamed) {
                throw new IOException("cannot replace " + sessionsTsv);
            }
        } finally {
            if (!renamed && temp.exists()) {
                temp.delete();
            }
        }
    }

    /**
     * Insert a completed session and save. The record is treated as the newest
     * completed session; save() owns sorting and the 50-record cap. Returns
     * false without touching existing bytes when the current history cannot be
     * read, parsed or saved.
     */
    boolean append(SessionRecord record) {
        if (record == null) {
            return false;
        }
        List<SessionRecord> records;
        try {
            records = loadStrict();
        } catch (IOException e) {
            return false;
        }
        records.add(0, record);
        try {
            save(records);
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    private File tempFile() {
        return new File(sessionsTsv.getParentFile(), "." + sessionsTsv.getName() + ".tmp");
    }

    private static String render(List<SessionRecord> records) {
        StringBuilder text = new StringBuilder();
        for (SessionRecord record : records) {
            String[] fields = new String[] {
                    escape(record.id),
                    escape(record.role),
                    Long.toString(record.startedAtMs),
                    Long.toString(record.lastActivityAtMs),
                    Long.toString(record.endedAtMs),
                    Long.toString(record.connectedMs),
                    Long.toString(record.upBytes),
                    Long.toString(record.downBytes),
                    Long.toString(record.avgUpBps),
                    Long.toString(record.avgDownBps),
                    Long.toString(record.peakUpBps),
                    Long.toString(record.peakDownBps),
                    Long.toString(record.drops),
                    Long.toString(record.linkDowns),
                    Long.toString(record.linkUps),
                    renderReconnects(record.reconnects),
                    escape(record.peerEndpoint),
                    escape(record.endReason),
                    escape(record.startReason),
            };
            for (int i = 0; i < fields.length; i++) {
                if (i > 0) {
                    text.append(FIELD);
                }
                text.append(fields[i]);
            }
            text.append('\n');
        }
        return text.toString();
    }

    private static String renderReconnects(List<SessionRecord.Reconnect> reconnects) {
        StringBuilder out = new StringBuilder();
        if (reconnects == null) {
            return "";
        }
        for (int i = 0; i < reconnects.size(); i++) {
            if (i > 0) {
                out.append(RECONNECT_SEPARATOR);
            }
            SessionRecord.Reconnect reconnect = reconnects.get(i);
            out.append(reconnect.disconnectedAtMs).append(RECONNECT_FIELD)
                    .append(reconnect.detectedAtMs).append(RECONNECT_FIELD)
                    .append(reconnect.connectedAtMs).append(RECONNECT_FIELD)
                    .append(reconnect.durationMs).append(RECONNECT_FIELD)
                    .append(escape(reconnect.reason));
        }
        return out.toString();
    }

    /** Null for a malformed line; the caller simply skips it. */
    private static SessionRecord parse(String line) {
        String[] parts = line.split(FIELD, -1);
        if (parts.length != FIELD_COUNT && parts.length != OLD_FIELD_COUNT) {
            return null;
        }
        try {
            SessionRecord record = new SessionRecord();
            record.id = unescape(parts[0]);
            record.role = unescape(parts[1]);
            record.startedAtMs = Long.parseLong(parts[2]);
            record.lastActivityAtMs = Long.parseLong(parts[3]);
            record.endedAtMs = Long.parseLong(parts[4]);
            record.connectedMs = Long.parseLong(parts[5]);
            record.upBytes = Long.parseLong(parts[6]);
            record.downBytes = Long.parseLong(parts[7]);
            record.avgUpBps = Long.parseLong(parts[8]);
            record.avgDownBps = Long.parseLong(parts[9]);
            record.peakUpBps = Long.parseLong(parts[10]);
            record.peakDownBps = Long.parseLong(parts[11]);
            record.drops = Long.parseLong(parts[12]);
            record.linkDowns = Long.parseLong(parts[13]);
            record.linkUps = Long.parseLong(parts[14]);
            List<SessionRecord.Reconnect> reconnects = parseReconnects(parts[15]);
            if (reconnects == null) {
                return null;
            }
            record.reconnects = reconnects;
            record.peerEndpoint = unescape(parts[16]);
            record.endReason = unescape(parts[17]);
            record.startReason = parts.length == FIELD_COUNT ? unescape(parts[18]) : "";
            return record;
        } catch (IllegalArgumentException e) {
            // A bad number or escape means a malformed record.
            return null;
        }
    }

    private static List<SessionRecord.Reconnect> parseReconnects(String field) {
        List<SessionRecord.Reconnect> reconnects = new ArrayList<>();
        if (field.isEmpty()) {
            return reconnects;
        }
        String[] groups = field.split(RECONNECT_SEPARATOR, -1);
        for (String group : groups) {
            String[] parts = group.split(RECONNECT_FIELD, -1);
            if (parts.length != RECONNECT_FIELDS) {
                return null;
            }
            SessionRecord.Reconnect reconnect = new SessionRecord.Reconnect();
            reconnect.disconnectedAtMs = Long.parseLong(parts[0]);
            reconnect.detectedAtMs = Long.parseLong(parts[1]);
            reconnect.connectedAtMs = Long.parseLong(parts[2]);
            reconnect.durationMs = Long.parseLong(parts[3]);
            reconnect.reason = unescape(parts[4]);
            reconnects.add(reconnect);
        }
        return reconnects;
    }

    private static String escape(String value) {
        if (value == null) {
            return "";
        }
        try {
            return URLEncoder.encode(value, "UTF-8");
        } catch (IOException e) {
            // UTF-8 is always available.
            throw new IllegalStateException(e);
        }
    }

    private static String unescape(String value) {
        try {
            return URLDecoder.decode(value, "UTF-8");
        } catch (IOException e) {
            throw new IllegalArgumentException(e);
        }
    }

    private static void ensurePrivateDir(File dir) throws IOException {
        if (dir == null) {
            return;
        }
        if (dir.exists()) {
            if (!dir.isDirectory()) {
                throw new IOException(dir + " is not a directory");
            }
            return;
        }
        if (!dir.mkdirs() && !dir.isDirectory()) {
            throw new IOException("cannot create " + dir);
        }
        restrictOwnerOnly(dir, true);
    }

    private static void restrictOwnerOnly(File file, boolean directory) {
        try {
            file.setReadable(false, false);
            file.setWritable(false, false);
            file.setExecutable(false, false);
            file.setReadable(true, true);
            file.setWritable(true, true);
            if (directory) {
                file.setExecutable(true, true);
            }
        } catch (SecurityException e) {
            // A filesystem that cannot express these modes keeps its default.
        }
    }

    /** Read the whole file, rejecting bytes that are not valid UTF-8. */
    private static String readUtf8(File file) throws IOException {
        byte[] bytes;
        FileInputStream in = new FileInputStream(file);
        try {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            int read;
            while ((read = in.read(buffer)) != -1) {
                out.write(buffer, 0, read);
            }
            bytes = out.toByteArray();
        } finally {
            in.close();
        }
        CharsetDecoder decoder = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT);
        try {
            return decoder.decode(ByteBuffer.wrap(bytes)).toString();
        } catch (CharacterCodingException e) {
            throw new IOException("session file is not valid UTF-8", e);
        }
    }
}
