package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.List;

/**
 * The notices screen holds a 3.3 MB file. It is split into small chunks so a
 * single TextView never lays out the whole file; the split must keep every
 * character, in order, and the header must stay first.
 */
public class NoticesTextTest {
    @Test
    public void anEmptyNoticeHasNoChunks() {
        assertTrue(NoticesText.sections("").isEmpty());
        assertTrue(NoticesText.sections(null).isEmpty());
    }

    @Test
    public void theChunksKeepEveryCharacterInOrder() {
        StringBuilder raw = new StringBuilder();
        for (int i = 0; i < 500; i++) {
            raw.append("crate-").append(i).append(" licence line\n");
        }
        List<String> chunks = NoticesText.sections(raw.toString());
        assertFalse(chunks.isEmpty());
        StringBuilder joined = new StringBuilder();
        for (String chunk : chunks) {
            joined.append(chunk);
            assertTrue("chunk stays small: " + chunk.length(),
                    chunk.length() <= NoticesText.MAX_CHUNK);
        }
        assertEquals(raw.toString(), joined.toString());
    }

    @Test
    public void aLongTextBecomesManyChunks() {
        StringBuilder raw = new StringBuilder();
        for (int i = 0; i < 5000; i++) {
            raw.append("0123456789abcdef\n");
        }
        List<String> chunks = NoticesText.sections(raw.toString());
        assertTrue("more than one chunk", chunks.size() > 1);
        for (String chunk : chunks) {
            assertTrue("chunk stays small: " + chunk.length(),
                    chunk.length() <= NoticesText.MAX_CHUNK);
        }
    }

    @Test
    public void theFirstChunkStillStartsWithTheHeader() {
        String raw = "Jafnify third-party notices\n===========================\n\nMIT\n";
        List<String> chunks = NoticesText.sections(raw);
        assertFalse(chunks.isEmpty());
        assertTrue(chunks.get(0).startsWith("Jafnify third-party notices"));
    }

    @Test
    public void aLineLongerThanTheLimitIsNotLost() {
        String raw = "short\n" + repeat('x', NoticesText.MAX_CHUNK * 2) + "\nend\n";
        List<String> chunks = NoticesText.sections(raw);
        StringBuilder joined = new StringBuilder();
        for (String chunk : chunks) {
            joined.append(chunk);
        }
        assertEquals(raw, joined.toString());
    }

    private static String repeat(char c, int n) {
        char[] out = new char[n];
        for (int i = 0; i < n; i++) {
            out[i] = c;
        }
        return new String(out);
    }
}
