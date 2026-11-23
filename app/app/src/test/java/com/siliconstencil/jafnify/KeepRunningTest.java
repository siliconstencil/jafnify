package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The pure status decisions behind the keep-running rows. */
public class KeepRunningTest {
    @Test
    public void the_unused_row_is_hidden_below_api_30() {
        assertEquals(KeepRunning.UNUSED_HIDDEN, KeepRunning.unused(29, true));
        assertEquals(KeepRunning.UNUSED_HIDDEN, KeepRunning.unused(21, false));
    }

    @Test
    public void the_unused_row_reads_the_auto_revoke_state() {
        assertEquals(KeepRunning.UNUSED_OK, KeepRunning.unused(30, true));
        assertEquals(KeepRunning.UNUSED_CAN_REVOKE, KeepRunning.unused(30, false));
        assertEquals(KeepRunning.UNUSED_OK, KeepRunning.unused(35, true));
    }

    @Test
    public void data_saver_ok_covers_disabled_and_whitelisted() {
        assertEquals(KeepRunning.DATA_OK, KeepRunning.dataSaver(1));
        assertEquals(KeepRunning.DATA_OK, KeepRunning.dataSaver(2));
        assertEquals(KeepRunning.DATA_BLOCKED, KeepRunning.dataSaver(3));
        assertEquals(KeepRunning.DATA_UNKNOWN, KeepRunning.dataSaver(0));
    }
}
