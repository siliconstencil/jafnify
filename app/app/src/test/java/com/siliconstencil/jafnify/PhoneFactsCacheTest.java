package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The phone's own facts (charging, battery saver) change rarely, but the
 * system answers six binder calls for them. The screen asks for them once in
 * a while, not on every poll.
 */
public class PhoneFactsCacheTest {

    @Test
    public void aNewCacheHasNoFactsYet() {
        PhoneFactsCache cache = new PhoneFactsCache();
        assertTrue(cache.stale(1_000_000L));
    }

    @Test
    public void theFactsAreFreshRightAfterTheyWereAskedFor() {
        PhoneFactsCache cache = new PhoneFactsCache();
        cache.put("charging=true", 1_000_000L);
        assertFalse(cache.stale(1_000_000L));
        assertFalse(cache.stale(1_004_999L));
        assertEquals("charging=true", cache.get());
    }

    @Test
    public void theFactsAreAskedForAgainAfterTheAge() {
        PhoneFactsCache cache = new PhoneFactsCache();
        cache.put("charging=true", 1_000_000L);
        assertTrue(cache.stale(1_000_000L + PhoneFactsCache.MAX_AGE_MS));
    }
}
