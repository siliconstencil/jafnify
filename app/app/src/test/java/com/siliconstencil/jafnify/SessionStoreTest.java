package com.siliconstencil.jafnify;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import org.junit.Assume;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileSystems;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/**
 * The Java twin of linux/tests/session_store_test.rs plus the TSV-specific
 * escaping: retention, order, permissions, atomic replacement and privacy.
 */
public class SessionStoreTest {
    @Rule
    public TemporaryFolder folder = new TemporaryFolder();

    private static SessionRecord record(String id, long startedAt) {
        SessionRecord record = new SessionRecord();
        record.id = id;
        record.role = "joiner";
        record.startedAtMs = startedAt;
        record.lastActivityAtMs = startedAt + 1_000;
        record.endedAtMs = startedAt + 2_000;
        record.connectedMs = 2_000;
        record.upBytes = 80;
        record.downBytes = 300;
        record.avgUpBps = 40;
        record.avgDownBps = 150;
        record.peakUpBps = 50;
        record.peakDownBps = 200;
        record.drops = 1;
        record.linkDowns = 0;
        record.linkUps = 0;
        SessionRecord.Reconnect reconnect = new SessionRecord.Reconnect();
        reconnect.disconnectedAtMs = startedAt + 500;
        reconnect.detectedAtMs = startedAt + 1_000;
        reconnect.connectedAtMs = startedAt + 1_500;
        reconnect.durationMs = 1_000;
        reconnect.reason = "no data for 2 s";
        record.reconnects = new ArrayList<>(Collections.singletonList(reconnect));
        record.peerEndpoint = "10.1.0.2:9797";
        record.endReason = "stopped";
        return record;
    }

    private File path() {
        return new File(folder.getRoot(), "sessions.tsv");
    }

    private static void appendRaw(File file, String text) throws IOException {
        FileOutputStream out = new FileOutputStream(file, true);
        try {
            out.write(text.getBytes(StandardCharsets.UTF_8));
        } finally {
            out.close();
        }
    }

    @Test
    public void aMissingFileLoadsEmpty() {
        SessionStore store = new SessionStore(path());
        assertTrue(store.load().isEmpty());
    }

