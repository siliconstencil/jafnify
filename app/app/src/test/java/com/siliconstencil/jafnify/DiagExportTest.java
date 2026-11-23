package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Enumeration;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/** Zip export of the diagnostic journal: contents, sources, empty cases, order. */
public class DiagExportTest {
    @Rule
    public TemporaryFolder temp = new TemporaryFolder();

    /** These tests pin the archive layout, not the rule engine: identity keeps
     *  the expected bytes while the native redactor stays out of the JVM. */
    private static final DiagExport.TextRedactor IDENTITY = new DiagExport.TextRedactor() {
        @Override
        public String redact(String text, String aliases) {
            return text;
        }
    };

    private static String read(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }

    private static String readEntry(ZipFile zip, String name) throws IOException {
        ZipEntry entry = zip.getEntry(name);
        assertNotNull("missing zip entry " + name, entry);
        try (InputStream in = zip.getInputStream(entry)) {
            byte[] bytes = new byte[(int) entry.getSize()];
            int offset = 0;
            while (offset < bytes.length) {
                int read = in.read(bytes, offset, bytes.length - offset);
                if (read < 0) {
                    break;
                }
                offset += read;
            }
            return new String(bytes, 0, offset, StandardCharsets.UTF_8);
        }
    }

    private static List<String> names(ZipFile zip) {
        List<String> names = new ArrayList<>();
        Enumeration<? extends ZipEntry> entries = zip.entries();
        while (entries.hasMoreElements()) {
            names.add(entries.nextElement().getName());
        }
        return names;
    }

    @Test
    public void theUsageStoreIsNeverInTheDiagnosticsZip() throws Exception {
        File dir = temp.newFolder("diag");
        Files.write(new File(dir, "diag-0.log").toPath(),
                "ts=1 kind=STATS a=b\n".getBytes(StandardCharsets.UTF_8));
        Files.write(new File(dir, "phone-stats.tsv").toPath(),
                ("JFU1 1 hour=3600000 up=10 down=30 min_up=1 min_down=2 "
                        + "peak_up=3 peak_down=4 connected_ms=1000 samples=1 net=wifi\n")
                        .getBytes(StandardCharsets.UTF_8));
        File out = new File(temp.getRoot(), "diag.zip");

        DiagExport.zip(dir, out, IDENTITY);

        try (ZipFile zip = new ZipFile(out)) {
            assertEquals(1, zip.size());
            assertFalse(names(zip).contains("phone-stats.tsv"));
            assertFalse(readEntry(zip, "diag-0.log").contains("JFU1"));
        }
    }

    @Test
    public void everyDiagLogIsInTheZipAndTheSourcesAreUntouched() throws Exception {
        File dir = temp.newFolder("diag");
        File active = new File(dir, "diag-0.log");
        File rotated = new File(dir, "diag-2.log");
        File oldest = new File(dir, "diag-4.log");
        Files.write(active.toPath(), "ts=1 kind=STATS a=b\n".getBytes(StandardCharsets.UTF_8));
        Files.write(rotated.toPath(), "ts=2 kind=EVENT event=x\n".getBytes(StandardCharsets.UTF_8));
        Files.write(oldest.toPath(), "ts=3 kind=STATS c=d\n".getBytes(StandardCharsets.UTF_8));
        File unrelated = new File(dir, "sessions.tsv");
        Files.write(unrelated.toPath(), "not a journal\n".getBytes(StandardCharsets.UTF_8));
        byte[] activeBefore = Files.readAllBytes(active.toPath());
        byte[] rotatedBefore = Files.readAllBytes(rotated.toPath());
        byte[] oldestBefore = Files.readAllBytes(oldest.toPath());

        File out = new File(temp.getRoot(), "cache/diag.zip");
        File result = DiagExport.zip(dir, out, IDENTITY);

        assertEquals(out.getAbsoluteFile(), result.getAbsoluteFile());
        assertTrue(out.isFile());
        try (ZipFile zip = new ZipFile(out)) {
            List<String> names = names(zip);
            assertEquals(3, names.size());
            assertTrue(names.contains("diag-0.log"));
            assertTrue(names.contains("diag-2.log"));
            assertTrue(names.contains("diag-4.log"));
            assertFalse(names.contains("sessions.tsv"));
            assertEquals("ts=1 kind=STATS a=b\n", readEntry(zip, "diag-0.log"));
            assertEquals("ts=2 kind=EVENT event=x\n", readEntry(zip, "diag-2.log"));
            assertEquals("ts=3 kind=STATS c=d\n", readEntry(zip, "diag-4.log"));
        }

        // Export only reads: every source keeps its bytes and still exists.
        assertTrue(active.isFile());
        assertTrue(rotated.isFile());
        assertTrue(oldest.isFile());
        assertTrue(Arrays.equals(activeBefore, Files.readAllBytes(active.toPath())));
        assertTrue(Arrays.equals(rotatedBefore, Files.readAllBytes(rotated.toPath())));
        assertTrue(Arrays.equals(oldestBefore, Files.readAllBytes(oldest.toPath())));
    }

