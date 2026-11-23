package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The parser of the phone home: the text the core sends becomes a card and
 * tiles. Pure JVM: the text is a literal, the parser is plain Java.
 */
public class PhoneHomeTest {
    private static final String SAMPLE = "JFH1 1\n"
            + "s | connected=1 | tone=ok | title=Sharing with the laptop | sub=Up 0:01:12 - 903.0 MB today"
            + " - peak 49.9 MB/s down | stop=1 | action=Stop sharing | notice=\n"
            + "t | id=signal | label=SIGNAL | badge= | btone=ok | value=-71 | unit=dBm | tone=muted"
            + " | kind=line | points=-71,-70 | ref= | level=4 | level_max=4"
            + " | fact=5G (advanced) | fact=Signal 4/4 | note=\n"
            + "t | id=heat | label=HEAT | badge= | btone=ok | value=30 | unit=C | tone=muted"
            + " | kind=line | points=300,301 | ref=420 | level= | level_max="
            + " | fact=Android: no throttling | note=\n";

    @Test
    public void the_card_keeps_its_words() {
        PhoneHome.Model home = PhoneHome.parse(SAMPLE);
        assertEquals("Sharing with the laptop", home.status.title);
        assertEquals("Up 0:01:12 - 903.0 MB today - peak 49.9 MB/s down", home.status.subline);
        assertEquals("Stop sharing", home.status.action);
        assertTrue(home.status.connected);
        assertTrue(home.status.stop);
    }

    @Test
    public void the_tiles_keep_their_parts() {
        PhoneHome.Model home = PhoneHome.parse(SAMPLE);
        assertEquals(2, home.tiles.size());
        PhoneHome.Tile signal = home.tiles.get(0);
        assertEquals("signal", signal.id);
        assertEquals("SIGNAL", signal.label);
        assertEquals("-71", signal.value);
        assertEquals("dBm", signal.unit);
        assertEquals(2, signal.facts.size());
        assertEquals("5G (advanced)", signal.facts.get(0));
        assertEquals(4, signal.level);
        assertEquals(4, signal.levelMax);
        assertEquals("line", signal.kind);
        assertEquals(2, signal.points.length);
        assertEquals(-71, signal.points[0]);
    }

    @Test
    public void the_heat_tile_keeps_its_reference() {
        PhoneHome.Model home = PhoneHome.parse(SAMPLE);
        PhoneHome.Tile heat = home.tiles.get(1);
        assertEquals("HEAT", heat.label);
        assertTrue(heat.hasReference);
        assertEquals(420, heat.reference);
        assertEquals("Android: no throttling", heat.facts.get(0));
    }

    @Test
    public void the_state_of_the_sharing_is_told_to_the_core() {
        // The card must not say "sharing" while the tunnel is still coming up.
        assertEquals(PhoneHome.IDLE, PhoneHome.stateCode(false, false));
        assertEquals(PhoneHome.IDLE, PhoneHome.stateCode(false, true));
        assertEquals(PhoneHome.TRYING, PhoneHome.stateCode(true, false));
        assertEquals(PhoneHome.SHARING, PhoneHome.stateCode(true, true));
    }

    @Test
    public void a_hotspot_off_card_uses_the_off_sentence_as_its_title() {
        String text = "JFH1 1\n"
                + "s | connected=0 | tone=wait | title=Trying to share"
                + " | sub=Hotspot is off. Turn it on to keep sharing."
                + " | stop=1 | action=Stop sharing | notice=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        PhoneHome.showHotspotOff(home, "Hotspot is off. Turn it on to keep sharing.");
        assertEquals("Hotspot is off. Turn it on to keep sharing.", home.status.title);
        assertEquals("", home.status.subline);
        assertEquals("wait", home.status.tone);
    }

    @Test
    public void a_hotspot_off_card_also_drops_the_path_notice() {
        String text = "JFH1 1\n"
                + "s | connected=1 | tone=ok | title=Sharing with the laptop | sub=Up 0:01"
                + " | stop=1 | action=Stop sharing | notice=Direct over hotspot\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        assertEquals("Direct over hotspot", home.status.notice);
        PhoneHome.showHotspotOff(home, "Hotspot is off. Turn it on to keep sharing.");
        assertEquals("", home.status.notice);
    }

