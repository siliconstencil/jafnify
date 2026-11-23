package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.util.List;

/** The tile catalog line parser: id and label, malformed lines skipped. */
public class HomeLayoutPrefsTest {
    @Test
    public void the_catalog_parses_id_and_label() {
        List<HomeLayoutPrefs.Option> options = HomeLayoutPrefs.parseCatalog(
                "signal\tSIGNAL\ndns\tDNS\navg\tAVERAGE\n");
        assertEquals(3, options.size());
        assertEquals("signal", options.get(0).id);
        assertEquals("SIGNAL", options.get(0).label);
        assertEquals("avg", options.get(2).id);
    }

    @Test
    public void a_malformed_line_is_skipped() {
        List<HomeLayoutPrefs.Option> options = HomeLayoutPrefs.parseCatalog(
                "too\tmany\tfields\n\tNO ID\nnoid\nheat\tHEAT\n");
        assertEquals(1, options.size());
        assertEquals("heat", options.get(0).id);
    }

    @Test
    public void empty_input_has_no_options() {
        assertEquals(0, HomeLayoutPrefs.parseCatalog(null).size());
        assertEquals(0, HomeLayoutPrefs.parseCatalog("").size());
    }
}