    @Test
    public void aMissingDirectoryProducesAnEmptyZipWithoutThrowing() throws Exception {
        File missing = new File(temp.getRoot(), "absent");
        File out = new File(temp.getRoot(), "cache/diag.zip");

        File result = DiagExport.zip(missing, out, IDENTITY);

        assertNotNull(result);
        assertTrue(result.isFile());
        try (ZipFile zip = new ZipFile(result)) {
            assertEquals(0, zip.size());
        }
        assertFalse(missing.exists());
    }

    @Test
    public void anEmptyDirectoryProducesAnEmptyZipWithoutThrowing() throws Exception {
        File empty = temp.newFolder("empty-diag");
        File out = new File(temp.getRoot(), "diag.zip");

        File result = DiagExport.zip(empty, out, IDENTITY);

        assertNotNull(result);
        try (ZipFile zip = new ZipFile(result)) {
            assertEquals(0, zip.size());
        }
    }

    @Test
    public void anExistingArchiveIsOverwritten() throws Exception {
        File dir = temp.newFolder("diag");
        Files.write(new File(dir, "diag-0.log").toPath(),
                "fresh\n".getBytes(StandardCharsets.UTF_8));
        File out = new File(temp.getRoot(), "diag.zip");
        Files.write(out.toPath(), "old garbage".getBytes(StandardCharsets.UTF_8));

        DiagExport.zip(dir, out, IDENTITY);

        try (ZipFile zip = new ZipFile(out)) {
            assertEquals(1, zip.size());
            assertEquals("fresh\n", readEntry(zip, "diag-0.log"));
        }
    }

    @Test
    public void statsFieldsKeepTheNativeOrderForDeterministicRecords() {
        String line = "STATS udp_rx_packets=4 udp_rx_bytes=5 last_rx_ms=-1"
                + " peer=-1 local=-1 wg_rx_bytes=7";

        LinkedHashMap<String, String> fields = DiagExport.parseStatsLine(line);

        assertEquals(Arrays.asList("udp_rx_packets", "udp_rx_bytes", "last_rx_ms",
                "peer", "local", "wg_rx_bytes"), new ArrayList<>(fields.keySet()));
        assertEquals("4", fields.get("udp_rx_packets"));
        assertEquals("-1", fields.get("last_rx_ms"));
        // Parsing twice gives the same order: the record layout is determined
        // by the line, not by a hash map's iteration.
        assertEquals(new ArrayList<>(fields.keySet()),
                new ArrayList<>(DiagExport.parseStatsLine(line).keySet()));
    }

    @Test
    public void parseStatsLineIsDefensiveAboutMissingOrMalformedInput() {
        assertTrue(DiagExport.parseStatsLine(null).isEmpty());
        assertTrue(DiagExport.parseStatsLine("").isEmpty());
        assertTrue(DiagExport.parseStatsLine("STATS").isEmpty());
        Map<String, String> fields = DiagExport.parseStatsLine("STATS a=1 broken =2 b=");
        assertEquals(2, fields.size());
        assertEquals("1", fields.get("a"));
        assertEquals("", fields.get("b"));
        assertNull(fields.get("broken"));
    }
}