    @Test
    public void the_link_values_are_not_current_while_the_hotspot_is_off() {
        String text = "JFH1 1\n"
                + "s | connected=1 | tone=ok | title=Sharing with the laptop | sub=Up 0:01"
                + " | stop=1 | action=Stop sharing | notice=Direct over hotspot\n"
                + "t | id=range | label=RANGE | badge= | btone=ok | value=-33 | unit=dBm"
                + " | tone=muted | kind=line | points=-33,-34 | ref= | level= | level_max="
                + " | fact=-34 to -33 dBm in 5 min | fact=Likely close to the phone"
                + " | note=Link 866.7 Mbit/s\n"
                + "t | id=speed | label=SPEED | badge= | btone=ok | value=99.7 | unit=KB/s"
                + " | tone=muted | kind=line | points=99,98 | ref= | level= | level_max="
                + " | fact=0.0-99.7 KB/s in 5 min | note=Peak 99.7 KB/s down\n"
                + "t | id=heat | label=HEAT | badge= | btone=ok | value=30 | unit=C"
                + " | tone=muted | kind=line | points=300,301 | ref=420 | level= | level_max="
                + " | fact=Android: no throttling | note=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        PhoneHome.hideLinkValues(home);

        PhoneHome.Tile range = home.tiles.get(0);
        assertEquals("--", range.value);
        assertEquals("", range.unit);
        assertEquals("none", range.kind);
        assertEquals(0, range.points.length);
        assertEquals(1, range.facts.size());
        assertEquals("No link to the laptop", range.facts.get(0));
        assertEquals("", range.note);

        PhoneHome.Tile speed = home.tiles.get(1);
        assertEquals("--", speed.value);
        assertEquals("", speed.unit);
        assertEquals(1, speed.facts.size());
        assertEquals("No link", speed.facts.get(0));
        assertEquals("", speed.note);

        // The heat tile is measured on the phone itself: it stays live.
        PhoneHome.Tile heat = home.tiles.get(2);
        assertEquals("30", heat.value);
        assertEquals("C", heat.unit);
        assertEquals("Android: no throttling", heat.facts.get(0));
    }

    @Test
    public void the_card_keeps_its_tone() {
        // The dot of the card takes its colour from the tone: a phone that is
        // trying must not show the colour of a phone that is sharing.
        PhoneHome.Model home = PhoneHome.parse(SAMPLE);
        assertEquals("ok", home.status.tone);
    }

    @Test
    public void the_cells_of_a_strip_are_read() {
        String text = "JFH1 1\n"
                + "s | connected=0 | tone=wait | title=Trying to share | sub=Retrying in 12 s"
                + " | stop=1 | action=Stop sharing | notice=\n"
                + "t | id=drops | label=DROPS 24 H | badge= | btone=muted | value=2 | unit="
                + " | tone=err | kind=hours | points= | ref= | level= | level_max="
                + " | cells=15:unknown,16:drop | fact=11:35, back after 1:53 | note=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        assertEquals("wait", home.status.tone);
        assertEquals(1, home.tiles.size());
        PhoneHome.Tile drops = home.tiles.get(0);
        assertEquals("hours", drops.kind);
        assertEquals(2, drops.cells.size());
        assertEquals("15:unknown", drops.cells.get(0));
        assertEquals("16:drop", drops.cells.get(1));
    }

    @Test
    public void a_missing_tile_is_not_a_tile() {
        String text = "JFH1 1\n"
                + "s | connected=0 | tone=muted | title=Not sharing | sub= | stop=0 | action=Share | notice=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        assertEquals(0, home.tiles.size());
        assertEquals("Not sharing", home.status.title);
        assertEquals("Share", home.status.action);
        assertTrue(!home.status.connected);
    }

    @Test
    public void a_text_without_the_header_is_empty() {
        assertTrue(PhoneHome.parse("").tiles.isEmpty());
        assertTrue(PhoneHome.parse("nonsense").tiles.isEmpty());
        assertTrue(PhoneHome.parse(null).tiles.isEmpty());
    }

    @Test
    public void a_text_of_another_version_is_not_a_home() {
        // The fields of a later version may have moved: a phone that reads
        // them anyway would show numbers it does not understand.
        String text = "JFH1 10\n"
                + "s | connected=1 | tone=ok | title=Sharing with the laptop | sub= | stop=1"
                + " | action=Stop sharing | notice=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        assertEquals("", home.status.title);
        assertTrue(home.tiles.isEmpty());
    }

    @Test
    public void the_battery_tile_keeps_its_bar() {
        String text = "JFH1 1\n"
                + "s | connected=1 | tone=ok | title=Sharing with the laptop | sub= | stop=1"
                + " | action=Stop sharing | notice=\n"
                + "t | id=battery | label=BATTERY | badge= | btone=ok | value=74 | unit=%"
                + " | tone=muted | kind=fill | fill=74 | fill_max=100 | ref= | level= | level_max="
                + " | fact=Charging | fact=Battery saver off | note=\n";
        PhoneHome.Model home = PhoneHome.parse(text);
        assertEquals(1, home.tiles.size());
        PhoneHome.Tile battery = home.tiles.get(0);
        assertEquals("fill", battery.kind);
        assertEquals(74, battery.fill);
        assertEquals(100, battery.fillMax);
        assertEquals("Charging", battery.facts.get(0));
    }

    @Test
    public void theSplitOfTodayIsRead() {
        String text = "JFH1 1\nt | id=today | label=TODAY | value=903.0 MB | "
                + "kind=split | down=695000000 | up=208000000 | fact=Down 695.0 MB, up 208.0 MB";
        PhoneHome.Model model = PhoneHome.parse(text);
        assertEquals(1, model.tiles.size());
        PhoneHome.Tile tile = model.tiles.get(0);
        assertEquals("split", tile.kind);
        assertEquals(695000000L, tile.down);
        assertEquals(208000000L, tile.up);
    }

    @Test
    public void theSplitOfABigDayIsRead() {
        String text = "JFH1 1\nt | id=today | label=TODAY | "
                + "kind=split | down=3000000000 | up=2500000000";
        PhoneHome.Model model = PhoneHome.parse(text);
        PhoneHome.Tile tile = model.tiles.get(0);
        assertEquals(3000000000L, tile.down);
        assertEquals(2500000000L, tile.up);
    }
}
