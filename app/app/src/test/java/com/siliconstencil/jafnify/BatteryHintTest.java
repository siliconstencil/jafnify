package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The battery hint matrix: sharing, on battery, not exempt, not dismissed. */
public class BatteryHintTest {
    @Test
    public void the_hint_needs_a_running_session_on_battery_without_exemption() {
        assertTrue(BatteryHint.show(true, false, false, false));
    }

    @Test
    public void a_plugged_phone_never_shows_it() {
        assertFalse(BatteryHint.show(true, true, false, false));
    }

    @Test
    public void an_exempt_app_never_shows_it() {
        assertFalse(BatteryHint.show(true, false, true, false));
    }

    @Test
    public void a_dismissed_hint_stays_hidden_for_the_run() {
        assertFalse(BatteryHint.show(true, false, false, true));
    }

    @Test
    public void an_idle_phone_never_shows_it() {
        assertFalse(BatteryHint.show(false, false, false, false));
    }
}
