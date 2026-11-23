package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

public class NotifTextTest {
    @Test
    public void anUnchangedStateIsNotReposted() {
        NotifText n = new NotifText();
        assertEquals("Connected", n.next("Connected"));
        assertNull(n.next("Connected"));
    }

    @Test
    public void everyTransitionIsSentOnce() {
        NotifText n = new NotifText();
        assertEquals("Connecting...", n.next("Connecting..."));
        assertEquals("Connected", n.next("Connected"));
        assertEquals("Retrying", n.next("Retrying"));
    }

    @Test
    public void aForegroundStartDoesNotSwallowTheNextState() {
        NotifText n = new NotifText();
        // The last state before the app was reopened.
        n.next("Connected");
        // onStartCommand called startForeground() again, outside the poll.
        n.shown("Starting...");
        // The poll must re-push the real state even though it did not change.
        assertEquals("Connected", n.next("Connected"));
    }

    @Test
    public void theStartTextIsNotPushedTwice() {
        NotifText n = new NotifText();
        n.shown("Starting...");
        assertNull(n.next("Starting..."));
    }
}