    @Test
    public void saveThenLoadRoundTrips() throws IOException {
        File file = path();
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("a", 1_000)));

        assertEquals(Collections.singletonList(record("a", 1_000)), store.load());
    }

    @Test
    public void the_start_reason_round_trips() throws IOException {
        SessionRecord withReason = record("a", 1_000);
        withReason.startReason = "boot";
        SessionStore store = new SessionStore(path());
        store.save(Collections.singletonList(withReason));
        assertEquals("boot", store.load().get(0).startReason);
    }

    @Test
    public void an_old_eighteen_field_line_loads_with_an_empty_start_reason() throws IOException {
        SessionRecord withReason = record("a", 1_000);
        withReason.startReason = "system restart";
        SessionStore store = new SessionStore(path());
        store.save(Collections.singletonList(withReason));

        String text = new String(Files.readAllBytes(path().toPath()), StandardCharsets.UTF_8);
        int tab = text.lastIndexOf('\t');
        int newline = text.indexOf('\n', tab);
        String old = text.substring(0, tab) + text.substring(newline);
        Files.write(path().toPath(), old.getBytes(StandardCharsets.UTF_8));

        List<SessionRecord> loaded = store.load();
        assertEquals(1, loaded.size());
        assertEquals("", loaded.get(0).startReason);
        assertEquals("a", loaded.get(0).id);
    }

    @Test
    public void tabsNewlinesAndUrlMetacharactersSurviveTheTsv() throws IOException {
        SessionRecord weird = record("id\tone", 1_000);
        weird.role = "join\ner";
        weird.peerEndpoint = "10.1.0.2:9797\tx + y";
        weird.endReason = "stop\nped % 100 & done";
        weird.reconnects.get(0).reason = "no\tdata\nfor 2 s + 1% & a=b";
        weird.reconnects.add(new SessionRecord.Reconnect());

        SessionStore store = new SessionStore(path());
        store.save(Collections.singletonList(weird));

        assertEquals(Collections.singletonList(weird), store.load());
    }

    @Test
    public void emptyStringsAndNoReconnectsSurviveTheRoundTrip() throws IOException {
        SessionRecord minimal = new SessionRecord();
        minimal.id = "minimal";
        minimal.role = "";
        minimal.startedAtMs = 5;
        minimal.peerEndpoint = "";
        minimal.endReason = "";
        minimal.reconnects = new ArrayList<>();

        SessionStore store = new SessionStore(path());
        store.save(Collections.singletonList(minimal));

        assertEquals(Collections.singletonList(minimal), store.load());
    }

    @Test
    public void loadSortsAFileWhoseLinesAreOutOfOrder() throws IOException {
        File oldFile = new File(folder.getRoot(), "old.tsv");
        File newFile = new File(folder.getRoot(), "new.tsv");
        new SessionStore(oldFile).save(Collections.singletonList(record("old", 1_000)));
        new SessionStore(newFile).save(Collections.singletonList(record("new", 3_000)));
        File file = path();
        Files.write(file.toPath(), (readText(oldFile) + readText(newFile))
                .getBytes(StandardCharsets.UTF_8));

        assertEquals(Arrays.asList("new", "old"), ids(new SessionStore(file).load()));
    }

    @Test
    public void appendInsertsNewestFirst() throws IOException {
        SessionStore store = new SessionStore(path());
        assertTrue(store.append(record("a", 1_000)));
        assertTrue(store.append(record("b", 2_000)));

        assertEquals(Arrays.asList("b", "a"), ids(store.load()));
    }

    @Test
    public void fiftyOneSavesKeepTheNewestFifty() throws IOException {
        SessionStore store = new SessionStore(path());
        List<SessionRecord> all = new ArrayList<>();
        for (int i = 0; i < 51; i++) {
            all.add(record("id-" + i, i * 1_000L));
        }
        store.save(all);

        List<SessionRecord> kept = store.load();
        assertEquals(50, kept.size());
        assertEquals("id-50", kept.get(0).id);
        assertEquals("id-1", kept.get(49).id);
        assertFalse(ids(kept).contains("id-0"));
    }

    @Test
    public void appendStopsAtFifty() throws IOException {
        SessionStore store = new SessionStore(path());
        for (int i = 0; i < 51; i++) {
            assertTrue(store.append(record("id-" + i, i * 1_000L)));
        }

        List<SessionRecord> kept = store.load();
        assertEquals(50, kept.size());
        assertEquals("id-50", kept.get(0).id);
        assertEquals("id-1", kept.get(49).id);
    }

    @Test
    public void appendWithAFullHistoryDropsTheOlderAppendedRecord() throws IOException {
        SessionStore store = new SessionStore(path());
        List<SessionRecord> full = new ArrayList<>();
        for (int i = 0; i < 50; i++) {
            full.add(record("id-" + i, i * 1_000L));
        }
        store.save(full);

        assertTrue(store.append(record("older", -1_000)));

        List<SessionRecord> kept = store.load();
        assertEquals(50, kept.size());
        assertEquals("id-49", kept.get(0).id);
        assertEquals("id-0", kept.get(49).id);
        assertTrue(ids(kept).contains("id-0"));
        assertFalse(ids(kept).contains("older"));
    }

    @Test
    public void aMalformedLineIsSkippedAndGoodLinesSurvive() throws IOException {
        File file = path();
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("good", 1_000)));
        appendRaw(file, "not-a-record\n");
        appendRaw(file, "a\tb\tc\n");
        appendRaw(file, "\n");

        List<SessionRecord> loaded = store.load();
        assertEquals(1, loaded.size());
        assertEquals("good", loaded.get(0).id);
    }

    @Test
    public void aCorruptFileLoadsEmptyWithoutThrowing() throws IOException {
        File file = path();
        Files.write(file.toPath(), "not\ta\trecord\n".getBytes(StandardCharsets.UTF_8));

        assertTrue(new SessionStore(file).load().isEmpty());
    }

    @Test
    public void strictLoadAcceptsMissingAndEmptyFilesButRejectsMalformedOnes()
            throws IOException {
        assertTrue(new SessionStore(new File(folder.getRoot(), "missing.tsv"))
                .loadStrict().isEmpty());

        File empty = new File(folder.getRoot(), "empty.tsv");
        Files.write(empty.toPath(), new byte[0]);
        assertTrue(new SessionStore(empty).loadStrict().isEmpty());

        File malformed = new File(folder.getRoot(), "malformed.tsv");
        Files.write(malformed.toPath(), "not-a-record\n".getBytes(StandardCharsets.UTF_8));
        try {
            new SessionStore(malformed).loadStrict();
            fail("a malformed existing file must be reported");
        } catch (IOException expected) {
            // Expected: append must not overwrite this history.
        }
    }

    @Test
    public void appendRefusesMalformedHistoryWithoutChangingIt() throws IOException {
        File file = path();
        Files.write(file.toPath(), "not-a-record\n".getBytes(StandardCharsets.UTF_8));
        byte[] before = Files.readAllBytes(file.toPath());

        assertFalse(new SessionStore(file).append(record("new", 5_000)));

        assertArrayEquals(before, Files.readAllBytes(file.toPath()));
    }

    @Test
    public void appendRefusesUndecodableHistoryWithoutChangingIt() throws IOException {
        File file = path();
        byte[] undecodable = new byte[] {(byte) 0xC3, (byte) 0x28};
        Files.write(file.toPath(), undecodable);
        byte[] before = Files.readAllBytes(file.toPath());

        assertFalse(new SessionStore(file).append(record("new", 5_000)));

        assertArrayEquals(before, Files.readAllBytes(file.toPath()));
    }

    @Test
    public void appendRefusesUnreadableHistoryWithoutChangingIt() throws IOException {
        Assume.assumeTrue(FileSystems.getDefault().supportedFileAttributeViews()
                .contains("posix"));
        Assume.assumeTrue(!"root".equals(System.getProperty("user.name")));
        File file = path();
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("kept", 1_000)));
        byte[] before = Files.readAllBytes(file.toPath());
        Files.setPosixFilePermissions(file.toPath(), PosixFilePermissions.fromString("---------"));
        try {
            assertFalse(store.append(record("new", 2_000)));
        } finally {
            Files.setPosixFilePermissions(file.toPath(),
                    PosixFilePermissions.fromString("rw-------"));
        }

        assertArrayEquals(before, Files.readAllBytes(file.toPath()));
    }

    @Test
    public void savingTwiceReplacesTheOlderFile() throws IOException {
        File file = path();
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("old", 1_000)));
        store.save(Collections.singletonList(record("new", 2_000)));

        assertEquals(Collections.singletonList("new"), ids(store.load()));
        // The temp is always renamed away.
        assertFalse(new File(folder.getRoot(), ".sessions.tsv.tmp").exists());
    }

    @Test
    public void aFailedRenameCleansUpTheTempAndLeavesTheTargetAlone() throws IOException {
        File target = path();
        assertTrue(target.mkdirs());
        File keep = new File(target, "keep");
        Files.write(keep.toPath(), "keep".getBytes(StandardCharsets.UTF_8));

        SessionStore store = new SessionStore(target);
        try {
            store.save(Collections.singletonList(record("a", 1_000)));
            fail("save over a directory must fail");
        } catch (IOException expected) {
            // Expected: the rename cannot replace a non-empty directory.
        }

        assertFalse(new File(folder.getRoot(), ".sessions.tsv.tmp").exists());
        assertTrue(target.isDirectory());
        assertTrue(keep.isFile());
    }

    @Test
    public void savedFileIsOwnerOnlyAndTheCreatedDirectoryIsPrivate() throws IOException {
        Assume.assumeTrue(FileSystems.getDefault().supportedFileAttributeViews()
                .contains("posix"));
        File file = new File(new File(folder.getRoot(), "cfg"), "sessions.tsv");
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("a", 1_000)));

        assertEquals("rw-------", PosixFilePermissions.toString(
                Files.getPosixFilePermissions(Paths.get(file.getPath()))));
        assertEquals("rwx------", PosixFilePermissions.toString(
                Files.getPosixFilePermissions(Paths.get(file.getParent()))));
    }

    @Test
    public void aSavedFileHasNoKeyShapedBase64() throws IOException {
        File file = path();
        SessionStore store = new SessionStore(file);
        store.save(Collections.singletonList(record("a", 1_000)));

        String text = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
        assertFalse("key-shaped text in " + text,
                text.matches("(?s).*[A-Za-z0-9+/]{42,44}=.*"));
    }

    private static String readText(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }

    private static List<String> ids(List<SessionRecord> records) {
        List<String> out = new ArrayList<>();
        for (SessionRecord record : records) {
            out.add(record.id);
        }
        return out;
    }
}
