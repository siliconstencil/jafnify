package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.TimeZone;

public class UiTextTest {
    // ---- native status text -> Status -----------------------------------

    @Test
    public void statusMapsEveryNativeStatusLine() {
        // These are the core's snapshot::Snapshot::status_line words.
        assertEquals(UiText.Status.IDLE, UiText.status("Not sharing"));
        assertEquals(UiText.Status.SEARCHING,
                UiText.status("Searching for the other device"));
        assertEquals(UiText.Status.PAIRING, UiText.status("Pairing"));
        assertEquals(UiText.Status.CONNECTING, UiText.status("Connecting"));
        assertEquals(UiText.Status.CONNECTED, UiText.status("Connected"));
        assertEquals(UiText.Status.RETRYING,
                UiText.status("Connection lost - retrying in 7 s - No data from peer for 35 s"));
    }

    @Test
    public void emptyOrMissingStatusMeansTheSearchHasNotReportedYet() {
        assertEquals(UiText.Status.SEARCHING, UiText.status(""));
        assertEquals(UiText.Status.SEARCHING, UiText.status(null));
    }

    @Test
    public void anUnknownStatusLineIsUnknown() {
        assertEquals(UiText.Status.UNKNOWN, UiText.status("Something new"));
    }

    @Test
    public void retryReasonIsTheTextAfterTheRetryPrefix() {
        assertEquals("No data from peer for 35 s",
                UiText.retryReason("Retrying (10s) - No data from peer for 35 s"));
        assertEquals("No data from peer for 35 s",
                UiText.retryReason(
                        "Connection lost - retrying in 7 s - No data from peer for 35 s"));
        assertEquals("", UiText.retryReason("Connection lost - retrying in 7 s"));
        assertEquals("", UiText.retryReason("Connected"));
        assertEquals("", UiText.retryReason(null));
    }

    // ---- sharerStatus() text -> fields ------------------------------------

    @Test
    public void parseSplitsTheFiveStatusLines() {
        UiText.Parsed p = UiText.parse("Connected\npeerid\nLaptop - Blue Falcon\n2048\n4096");
        assertEquals("Connected", p.line);
        assertEquals("peerid", p.peerId);
        assertEquals("Laptop - Blue Falcon", p.peerName);
        assertEquals(2048L, p.rx);
        assertEquals(4096L, p.tx);
    }

    @Test
    public void parseReadsTheSixthEndpointLine() {
        UiText.Parsed p = UiText.parse(
                "Connected\npeerid\nLaptop - Blue Falcon\n2048\n4096\n10.1.0.2:9797");
        assertEquals("Connected", p.line);
        assertEquals("peerid", p.peerId);
        assertEquals("Laptop - Blue Falcon", p.peerName);
        assertEquals(2048L, p.rx);
        assertEquals(4096L, p.tx);
        assertEquals("10.1.0.2:9797", p.endpoint);
    }

    @Test
    public void fiveLineInputsStillParseWithAnEmptyEndpoint() {
        UiText.Parsed p = UiText.parse("Connected\npeerid\nLaptop - Blue Falcon\n2048\n4096");
        assertEquals("Connected", p.line);
        assertEquals("peerid", p.peerId);
        assertEquals("Laptop - Blue Falcon", p.peerName);
        assertEquals(2048L, p.rx);
        assertEquals(4096L, p.tx);
        assertEquals("", p.endpoint);
    }

    @Test
    public void aMissingEndpointLineIsEmpty() {
        UiText.Parsed p = UiText.parse("Connected\npeerid\nname\n1\n2\n");
        assertEquals("", p.endpoint);

        UiText.Parsed none = UiText.parse(null);
        assertEquals("", none.endpoint);
    }

    @Test
    public void parseKeepsEmptyMiddleLinesAndSurvivesShortOrNullInput() {
        UiText.Parsed p = UiText.parse("Searching...\n\n\n0\n0");
        assertEquals("Searching...", p.line);
        assertEquals("", p.peerId);
        assertEquals("", p.peerName);
        assertEquals(0L, p.rx);

        UiText.Parsed none = UiText.parse(null);
        assertEquals("", none.line);
        assertEquals(0L, none.tx);

        UiText.Parsed junk = UiText.parse("Connected\nid\nname\nx\ny");
        assertEquals(0L, junk.rx);
        assertEquals(0L, junk.tx);
    }

