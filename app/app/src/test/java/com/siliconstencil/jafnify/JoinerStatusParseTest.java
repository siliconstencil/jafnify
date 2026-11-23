package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/**
 * The native status has the same six-line shape as the sharer's: status,
 * peer id, peer name, rx, tx, endpoint. Five-line input from an older
 * library still parses, and a broken status can never throw.
 */
public class JoinerStatusParseTest {
    // A realistic 32-byte base64 public key. It is a fixture, not a secret.
    private static final String PUBLIC_KEY =
            "MTIzNDU2Nzg5MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTI=";

    @Test
    public void sixLinesParseIntoStatusPeerAndCounters() {
        UiText.Parsed p = UiText.parse(
                "Connected\n" + PUBLIC_KEY + "\nLaptop - Blue Falcon\n2048\n4096\n10.1.0.2:9797");

        assertEquals("Connected", p.line);
        assertEquals(PUBLIC_KEY, p.peerId);
        assertEquals("Laptop - Blue Falcon", p.peerName);
        assertEquals(2048L, p.rx);
        assertEquals(4096L, p.tx);
        assertEquals("10.1.0.2:9797", p.endpoint);
    }

    @Test
    public void fiveLinesStillParseWithAnEmptyEndpoint() {
        UiText.Parsed p = UiText.parse(
                "Searching...\n" + PUBLIC_KEY + "\nLaptop - Blue Falcon\n2048\n4096");

        assertEquals("Searching...", p.line);
        assertEquals(PUBLIC_KEY, p.peerId);
        assertEquals("Laptop - Blue Falcon", p.peerName);
        assertEquals(2048L, p.rx);
        assertEquals(4096L, p.tx);
        assertEquals("", p.endpoint);
    }

    @Test
    public void malformedOrShortStatusNeverThrows() {
        UiText.Parsed shortStatus = UiText.parse("Connected\n" + PUBLIC_KEY);
        assertEquals("Connected", shortStatus.line);
        assertEquals(PUBLIC_KEY, shortStatus.peerId);
        assertEquals("", shortStatus.peerName);
        assertEquals(0L, shortStatus.rx);
        assertEquals(0L, shortStatus.tx);
        assertEquals("", shortStatus.endpoint);

        UiText.Parsed junk = UiText.parse("Retrying (10s) - no data\n\n\nx\ny\n");
        assertEquals("Retrying (10s) - no data", junk.line);
        assertEquals(0L, junk.rx);
        assertEquals(0L, junk.tx);
        assertEquals("", junk.endpoint);

        UiText.Parsed none = UiText.parse(null);
        assertEquals("", none.line);
        assertEquals("", none.peerId);
        assertEquals("", none.peerName);
        assertEquals(0L, none.rx);
        assertEquals(0L, none.tx);
        assertEquals("", none.endpoint);
    }
}
