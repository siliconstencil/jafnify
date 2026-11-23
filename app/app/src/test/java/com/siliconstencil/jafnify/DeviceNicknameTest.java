package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class DeviceNicknameTest {
    @Test
    public void controlCharactersAreStripped() {
        assertEquals("Alice", DeviceNickname.clean("Ali\nce\t"));
    }

    @Test
    public void theNameIsTrimmed() {
        assertEquals("Bob", DeviceNickname.clean("  Bob  "));
    }

    @Test
    public void theNameIsAtMostThirtyTwoCharacters() {
        StringBuilder text = new StringBuilder();
        for (int i = 0; i < 40; i++) {
            text.append('x');
        }
        assertEquals(32, DeviceNickname.clean(text.toString()).length());
    }

    @Test
    public void printableUnicodeIsKept() {
        assertEquals("Kai \u00fcber", DeviceNickname.clean("Kai \u00fcber"));
    }

    @Test
    public void anEmptyNameIsAllowed() {
        assertEquals("", DeviceNickname.clean(""));
        assertEquals("", DeviceNickname.clean(null));
    }
}
