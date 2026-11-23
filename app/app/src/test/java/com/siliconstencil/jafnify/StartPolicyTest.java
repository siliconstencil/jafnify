package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The full start matrix: setting, pair, terms, stop, trigger. */
public class StartPolicyTest {
    private static boolean expected(StartPolicy.Trigger trigger, boolean setting,
            boolean pair, boolean terms, boolean stopped) {
        switch (trigger) {
            case COLD_OPEN:
                return setting && pair && terms;
            case SERVICE_RESTART:
            case BOOT:
            case UPDATE:
                return setting && pair && terms && !stopped;
            case RESTORE:
            default:
                return false;
        }
    }

    @Test
    public void the_matrix_is_exact_for_every_trigger_and_input() {
        for (StartPolicy.Trigger trigger : StartPolicy.Trigger.values()) {
            for (boolean setting : new boolean[] {false, true}) {
                for (boolean pair : new boolean[] {false, true}) {
                    for (boolean terms : new boolean[] {false, true}) {
                        for (boolean stopped : new boolean[] {false, true}) {
                            String where = trigger + " setting=" + setting + " pair=" + pair
                                    + " terms=" + terms + " stopped=" + stopped;
                            assertEquals(where, expected(trigger, setting, pair, terms, stopped),
                                    StartPolicy.shouldStart(trigger, setting, pair, terms,
                                            stopped));
                        }
                    }
                }
            }
        }
    }

    @Test
    public void no_automatic_trigger_starts_with_both_flags_off() {
        for (StartPolicy.Trigger trigger : new StartPolicy.Trigger[] {
                StartPolicy.Trigger.SERVICE_RESTART, StartPolicy.Trigger.BOOT,
                StartPolicy.Trigger.UPDATE}) {
            assertFalse(StartPolicy.shouldStart(trigger, false, true, true, false));
        }
    }

    @Test
    public void an_update_restarts_only_a_run_that_was_live_and_never_a_stop() {
        // The "setting" bit of an update is whether sharing was live before
        // the install (the saved KEY_SHARING_LIVE).
        assertTrue(StartPolicy.shouldStart(StartPolicy.Trigger.UPDATE,
                true, true, true, false));
        assertFalse("a user Stop wins", StartPolicy.shouldStart(
                StartPolicy.Trigger.UPDATE, true, true, true, true));
        assertFalse("sharing was not on", StartPolicy.shouldStart(
                StartPolicy.Trigger.UPDATE, false, true, true, false));
    }

    @Test
    public void a_restore_never_starts_even_when_everything_is_allowed() {
        assertFalse(StartPolicy.shouldStart(StartPolicy.Trigger.RESTORE,
                true, true, true, false));
    }

    @Test
    public void a_user_stop_stops_the_automatic_triggers_but_not_a_cold_open() {
        assertFalse(StartPolicy.shouldStart(StartPolicy.Trigger.BOOT,
                true, true, true, true));
        assertFalse(StartPolicy.shouldStart(StartPolicy.Trigger.SERVICE_RESTART,
                true, true, true, true));
        assertTrue(StartPolicy.shouldStart(StartPolicy.Trigger.COLD_OPEN,
                true, true, true, true));
    }

    @Test
    public void no_terms_and_no_pair_never_start() {
        for (StartPolicy.Trigger trigger : StartPolicy.Trigger.values()) {
            assertFalse(trigger + " no terms", StartPolicy.shouldStart(trigger,
                    true, true, false, false));
            assertFalse(trigger + " no pair", StartPolicy.shouldStart(trigger,
                    true, false, true, false));
        }
    }

    @Test
    public void the_action_is_start_or_nothing_and_never_pair_new() {
        for (StartPolicy.Trigger trigger : StartPolicy.Trigger.values()) {
            for (boolean setting : new boolean[] {false, true}) {
                for (boolean pair : new boolean[] {false, true}) {
                    for (boolean terms : new boolean[] {false, true}) {
                        for (boolean stopped : new boolean[] {false, true}) {
                            String action = StartPolicy.actionFor(trigger, setting, pair,
                                    terms, stopped);
                            String where = trigger + " setting=" + setting + " pair=" + pair
                                    + " terms=" + terms + " stopped=" + stopped;
                            assertNotEquals(where, SharerService.ACTION_PAIR_NEW, action);
                            if (StartPolicy.shouldStart(trigger, setting, pair, terms,
                                    stopped)) {
                                assertEquals(where, SharerService.ACTION_START, action);
                            } else {
                                assertEquals(where, null, action);
                            }
                        }
                    }
                }
            }
        }
    }
}
