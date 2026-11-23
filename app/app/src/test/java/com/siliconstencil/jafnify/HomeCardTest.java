package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The card of the tile home: one state, one sentence and one button, plus the
 * pairing code when a code is on screen. Pure JVM: the model takes plain
 * words, never Android views.
 */
public class HomeCardTest {
    private static PhoneHome.Status status(String title, String sub, String action,
            boolean stop, boolean connected) {
        PhoneHome.Status status = new PhoneHome.Status();
        status.title = title;
        status.subline = sub;
        status.action = action;
        status.stop = stop;
        status.connected = connected;
        return status;
    }

    @Test
    public void state_follows_the_service() {
        assertEquals(HomeCard.IDLE,
                HomeCard.stateFor(false, false, false, false));
        assertEquals(HomeCard.TRYING,
                HomeCard.stateFor(false, true, false, false));
        assertEquals(HomeCard.IDLE,
                HomeCard.stateFor(false, true, false, true));
        assertEquals(HomeCard.TRYING,
                HomeCard.stateFor(true, false, false, false));
        assertEquals(HomeCard.SHARING,
                HomeCard.stateFor(true, false, true, false));
    }

    @Test
    public void idle_says_not_sharing_once_and_offers_share() {
        HomeCard card = HomeCard.of(status("Not sharing", "Not sharing", "Share",
                false, false), "", "");
        assertEquals(HomeCard.IDLE, card.state);
        assertEquals("Not sharing", card.title);
        assertEquals("", card.subline);
        assertEquals("Share", card.action);
        assertFalse(card.stop);
        assertFalse(card.showCode);
    }

    @Test
    public void an_idle_card_keeps_a_useful_subline() {
        HomeCard card = HomeCard.of(status("Not sharing", "Last share ended", "Share",
                false, false), "", "");
        assertEquals("Last share ended", card.subline);
    }

    @Test
    public void trying_says_starting_and_offers_stop() {
        HomeCard card = HomeCard.of(status("Trying to share", "Starting...",
                "Stop sharing", true, false), "", "");
        assertEquals(HomeCard.TRYING, card.state);
        assertEquals("Trying to share", card.title);
        assertEquals("Starting...", card.subline);
        assertEquals("Stop sharing", card.action);
        assertTrue(card.stop);
        assertFalse(card.showCode);
    }

    @Test
    public void a_code_on_screen_comes_with_its_expiry_and_a_new_code() {
        HomeCard card = HomeCard.of(status("Trying to share", "Starting...",
                "Stop sharing", true, false), "1234", "Expires in 10 minutes");
        assertEquals(HomeCard.TRYING, card.state);
        assertTrue(card.showCode);
        assertTrue(card.showExpiry);
        assertTrue(card.showNewCode);
        assertTrue(card.stop);
    }

    @Test
    public void a_code_without_an_expiry_line_still_shows_the_code() {
        HomeCard card = HomeCard.of(status("Trying to share", "Starting...",
                "Stop sharing", true, false), "1234", "");
        assertTrue(card.showCode);
        assertFalse(card.showExpiry);
        assertTrue(card.showNewCode);
    }

    @Test
    public void connected_hides_the_code_and_keeps_the_uptime() {
        HomeCard card = HomeCard.of(status("Sharing with the laptop", "Up 0:01:12",
                "Stop sharing", true, true), "1234", "Expires in 10 minutes");
        assertEquals(HomeCard.SHARING, card.state);
        assertEquals("Up 0:01:12", card.subline);
        assertFalse(card.showCode);
        assertFalse(card.showExpiry);
        assertFalse(card.showNewCode);
        assertTrue(card.stop);
    }

    @Test
    public void waiting_for_hotspot_keeps_the_code_and_stop() {
        PhoneHome.Model home = new PhoneHome.Model();
        home.status.title = "Trying to share";
        home.status.subline = "Starting...";
        home.status.action = "Stop sharing";
        home.status.stop = true;
        PhoneHome.showHotspotOff(home, "Hotspot is off. Turn it on to keep sharing.");
        HomeCard card = HomeCard.of(home.status, "1234", "");
        assertEquals(HomeCard.TRYING, card.state);
        assertEquals("Hotspot is off. Turn it on to keep sharing.", card.title);
        assertEquals("", card.subline);
        assertEquals("Stop sharing", card.action);
        assertTrue(card.showCode);
    }

    @Test
    public void a_start_without_a_hotspot_says_turn_on_the_hotspot() {
        PhoneHome.Model home = new PhoneHome.Model();
        home.status.title = "Trying to share";
        home.status.subline = "Searching for the other device - Laptop";
        home.status.action = "Stop sharing";
        home.status.stop = true;
        PhoneHome.showHotspotOff(home,
                "Hotspot is off. Turn on the hotspot to share.");
        HomeCard card = HomeCard.of(home.status, "1234", "");
        assertEquals("Hotspot is off. Turn on the hotspot to share.", card.title);
        assertEquals("", card.subline);
        assertEquals("Stop sharing", card.action);
        assertTrue(card.stop);
        assertTrue(card.showCode);
    }

    @Test
    public void an_idle_card_never_shows_a_leftover_code() {
        HomeCard card = HomeCard.of(status("Not sharing", "", "Share", false, false),
                "1234", "");
        assertFalse(card.showCode);
    }

    @Test
    public void a_null_status_is_an_empty_idle_card() {
        HomeCard card = HomeCard.of(null, "", "");
        assertEquals(HomeCard.IDLE, card.state);
        assertEquals("", card.title);
        assertEquals("", card.subline);
        assertFalse(card.showCode);
    }
}
