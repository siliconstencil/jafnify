package com.siliconstencil.jafnify;

/** Pair confirmation wording, pure so the no-pair case is testable. */
final class DevicesText {
    private DevicesText() {
    }

    static String pairNewMessage(String pairedName, String replaceMessage,
            String firstPairMessage) {
        return pairedName == null || pairedName.isEmpty()
                ? firstPairMessage
                : replaceMessage;
    }
}
