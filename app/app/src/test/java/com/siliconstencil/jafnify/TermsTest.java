package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The use-at-your-own-risk acceptance. */
public class TermsTest {
    @Test
    public void nothingStoredMeansNotAccepted() {
        // SharedPreferences returns 0 for a missing int.
        assertFalse(Terms.accepted(0));
    }

    @Test
    public void the_current_version_is_accepted() {
        assertTrue(Terms.accepted(Terms.VERSION));
    }

    @Test
    public void a_later_version_still_counts_as_accepted() {
        assertTrue(Terms.accepted(Terms.VERSION + 1));
    }

    @Test
    public void the_stored_key_is_stable() {
        assertEquals("terms_version", Terms.PREF_KEY);
    }
}
