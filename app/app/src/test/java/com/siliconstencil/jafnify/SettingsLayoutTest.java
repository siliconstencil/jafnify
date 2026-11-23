package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class SettingsLayoutTest {
    @Test
    public void theFirstRowGetsExtraRoomOnlyWithoutANote() {
        assertEquals(R.dimen.jafnify_spacing_md,
                SettingsLayout.rowTopMarginRes(true, false));
        assertEquals(R.dimen.jafnify_spacing_xs,
                SettingsLayout.rowTopMarginRes(true, true));
        assertEquals(R.dimen.jafnify_spacing_sm,
                SettingsLayout.rowTopMarginRes(false, false));
        assertEquals(R.dimen.jafnify_spacing_sm,
                SettingsLayout.rowTopMarginRes(false, true));
    }

    @Test
    public void theAdvancedPlaceholderComesBeforeExperimental() {
        assertEquals(true, SettingsLayout.advancedPlaceholderFirst(false, true));
        assertEquals(false, SettingsLayout.advancedPlaceholderFirst(true, true));
        assertEquals(false, SettingsLayout.advancedPlaceholderFirst(false, false));
    }
}
