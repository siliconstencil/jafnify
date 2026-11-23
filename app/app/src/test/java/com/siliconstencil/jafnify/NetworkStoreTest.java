package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import java.io.File;
import java.util.List;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class NetworkStoreTest {
    @Rule
    public TemporaryFolder folder = new TemporaryFolder();

    private UsageLineStore store() throws Exception {
        return new UsageLineStore(new File(folder.getRoot(), "phone-stats.tsv"));
    }

    @Test
    public void everyFamilyKeepsItsOwnLines() throws Exception {
        UsageLineStore store = store();
        store.upsert("JFU1 1 hour=3600000 up=10 down=20");
        store.upsertNetwork("JFN1 1 hour=3600000 switches=1 gen=5g:60000");
        store.upsertSignal("JSG1 1 at=5 level=3");
        store.upsertNetwork("JFN1 1 hour=7200000 switches=0 gen=none");
        store.upsertSignal("JSG1 1 at=9 level=2");

        List<String> usage = store.load();
        assertEquals(1, usage.size());
        assertEquals("JFU1 1 hour=3600000 up=10 down=20", usage.get(0));

        List<String> network = store.loadNetwork();
        assertEquals(2, network.size());
        assertEquals("JFN1 1 hour=3600000 switches=1 gen=5g:60000", network.get(0));

        List<String> signal = store.loadSignal();
        assertEquals(2, signal.size());
        assertEquals("JSG1 1 at=5 level=3", signal.get(0));
    }

    @Test
    public void aNetworkLineNeverReplacesTheUsageLineOfTheSameHour() throws Exception {
        UsageLineStore store = store();
        store.upsert("JFU1 1 hour=3600000 up=10 down=20");
        store.upsertNetwork("JFN1 1 hour=3600000 switches=1 gen=5g:60000");
        store.upsertNetwork("JFN1 1 hour=3600000 switches=2 gen=4g:60000");

        assertEquals(1, store.load().size());
        assertEquals(1, store.loadNetwork().size());
        assertEquals("JFN1 1 hour=3600000 switches=2 gen=4g:60000",
                store.loadNetwork().get(0));
    }

    @Test
    public void theSignalLinesAreCappedAtOneDay() throws Exception {
        UsageLineStore store = store();
        for (int i = 0; i < 300; i++) {
            store.upsertSignal("JSG1 1 at=" + i + " level=3");
        }

        List<String> signal = store.loadSignal();
        assertEquals(288, signal.size());
        assertEquals("JSG1 1 at=12 level=3", signal.get(0));
    }
}
