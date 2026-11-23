package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class PhonePermissionTest {
    @Test
    public void nothingIsAskedBeforeTheUserTurnsTheSwitchOn() {
        assertEquals(PhonePermission.State.NOT_ASKED,
                PhonePermission.initial(35));
        assertEquals(PhonePermission.State.UNSUPPORTED,
                PhonePermission.initial(21));
        // API 23-30 has no TelephonyCallback: the permission would give nothing.
        assertEquals(PhonePermission.State.UNSUPPORTED,
                PhonePermission.initial(30));
    }

    @Test
    public void theStateAfterTheDialogIsGrantedDeniedOrDontAskAgain() {
        assertEquals(PhonePermission.State.GRANTED,
                PhonePermission.afterRequest(35, true, false));
        assertEquals(PhonePermission.State.DENIED,
                PhonePermission.afterRequest(35, false, true));
        assertEquals(PhonePermission.State.DONT_ASK_AGAIN,
                PhonePermission.afterRequest(35, false, false));
        assertEquals(PhonePermission.State.UNSUPPORTED,
                PhonePermission.afterRequest(21, false, true));
    }

    @Test
    public void theStateRightNowComesFromTheSystemAndWhatWeAsked() {
        assertEquals(PhonePermission.State.GRANTED,
                PhonePermission.current(35, true, false, true));
        assertEquals(PhonePermission.State.NOT_ASKED,
                PhonePermission.current(35, false, false, false));
        assertEquals(PhonePermission.State.DENIED,
                PhonePermission.current(35, false, true, true));
        assertEquals(PhonePermission.State.DONT_ASK_AGAIN,
                PhonePermission.current(35, false, false, true));
    }

    @Test
    public void aRevokedPermissionGoesBackToTheSettings() {
        assertEquals(PhonePermission.State.REVOKED, PhonePermission.revoked());
        assertTrue(PhonePermission.needsSettings(PhonePermission.State.REVOKED));
        assertTrue(PhonePermission.needsSettings(PhonePermission.State.DONT_ASK_AGAIN));
        assertFalse(PhonePermission.needsSettings(PhonePermission.State.DENIED));
        assertFalse(PhonePermission.needsSettings(PhonePermission.State.GRANTED));
    }

    @Test
    public void theGenerationIsOnlyReadableFromApi31() {
        assertFalse(PhonePermission.generationAvailable(30));
        assertTrue(PhonePermission.generationAvailable(31));
        assertTrue(PhonePermission.generationAvailable(35));
    }

    @Test
    public void everyStateHasItsOwnReasonLine() {
        String denied = "denied";
        String dontAsk = "dont ask";
        String revoked = "revoked";
        String unsupported = "unsupported";
        assertEquals(denied, PhonePermission.reasonLine(
                PhonePermission.State.DENIED, denied, dontAsk, revoked, unsupported));
        assertEquals(dontAsk, PhonePermission.reasonLine(
                PhonePermission.State.DONT_ASK_AGAIN, denied, dontAsk, revoked, unsupported));
        assertEquals(revoked, PhonePermission.reasonLine(
                PhonePermission.State.REVOKED, denied, dontAsk, revoked, unsupported));
        assertEquals(unsupported, PhonePermission.reasonLine(
                PhonePermission.State.UNSUPPORTED, denied, dontAsk, revoked, unsupported));
        assertEquals("", PhonePermission.reasonLine(
                PhonePermission.State.GRANTED, denied, dontAsk, revoked, unsupported));
        assertEquals("", PhonePermission.reasonLine(
                PhonePermission.State.NOT_ASKED, denied, dontAsk, revoked, unsupported));
    }

    @Test
    public void a_missing_permission_turns_a_running_switch_off() {
        assertTrue(PhonePermission.shouldTurnOff(false, true, false));
        assertTrue(PhonePermission.shouldTurnOff(false, false, true));
        assertFalse(PhonePermission.shouldTurnOff(false, false, false));
        assertFalse(PhonePermission.shouldTurnOff(true, true, true));
    }

    @Test
    public void the_signal_level_only_leaves_when_a_switch_is_on() {
        assertEquals(3, PhonePermission.gatedSignal(true, true, false, 3));
        assertEquals(3, PhonePermission.gatedSignal(true, false, true, 3));
        assertEquals(-1, PhonePermission.gatedSignal(false, true, true, 3));
        assertEquals(-1, PhonePermission.gatedSignal(true, false, false, 3));
        assertEquals(-1, PhonePermission.gatedSignal(true, true, false, -1));
        assertEquals(-1, PhonePermission.gatedSignal(true, true, false, 5));
    }

    @Test
    public void the_signal_strength_leaves_with_the_same_gates() {
        assertEquals(-95, PhonePermission.gatedDbm(true, true, false, -95));
        assertEquals(-95, PhonePermission.gatedDbm(true, false, true, -95));
        assertEquals(-1, PhonePermission.gatedDbm(false, true, true, -95));
        assertEquals(-1, PhonePermission.gatedDbm(true, false, false, -95));
        assertEquals(-1, PhonePermission.gatedDbm(true, true, false, -1));
        assertEquals(-1, PhonePermission.gatedDbm(true, true, false, -500));
    }

    // The phone measures its own spots, so the best-spot switch opens the same
    // gate as the report switches: the level and the strength travel together.
    @Test
    public void the_best_spot_switch_opens_the_signal_gate() {
        assertEquals(3, PhonePermission.gatedSignal(true, false, false, true, 3));
        assertEquals(-95, PhonePermission.gatedDbm(true, false, false, true, -95));
        assertEquals("without the permission nothing travels",
                -1, PhonePermission.gatedSignal(false, false, false, true, 3));
        assertEquals("without the permission nothing travels",
                -1, PhonePermission.gatedDbm(false, false, false, true, -95));
        assertEquals("the report switches keep their own gating",
                -1, PhonePermission.gatedSignal(true, false, false, false, 3));
        assertEquals("the report switches keep their own gating",
                -1, PhonePermission.gatedDbm(true, false, false, false, -95));
        assertEquals("a level out of range is still no level",
                -1, PhonePermission.gatedSignal(true, false, false, true, 5));
        assertEquals("a strength out of range is still no strength",
                -1, PhonePermission.gatedDbm(true, false, false, true, -500));
    }
}
