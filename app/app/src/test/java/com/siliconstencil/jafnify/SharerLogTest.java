package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class SharerLogTest {
    private static UiText.Parsed status(String line, long rx, long tx) {
        return new UiText.Parsed(line, "", "", rx, tx);
    }

    @Test
    public void aNewRunLogsItsCodeAndFirstState() {
        SharerLog log = new SharerLog();
        assertTrue(log.start(1));
        assertEquals("9123", log.code("9123"));
        assertEquals("Searching...", log.status(status("Searching...", 0, 0)));
    }

    @Test
    public void theCodeIsLoggedOncePerRun() {
        SharerLog log = new SharerLog();
        log.start(1);
        assertEquals("9123", log.code("9123"));
        assertNull(log.code("9123"));
    }

    @Test
    public void resumingTheSameRunDoesNotLogTheCodeOrStateAgain() {
        SharerLog log = new SharerLog();
        log.start(1);
        log.code("9123");
        log.status(status("Connected", 10, 20));
        assertFalse(log.start(1));
        assertNull(log.code("9123"));
        assertNull(log.status(status("Connected", 10, 20)));
    }

    @Test
    public void countersAloneNeverWriteALogLine() {
        SharerLog log = new SharerLog();
        log.start(1);
        assertEquals("Connected", log.status(status("Connected", 1, 2)));
        assertNull(log.status(status("Connected", 500, 900)));
    }

    @Test
    public void aStateChangeIsOneLine() {
        SharerLog log = new SharerLog();
        log.start(1);
        assertEquals("Connecting...", log.status(status("Connecting...", 0, 0)));
        assertEquals("Connected", log.status(status("Connected", 1, 2)));
    }

    @Test
    public void anEmptyStatusIsNotAnEvent() {
        SharerLog log = new SharerLog();
        log.start(1);
        assertNull(log.status(status("", 0, 0)));
    }

    @Test
    public void anEmptyCodeIsNotLogged() {
        SharerLog log = new SharerLog();
        log.start(1);
        assertNull(log.code(""));
    }

    @Test
    public void aNewRunLogsAgain() {
        SharerLog log = new SharerLog();
        log.start(1);
        log.status(status("Connected", 1, 2));
        assertTrue(log.start(2));
        assertEquals("5678", log.code("5678"));
        assertEquals("Connected", log.status(status("Connected", 0, 0)));
    }
}
