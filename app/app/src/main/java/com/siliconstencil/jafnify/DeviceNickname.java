package com.siliconstencil.jafnify;

/** The name this device calls itself. Local only; the peer never reads it. */
final class DeviceNickname {
    /** The longest name a device may carry. */
    static final int MAX_LEN = 32;

    private DeviceNickname() {
    }

    /** Control characters removed, the rest trimmed, at most MAX_LEN. */
    static String clean(String input) {
        if (input == null) {
            return "";
        }
        StringBuilder kept = new StringBuilder();
        for (int i = 0; i < input.length(); i++) {
            char c = input.charAt(i);
            if (!Character.isISOControl(c)) {
                kept.append(c);
            }
        }
        String trimmed = kept.toString().trim();
        if (trimmed.length() <= MAX_LEN) {
            return trimmed;
        }
        return trimmed.substring(0, MAX_LEN);
    }
}