    // ---- problems -----------------------------------------------------------

    @Test
    public void portInUseIsRecognisedWherePortBusyIsMeant() {
        assertEquals(UiText.Problem.PORT_BUSY,
                UiText.problem("port in use: UDP 9797 (Address already in use)"));
        assertEquals(UiText.Problem.PORT_BUSY,
                UiText.problem("Retrying (10s) - Tunnel error: port in use: UDP 9797 (x)"));
    }

    @Test
    public void socketAndBindFailuresMeanNoNetwork() {
        assertEquals(UiText.Problem.NO_NETWORK,
                UiText.problem("discovery bind failed: Network is unreachable"));
        assertEquals(UiText.Problem.NO_NETWORK, UiText.problem("handshake bind failed: x"));
        assertEquals(UiText.Problem.NO_NETWORK, UiText.problem("known-peer socket error: x"));
    }

    @Test
    public void otherTextIsOtherAndNothingIsNone() {
        assertEquals(UiText.Problem.OTHER, UiText.problem("could not create native instance"));
        assertEquals(UiText.Problem.NONE, UiText.problem(""));
        assertEquals(UiText.Problem.NONE, UiText.problem(null));
    }

    @Test
    public void noNetworkWarnsOnlyWhenThereIsNoUplinkAtAll() {
        assertTrue(UiText.noNetwork(false));
        assertFalse(UiText.noNetwork(true));
    }

    @Test
    public void theConnectionDetailLinesKeepTheirText() {
        String[] lines = UiText.connectionLines(
                "Path: Direct over hotspot\nMTU: 1280\n\nDNS: through the tunnel");
        assertEquals(3, lines.length);
        assertEquals("Path: Direct over hotspot", lines[0]);
        assertEquals("MTU: 1280", lines[1]);
        assertEquals("DNS: through the tunnel", lines[2]);
        assertEquals(0, UiText.connectionLines(null).length);
    }

    @Test
    public void notificationsAreAskedOnlyOnceAndTheNoteShowsAfterThat() {
        assertTrue(UiText.shouldAskNotifications(true, false, false));
        assertFalse(UiText.shouldAskNotifications(true, false, true));
        assertFalse(UiText.shouldAskNotifications(true, true, false));
        assertFalse(UiText.shouldAskNotifications(false, false, false));
        assertTrue(UiText.notificationsOffNote(true, false, true));
        assertFalse(UiText.notificationsOffNote(true, true, true));
        assertFalse(UiText.notificationsOffNote(true, false, false));
    }

    @Test
    public void backCollapsesDetailsOnlyWhenTheyAreExpanded() {
        assertTrue(UiText.backCollapsesDetails(true));
        assertFalse(UiText.backCollapsesDetails(false));
    }

    // ---- formatting ---------------------------------------------------------

    @Test
    public void codeDisplayKeepsTheDigitsOnOneUnspacedRun() {
        // The spacing is the view's letterSpacing, not literal spaces.
        assertEquals("4321", UiText.codeDisplay("4321"));
        assertEquals("", UiText.codeDisplay(""));
        assertEquals("", UiText.codeDisplay(null));
    }

    @Test
    public void theCodeFitsOnOneLineAt320dpAnd200Percent() {
        int size = UiText.codeTextSizeSp(320f, 2f);
        // 4 glyphs at 0.6 em plus 4 letter gaps at 0.2 em is 3.2 em.
        assertTrue(3.2f * size * 2f <= 320f);
        assertEquals(48, size);
    }

    @Test
    public void theCodeCapsAt48spAt320dpAnd100Percent() {
        assertEquals(48, UiText.codeTextSizeSp(320f, 1f));
    }

    @Test
    public void aVeryNarrowWidthFallsBackToThe20spFloor() {
        assertEquals(20, UiText.codeTextSizeSp(40f, 1f));
    }

    @Test
    public void elapsedIsMinutesSecondsThenHours() {
        assertEquals("0:00", UiText.formatElapsed(0));
        assertEquals("0:59", UiText.formatElapsed(59));
        assertEquals("12:34", UiText.formatElapsed(12 * 60 + 34));
        assertEquals("1:02:03", UiText.formatElapsed(3600 + 2 * 60 + 3));
        assertEquals("0:00", UiText.formatElapsed(-5));
    }

