package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import java.util.TimeZone;
import org.junit.After;
import org.junit.Test;

/** The 12-hour setting changes only the hour style; text stays English. */
public class ClockTest {
    private static final TimeZone PST = TimeZone.getTimeZone("America/Los_Angeles");
    private static final TimeZone UTC = TimeZone.getTimeZone("UTC");

    @After
    public void back() {
        Clock.twelveHour = false;
    }

    @Test
    public void twelveHourTimesCarryAmAndPm() {
        Clock.twelveHour = true;
        assertEquals("2023-11-14 2:13:20 PM PST",
                StatsText.formatTimestampOrNone(1_700_000_000_000L, PST));
        assertEquals("2023-11-14 2:13:20 PM PST",
                ConnectionLog.timestamp(1_700_000_000_000L, PST));
        assertEquals("1970-01-01 12:00:00 AM UTC", ConnectionLog.timestamp(0, UTC));
        assertEquals("Oct 23, 2:05 PM", UiText.formatWhen(1_792_764_300_000L, UTC));
    }

    @Test
    public void twentyFourHourTimesStayAsTheyWere() {
        Clock.twelveHour = false;
        assertEquals("2023-11-14 14:13:20 PST",
                StatsText.formatTimestampOrNone(1_700_000_000_000L, PST));
        assertEquals("Oct 23, 14:05", UiText.formatWhen(1_792_764_300_000L, UTC));
    }
}
