package com.siliconstencil.jafnify;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

/**
 * Zip export of the diagnostic journal, plus the small pure parsing helper the
 * service uses to keep STATS records in a deterministic order.
 *
 * <p>Pure Java (no Android types), so the archive layout and the parser run as
 * plain JVM unit tests. The production zip redacts each entry through the
 * native core redactor before writing it. Export only reads the journal: the
 * source files are never deleted, renamed or truncated. A missing or empty
 * journal directory still produces a valid, empty zip.
 */
final class DiagExport {
    /** File name used by the service's cache and the content provider. */
    static final String ZIP_NAME = "diag.zip";

    /** Redacts one whole export text. The native implementation is the core
     *  redactor; tests supply their own so the archive layout runs without JNI. */
    interface TextRedactor {
        String redact(String text, String aliases);
    }

    private static final TextRedactor NATIVE_REDACTOR = new TextRedactor() {
        @Override
        public String redact(String text, String aliases) {
            return Native.redactForExport(text, aliases);
        }
    };

    private DiagExport() {
    }

    /**
     * Write every {@code diag-*.log} file in {@code diagDir} into
     * {@code outZip}, overwriting an existing archive. Entries are in file-name
     * order. Missing/empty input yields an empty archive.
     *
     * @return {@code outZip}
     */
    static File zip(File diagDir, File outZip) throws IOException {
        return zip(diagDir, outZip, "", NATIVE_REDACTOR);
    }

    /** The same archive with the peer aliases the export knows. */
    static File zip(File diagDir, File outZip, String aliases) throws IOException {
        return zip(diagDir, outZip, aliases, NATIVE_REDACTOR);
    }

    static File zip(File diagDir, File outZip, TextRedactor redactor) throws IOException {
        return zip(diagDir, outZip, "", redactor);
    }

    /**
     * The same archive with an explicit redactor. Each entry's whole text is
     * redacted in one pass before it is written, so a peer keeps one label
     * inside the entry; the same alias list goes to every entry, so a peer
     * keeps one label across the whole archive. The source file is only read.
     *
     * <p>Memory is bounded by the journal itself: {@code DiagLog.MAX_BYTES}
     * caps a file at 2 MiB and {@code DiagLog.KEEP} keeps five, so the archive
     * is at most 10 MiB and at most one 2 MiB entry is held while it is
     * redacted. The archive is streamed to disk entry by entry, never held
     * whole in memory.
     *
     * @return {@code outZip}
     */
    static File zip(File diagDir, File outZip, String aliases, TextRedactor redactor)
            throws IOException {
        if (outZip == null) {
            throw new IllegalArgumentException("outZip");
        }
        if (redactor == null) {
            throw new IllegalArgumentException("redactor");
        }
        String aliasText = aliases == null ? "" : aliases;
        File out = outZip.getAbsoluteFile();
        File parent = out.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs() && !parent.isDirectory()) {
            throw new IOException("cannot create " + parent);
        }

        List<File> logs = logFiles(diagDir);
        try (ZipOutputStream zip = new ZipOutputStream(
                new BufferedOutputStream(new FileOutputStream(out)))) {
            for (File log : logs) {
                ZipEntry entry = new ZipEntry(log.getName());
                entry.setTime(log.lastModified());
                zip.putNextEntry(entry);
                String text = new String(readAll(log), StandardCharsets.UTF_8);
                zip.write(redactor.redact(text, aliasText).getBytes(StandardCharsets.UTF_8));
                zip.closeEntry();
            }
        }
        return out;
    }

    /** Every byte of one journal file. The export never changes the source. */
    private static byte[] readAll(File file) throws IOException {
        try (InputStream in = new BufferedInputStream(new FileInputStream(file))) {
            ByteArrayOutputStream out =
                    new ByteArrayOutputStream((int) Math.max(0, file.length()));
            byte[] buffer = new byte[8192];
            int read;
            while ((read = in.read(buffer)) >= 0) {
                if (read > 0) {
                    out.write(buffer, 0, read);
                }
            }
            return out.toByteArray();
        }
    }

    /**
     * The journal files in export order. A directory that does not exist, is
     * not a directory, or cannot be listed has no files (not an error).
     */
    static List<File> logFiles(File diagDir) {
        List<File> logs = new ArrayList<>();
        if (diagDir == null || !diagDir.isDirectory()) {
            return logs;
        }
        File[] files = diagDir.listFiles();
        if (files == null) {
            return logs;
        }
        for (File file : files) {
            String name = file.getName();
            if (file.isFile() && name.startsWith("diag-") && name.endsWith(".log")) {
                logs.add(file);
            }
        }
        Collections.sort(logs, new Comparator<File>() {
            @Override
            public int compare(File a, File b) {
                return a.getName().compareTo(b.getName());
            }
        });
        return logs;
    }

    /**
     * Parse the native snapshot's {@code STATS key=value ...} first line into an
     * insertion-ordered map. Malformed tokens are skipped; values may be empty.
     * Nothing here reads a clock or touches Android, so a STATS record keeps
     * exactly the order the native side emitted.
     */
    static LinkedHashMap<String, String> parseStatsLine(String line) {
        LinkedHashMap<String, String> fields = new LinkedHashMap<>();
        if (line == null) {
            return fields;
        }
        String body = line.trim();
        if (body.equals("STATS")) {
            return fields;
        }
        if (!body.startsWith("STATS ")) {
            return fields;
        }
        body = body.substring("STATS ".length()).trim();
        if (body.isEmpty()) {
            return fields;
        }
        for (String token : body.split(" ")) {
            int eq = token.indexOf('=');
            if (eq <= 0) {
                continue;
            }
            String key = token.substring(0, eq);
            String value = token.substring(eq + 1);
            fields.put(key, value);
        }
        return fields;
    }
}