    @Test
    public void bytesUseB_KB_MB() {
        assertEquals("0 B", UiText.formatBytes(0));
        assertEquals("1023 B", UiText.formatBytes(1023));
        assertEquals("1 KB", UiText.formatBytes(1024));
        assertEquals("340 KB", UiText.formatBytes(340 * 1024));
        assertEquals("1.5 MB", UiText.formatBytes(1536 * 1024));
    }

    @Test
    public void phoneTrafficSeparatesTheTwoDirectionsWithASeparator() {
        // rx = what the laptop sent (from laptop), tx = what the phone sent
        // (to laptop). The words must not be the laptop's Up/Down.
        assertEquals("To laptop 1.5 MB | From laptop 340 KB",
                UiText.phoneTraffic(340 * 1024, 1536 * 1024));
    }

    @Test
    public void theLinkPathLineIsNotCurrentWhenStoppedOrWaiting() {
        assertEquals("Direct over hotspot",
                UiText.linkLine("Direct over hotspot", true, false));
        assertEquals("", UiText.linkLine("Direct over hotspot", true, true));
        assertEquals("", UiText.linkLine("Direct over hotspot", false, false));
        assertEquals("", UiText.linkLine(null, true, false));
    }

    @Test
    public void theWaitingSentenceWinsInTheNotification() {
        String waiting = "Hotspot is off. Turn it on to keep sharing.";
        assertEquals(waiting, UiText.notificationText(true, waiting, true,
                "Laptop - Connected", "Connected", "Starting..."));
        assertEquals("Laptop - Connected", UiText.notificationText(false, waiting,
                true, "Laptop - Connected", "Connected", "Starting..."));
        assertEquals("Connected", UiText.notificationText(false, waiting,
                false, "", "Connected", "Starting..."));
        assertEquals("Starting...", UiText.notificationText(false, waiting,
                false, "", "", "Starting..."));
        assertEquals("", UiText.notificationText(false, null, false, "", null, null));
    }

    @Test
    public void liveTextOnlyChangesWhenTheWordsChange() {
        assertTrue(UiText.textChanged(null, "Connected"));
        assertTrue(UiText.textChanged("Connected", "Connecting"));
        assertTrue(UiText.textChanged(null, ""));
        assertFalse(UiText.textChanged("Connected", "Connected"));
        assertFalse(UiText.textChanged("", ""));
    }

    @Test
    public void theLiveRegionKeyIgnoresTheMovingSeconds() {
        String first = UiText.announceKey(UiText.Status.CONNECTED,
                "Direct over hotspot", "Laptop");
        String next = UiText.announceKey(UiText.Status.CONNECTED,
                "Direct over hotspot", "Laptop");
        assertEquals("the verdict no longer carries seconds", first, next);
        // An older native that still returns the combined line is normalized.
        assertEquals("a legacy combined line is still stable", first,
                UiText.announceKey(UiText.Status.CONNECTED,
                        "Last handshake 13 s ago - Direct over hotspot", "Laptop"));

        assertNotEquals("a new verdict speaks",
                first,
                UiText.announceKey(UiText.Status.CONNECTED, "Not reachable", "Laptop"));
        assertNotEquals("a new state speaks",
                first,
                UiText.announceKey(UiText.Status.RETRYING, "Direct over hotspot", "Laptop"));
        assertNotEquals("a new peer speaks",
                first,
                UiText.announceKey(UiText.Status.CONNECTED, "Direct over hotspot", "Tablet"));

        String retry = UiText.announceKey(UiText.Status.RETRYING,
                "", "Laptop");
        assertEquals("the retry countdown is not announced every second",
                retry, UiText.announceKey(UiText.Status.RETRYING, "", "Laptop"));
    }

    @Test
    public void theHealthVerdictIsTheVerdictNotTheAge() {
        assertEquals("Direct over hotspot",
                UiText.healthVerdict("Direct over hotspot"));
        assertEquals("Not reachable", UiText.healthVerdict("Not reachable"));
        assertEquals("Waiting for the first handshake",
                UiText.healthVerdict("Waiting for the first handshake"));
        // Legacy combined lines keep their old behavior.
        assertEquals("Direct over hotspot", UiText.healthVerdict(
                "Last handshake 12 s ago - Direct over hotspot"));
        assertEquals("Not reachable",
                UiText.healthVerdict("Last handshake 181 s ago - Not reachable"));
        assertEquals("", UiText.healthVerdict(null));
    }

