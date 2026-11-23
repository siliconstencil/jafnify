package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class DevicesTextTest {
    @Test
    public void aPairUsesTheReplaceMessageAndNoPairUsesTheFirstPairMessage() {
        assertEquals("Pair this phone with a laptop?",
                DevicesText.pairNewMessage("", "The new pairing replaces Phone.", "Pair this phone with a laptop?"));
        assertEquals("The new pairing replaces Phone.",
                DevicesText.pairNewMessage("Phone", "The new pairing replaces Phone.", "Pair this phone with a laptop?"));
    }
}
