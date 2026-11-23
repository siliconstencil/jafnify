package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The four start reasons on the wire, in the log and in the record. */
public class StartReasonTest {
    @Test
    public void the_wire_words_are_fixed() {
        assertEquals("user", StartReason.USER.wire());
        assertEquals("boot", StartReason.BOOT.wire());
        assertEquals("system restart", StartReason.SYSTEM_RESTART.wire());
        assertEquals("app update", StartReason.UPDATE.wire());
    }

    @Test
    public void a_known_word_round_trips() {
        for (StartReason reason : StartReason.values()) {
            assertEquals(reason, StartReason.parse(reason.wire()));
        }
    }

    @Test
    public void a_missing_or_unknown_extra_is_the_user() {
        assertEquals(StartReason.USER, StartReason.parse(null));
        assertEquals(StartReason.USER, StartReason.parse(""));
        assertEquals(StartReason.USER, StartReason.parse("hotspot"));
        assertEquals(StartReason.USER, StartReason.parse("BOOT"));
    }
}
