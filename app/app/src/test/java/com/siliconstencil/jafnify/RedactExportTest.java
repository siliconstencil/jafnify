package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * The Android export boundary: the zip and the copied/shared connection log
 * are redacted in one pass over the whole text, while the on-disk journal is
 * only read.
 *
 * <p>The core rules live in Rust and are covered by the core tests. The app
 * only ships an arm64 {@code libwgserv.so}, which a host JVM cannot load, so
 * these tests use a recording double for the native redactor: they prove the
 * Java wiring (what is passed, in how many passes, and what is left on disk),
 * not the rule engine itself.
 */
public class RedactExportTest {
    @Rule
    public TemporaryFolder temp = new TemporaryFolder();

    private static final long T = 1_700_000_000_000L;

    /** One public address is replaced, the LAN address is left alone, and
     *  every whole text and alias list handed in is recorded. */
    private static final class Recorder
            implements DiagExport.TextRedactor, ConnectionLog.ExportRedactor {
        final List<String> calls = new ArrayList<>();
        final List<String> aliasCalls = new ArrayList<>();

        @Override
        public String redact(String text, String aliases) {
            calls.add(text);
            aliasCalls.add(aliases);
            return text.replace("203.0.113.7", "<public-ip>");
        }
    }

    /** A deterministic stand-in for the native label assignment: the first
     *  alias of a peer becomes peer-1. */
    private static final class AliasLabelRedactor implements DiagExport.TextRedactor {
        final List<String> aliasCalls = new ArrayList<>();

        @Override
        public String redact(String text, String aliases) {
            aliasCalls.add(aliases);
            String out = text;
            for (String line : aliases.split("\n")) {
                String[] parts = line.split("\t");
                if (parts.length == 0 || parts[0].isEmpty()) {
                    continue;
                }
                out = out.replace(parts[0], "peer-1");
            }
            return out;
        }
    }

    private static String read(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }

    private static String readEntry(ZipFile zip, String name) throws IOException {
        ZipEntry entry = zip.getEntry(name);
        assertTrue("missing zip entry " + name, entry != null);
        try (InputStream in = zip.getInputStream(entry)) {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] buffer = new byte[8192];
            int read;
            while ((read = in.read(buffer)) >= 0) {
                if (read > 0) {
                    out.write(buffer, 0, read);
                }
            }
            return new String(out.toByteArray(), StandardCharsets.UTF_8);
        }
    }

    @Test
    public void theZipCarriesRedactedEntriesAndTheJournalStaysOriginal() throws Exception {
        File dir = temp.newFolder("diag");
        File journal = new File(dir, "diag-0.log");
        String line = "ts=1 kind=STATS peer=203.0.113.7:9797 local=10.1.0.2\n";
        Files.write(journal.toPath(), line.getBytes(StandardCharsets.UTF_8));
        Recorder recorder = new Recorder();

        File out = new File(temp.getRoot(), "cache/diag.zip");
        DiagExport.zip(dir, out, recorder);

        try (ZipFile zip = new ZipFile(out)) {
            String entry = readEntry(zip, "diag-0.log");
            assertTrue(entry.contains("<public-ip>"));
            assertFalse(entry.contains("203.0.113.7"));
            // The LAN address is kept byte for byte.
            assertTrue(entry.contains("local=10.1.0.2"));
            // One whole-file pass, so a peer keeps one label within the file.
            assertEquals(Arrays.asList(line), recorder.calls);
        }
        // Export only reads: the local journal still has the original bytes.
        assertEquals(line, read(journal));
    }

    @Test
    public void theJournalKeepsThePublicEndpointAndTheZipRedactsIt() throws Exception {
        File dir = temp.newFolder("diag-journal");
        DiagLog journal = new DiagLog(dir);
        try {
            LinkedHashMap<String, String> fields = new LinkedHashMap<>();
            fields.put("peer", "203.0.113.7:9797");
            fields.put("local", "10.1.0.2");
            journal.stats(T, fields);
        } finally {
            journal.close();
        }

        // The on-disk journal is local debugging material: the endpoint stays.
        String onDisk = read(new File(dir, "diag-0.log"));
        assertTrue(onDisk, onDisk.contains("203.0.113.7:9797"));

        Recorder recorder = new Recorder();
        File out = new File(temp.getRoot(), "cache/diag-journal.zip");
        DiagExport.zip(dir, out, recorder);

        try (ZipFile zip = new ZipFile(out)) {
            String entry = readEntry(zip, "diag-0.log");
            assertTrue(entry.contains("<public-ip>"));
            assertFalse(entry.contains("203.0.113.7"));
            // The LAN address is kept byte for byte.
            assertTrue(entry.contains("local=10.1.0.2"));
        }
    }

    @Test
    public void theCopiedOrSharedLogIsRedactedAsOneWholeText() {
        ConnectionLog log = new ConnectionLog();
        log.push("reconnect", "reconnected (peer 203.0.113.7:9797)", T);
        log.push("state", "state Connected -> Connected", T + 1_000L);
        log.push("reconnect", "reconnected (peer 10.1.0.2:9797)", T + 2_000L);
        String whole = log.text();
        Recorder recorder = new Recorder();

        String exported = ConnectionLog.exportText(whole, recorder);

        assertTrue(exported.contains("<public-ip>"));
        assertFalse(exported.contains("203.0.113.7"));
        // The LAN address is kept byte for byte.
        assertTrue(exported.contains("peer 10.1.0.2:9797"));
        // One pass over the whole log: the same peer cannot get two labels.
        assertEquals(1, recorder.calls.size());
        assertEquals(whole, recorder.calls.get(0));
        // The on-screen log keeps its text.
        assertEquals(whole, log.text());
    }

    @Test
    public void anEmptyExportStaysEmpty() {
        Recorder recorder = new Recorder();

        assertEquals("", ConnectionLog.exportText(null, recorder));
        assertEquals("", ConnectionLog.exportText("", recorder));
    }

    @Test
    public void theCopiedLogGetsThePeerAliasesInOnePass() {
        ConnectionLog log = new ConnectionLog();
        log.push("reconnect", "reconnected (peer Phone - Blue Falcon)", T);
        Recorder recorder = new Recorder();

        String exported = ConnectionLog.exportText(
                log.text(), "Phone - Blue Falcon", recorder);

        assertEquals(exported, log.text());
        assertEquals(1, recorder.calls.size());
        assertEquals(Arrays.asList("Phone - Blue Falcon"), recorder.aliasCalls);
    }

    @Test
    public void theSameAliasListGivesTheSameLabelInEveryEntry() throws Exception {
        File dir = temp.newFolder("diag-aliases");
        String alias = "Phone - Blue Falcon";
        Files.write(new File(dir, "diag-0.log").toPath(),
                ("first peer " + alias + "\n").getBytes(StandardCharsets.UTF_8));
        Files.write(new File(dir, "diag-1.log").toPath(),
                ("second peer " + alias + "\n").getBytes(StandardCharsets.UTF_8));
        AliasLabelRedactor redactor = new AliasLabelRedactor();

        File out = new File(temp.getRoot(), "cache/diag.zip");
        DiagExport.zip(dir, out, alias, redactor);

        try (ZipFile zip = new ZipFile(out)) {
            assertEquals("first peer peer-1\n", readEntry(zip, "diag-0.log"));
            assertEquals("second peer peer-1\n", readEntry(zip, "diag-1.log"));
        }
        // Every entry got the same alias list, so no entry can assign a
        // different label to the same peer.
        assertEquals(Arrays.asList(alias, alias), redactor.aliasCalls);
    }
}
