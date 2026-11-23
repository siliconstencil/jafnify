package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The best-spot helpers: the dBm pick, the sample and mark lines the core
 * reads and the split of the core's block. Pure JVM: no device, no JNI.
 */
public class BestSpotTest {
    // The modem reports CellInfo.UNAVAILABLE (and CellSignalStrength.UNAVAILABLE)
    // for a strength it does not know, and that value is Integer.MAX_VALUE. The
    // mockable android.jar of the unit tests carries no real field values, so
    // the number is spelled out here.
    @Test
    public void theDbmPickSkipsUnavailableEntries() {
        assertEquals("the modem's unknown value is the Android one",
                Integer.MAX_VALUE, BestSpot.UNAVAILABLE);
        assertEquals("the first valid entry wins",
                -95, BestSpot.pickDbm(new int[] {
                        Integer.MAX_VALUE, Integer.MAX_VALUE, -95, -90}));
        assertEquals("a single valid entry is the value",
                -95, BestSpot.pickDbm(new int[] {-95}));
        assertEquals("no valid entry means no dBm",
                -1, BestSpot.pickDbm(new int[] {Integer.MAX_VALUE}));
        assertEquals("no list means no dBm", -1, BestSpot.pickDbm(null));
    }

    @Test
    public void aSampleLineCarriesTheFieldsTheCoreReads() {
        assertEquals("1000,5g-adv,3,-95,40,",
                BestSpot.sampleLine(1000, "5g-adv", 3, -95, 40));
        assertEquals("an unknown dBm and delay leave the fields empty",
                "1000,wifi,2,,,", BestSpot.sampleLine(1000, "wifi", 2, -1, -1));
    }

    @Test
    public void aMarkLineCarriesTheTimeAndTheNumber() {
        assertEquals("2000,1", BestSpot.markLine(2000, 1));
    }

    @Test
    public void theSessionKeepsTheSamplesAndTheMarksInMemory() {
        BestSpot session = new BestSpot(120_000);
        session.addSample("1,5g,3,-95,40,", 1);
        assertEquals("a stopped session collects nothing", "", session.samplesBlob());
        session.start(1);
        assertTrue(session.running());
        session.addSample("1,5g,3,-95,40,", 2);
        assertEquals("1,5g,3,-95,40,", session.samplesBlob());
        assertEquals("the first mark is number one", 1, session.mark(2000));
        assertEquals("2000,1", session.marksBlob());
        assertEquals("the marks are numbered in order", 2, session.mark(3000));
        assertEquals("2000,1\n3000,2", session.marksBlob());
        session.stop();
        assertFalse(session.running());
        assertEquals("the samples stay for the last lines", "1,5g,3,-95,40,",
                session.samplesBlob());
    }

    // F1: a session lasts two minutes. The length comes from the core, so the
    // test passes it in; the phone never decides a length of its own.
    @Test
    public void theSessionEndsAfterItsLength() {
        BestSpot session = new BestSpot(120_000);
        session.start(1_000);
        session.addSample("1000,5g,3,-95,40,", 2_000);
        assertEquals("the first mark is number one", 1, session.mark(3_000));
        assertFalse("the session runs inside its length",
                session.expired(120_999));
        assertTrue("the session is over after its length",
                session.expired(121_000));
        assertEquals("a mark after the end is not accepted",
                0, session.mark(121_000));
        session.addSample("121000,5g,3,-95,40,", 121_000);
        assertEquals("the samples stop at the end too", "1000,5g,3,-95,40,",
                session.samplesBlob());
    }

    @Test
    public void theUserCanStopTheSessionEarly() {
        BestSpot session = new BestSpot(120_000);
        session.start(1_000);
        session.addSample("1000,5g,3,-95,40,", 2_000);
        session.stop();
        assertFalse("a stopped session is not running", session.running());
        assertEquals("a stopped session takes no mark", 0, session.mark(3_000));
        session.addSample("3000,5g,3,-95,40,", 3_000);
        assertEquals("a stopped session takes no sample", "1000,5g,3,-95,40,",
                session.samplesBlob());
        assertEquals("the samples stay for the last lines", "1000,5g,3,-95,40,",
                session.samplesBlob());
    }

    // F4: a rotation must not lose the session. The bundle carries the state.
    @Test
    public void theSessionStateSurvivesTheBundle() {
        BestSpot session = new BestSpot(120_000);
        session.start(1_000);
        session.addSample("1000,5g,3,-95,40,", 2_000);
        session.mark(3_000);
        String[] state = session.saveState();

        BestSpot restored = new BestSpot(120_000);
        restored.restoreState(state);
        assertTrue("the session runs on after the rotation", restored.running());
        assertEquals("the samples come back", "1000,5g,3,-95,40,",
                restored.samplesBlob());
        assertEquals("the marks come back", "3000,1", restored.marksBlob());
        assertFalse("the length still runs from the old start",
                restored.expired(120_999));
        assertTrue("the length still runs from the old start",
                restored.expired(121_000));
        assertEquals("a mark after the end is still refused", 0,
                restored.mark(121_500));
    }

    @Test
    public void aBrokenStateLeavesTheSessionOff() {
        BestSpot session = new BestSpot(120_000);
        session.restoreState(null);
        assertFalse("no state means no session", session.running());
        session.restoreState(new String[] {"1,5g,3,-95,40,", "2000,1"});
        assertFalse("a half state means no session", session.running());
        session.restoreState(new String[] {"", "", "1", "not a number"});
        assertFalse("a broken start means no session", session.running());
    }

    @Test
    public void theCoreBlockSplitsIntoTheLiveLineTheSpotsAndTheVerdict() {
        String[] parts = BestSpot.splitBlock(
                "Now: 5G, -95 dBm\nSpot 1: 4G\nSpot 2: 5G\nBest so far: Spot 2");
        assertEquals("Now: 5G, -95 dBm", parts[0]);
        assertEquals("Spot 1: 4G\nSpot 2: 5G", parts[1]);
        assertEquals("Best so far: Spot 2", parts[2]);
        String[] one = BestSpot.splitBlock("Mark another spot to compare");
        assertEquals("Mark another spot to compare", one[0]);
        assertEquals("", one[1]);
        assertEquals("", one[2]);
        String[] none = BestSpot.splitBlock("");
        assertEquals("", none[0]);
        assertEquals("", none[1]);
        assertEquals("", none[2]);
    }

    @Test
    public void theDelayAnswerTimeComesFromTheSummaryLine() {
        assertEquals("the core line carries the delay",
                40, BestSpot.rttFromSummary("JFS1 1 summary v=1 net=5g sig=3 rtt=40 up=0"));
        assertEquals("no delay means no answer time",
                -1, BestSpot.rttFromSummary("JFS1 1 summary v=1 net=5g sig=3 rtt=0 up=0"));
        assertEquals("no line means no answer time", -1, BestSpot.rttFromSummary(""));
        assertEquals("no line means no answer time", -1, BestSpot.rttFromSummary(null));
    }
}
