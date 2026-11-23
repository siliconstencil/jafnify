package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The one-line "sharing stopped" notice under the status: shown once after a
 * recent system stop, never after a user stop, an update, or while sharing.
 */
public class ShareStoppedNoticeTest {
    private static final long NOW = 1_700_000_000_000L;
    private static final int CRASH = 4;

    @Test
    public void aRecentSystemStopIsShownOnceAndANewerStopAgain() {
        assertTrue(ShareStoppedNotice.shouldShow(false, true, 60_000, CRASH, NOW, 0));
        assertFalse("the same exit is not shown twice",
                ShareStoppedNotice.shouldShow(false, true, 60_000, CRASH, NOW, NOW));
        assertTrue("a later exit is a new stop",
                ShareStoppedNotice.shouldShow(false, true, 60_000, CRASH, NOW + 1, NOW));
    }

    @Test
    public void aUserStopOrAnUpdateIsNotShown() {
        for (int reason : new int[] {
                ShareStoppedNotice.EXIT_SELF,
                ShareStoppedNotice.USER_REQUESTED,
                ShareStoppedNotice.USER_STOPPED,
                ShareStoppedNotice.PACKAGE_UPDATED,
        }) {
            assertFalse("reason " + reason,
                    ShareStoppedNotice.shouldShow(false, true, 1_000, reason, NOW, 0));
        }
    }

    @Test
    public void aRunningShareAnOldExitOrABadTimestampIsNotShown() {
        assertFalse("sharing now",
                ShareStoppedNotice.shouldShow(true, true, 1_000, CRASH, NOW, 0));
        assertFalse("older than the session",
                ShareStoppedNotice.shouldShow(false, true, ExitReasons.RECENT_MS + 1, CRASH, NOW, 0));
        assertFalse("negative age",
                ShareStoppedNotice.shouldShow(false, true, -1, CRASH, NOW, 0));
        assertFalse("no exit timestamp",
                ShareStoppedNotice.shouldShow(false, true, 1_000, CRASH, 0, 0));
    }

    @Test
    public void anIdleAppClosedByAndroidSaysNothing() {
        // Android closes idle apps all the time: only a process that was
        // sharing when it ended gets the line.
        assertFalse(ShareStoppedNotice.shouldShow(false, false, 60_000, CRASH, NOW, 0));
    }
}