    @Test
    public void thePeerNameIsAddedToTheCoreWordingNotInsteadOfIt() {
        assertEquals("Connected - Laptop", UiText.withPeer("Connected", "Laptop"));
        assertEquals("Not sharing - Laptop", UiText.withPeer("Not sharing", "Laptop"));
        assertEquals("Connected", UiText.withPeer("Connected", ""));
        assertEquals("Connected", UiText.withPeer("Connected", null));
        assertEquals("", UiText.withPeer(null, null));
    }

    @Test
    public void keyPrefixIsTheFirstTwelveCharacters() {
        assertEquals("AbCdEfGhIjKl", UiText.keyPrefix("AbCdEfGhIjKlMnOp="));
        assertEquals("short", UiText.keyPrefix("short"));
        assertEquals("", UiText.keyPrefix(null));
    }

    @Test
    public void formatWhenIsDateAndTimeInEnglish() {
        TimeZone utc = TimeZone.getTimeZone("UTC");
        // 2026-10-23 14:05:00 UTC
        assertEquals("Oct 23, 14:05", UiText.formatWhen(1792764300000L, utc));
        assertTrue(UiText.formatWhen(0L, utc).startsWith("Jan 1"));
    }

    @Test
    public void handshakeAgeDropsTheLeadForTheDetailsRow() {
        assertEquals("30 s ago", UiText.handshakeAge("Last handshake 30 s ago"));
        assertEquals("none yet", UiText.handshakeAge("none yet"));
        assertEquals("", UiText.handshakeAge(null));
    }

    @Test
    public void parseReadsTheSeventhSessionLineAndKeepsTheEndpointClean() {
        UiText.Parsed p = UiText.parse(
                "Connected\npeerid\nLaptop\n2048\n4096\n10.1.0.2:9797\n3");
        assertEquals("10.1.0.2:9797", p.endpoint);
        assertEquals(3L, p.session);
        assertEquals(0L, UiText.parse(
                "Connected\npeerid\nLaptop\n2048\n4096\n10.1.0.2:9797").session);
    }

    // ---- the native connected clock and the neutral flag ------------------

    @Test
    public void parseReadsTheNativeClockAndTheNeutralFlag() {
        UiText.Parsed p = UiText.parse(
                "Connected\npeerid\nLaptop\n2048\n4096\n10.1.0.2:9797\n3\n12500\n1");
        assertEquals(3L, p.session);
        assertEquals(12500L, p.connectedMs);
        assertTrue(p.neutral);
        assertEquals("Reconnecting...", UiText.displayLine(p));
    }

    @Test
    public void anOldStatusHasNoNativeClockAndReadsAsItsOwnLine() {
        UiText.Parsed p = UiText.parse(
                "Connected\npeerid\nLaptop\n2048\n4096\n10.1.0.2:9797\n3");
        assertEquals(0L, p.connectedMs);
        assertFalse(p.neutral);
        assertEquals("Connected", UiText.displayLine(p));
        assertEquals(UiText.Status.CONNECTED, UiText.displayStatus(p));
    }

    @Test
    public void aNeutralStatusIsNotConnectedForTheDisplay() {
        UiText.Parsed p = UiText.parse(
                "Connected\npeerid\nLaptop\n2048\n4096\n10.1.0.2:9797\n3\n0\n1");
        assertEquals(UiText.Status.CONNECTING, UiText.displayStatus(p));
        assertEquals("Reconnecting...", UiText.displayLine(p));
    }

    @Test
    public void theConnectedClockIsAnchoredToTheNativeElapsed() {
        assertEquals(15_000L, UiText.connectedSince(20_000L, 5_000L, 0L));
        assertEquals(20_000L, UiText.connectedSince(20_000L, 0L, 0L));
        assertEquals(3_000L, UiText.connectedSince(20_000L, 0L, 3_000L));
        assertEquals(0L, UiText.connectedSince(20_000L, 25_000L, 0L));
    }
}
