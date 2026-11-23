package com.siliconstencil.jafnify;

/** Why a sharing session started, one fixed word for the log and the record. */
public enum StartReason {
    USER("user"),
    BOOT("boot"),
    SYSTEM_RESTART("system restart"),
    UPDATE("app update");

    private final String wire;

    StartReason(String wire) {
        this.wire = wire;
    }

    /** The one word used in the log line, the journal and the session record. */
    public String wire() {
        return wire;
    }

    /** An absent or unknown extra reads as a manual start. */
    public static StartReason parse(String value) {
        for (StartReason reason : values()) {
            if (reason.wire.equals(value)) {
                return reason;
            }
        }
        return USER;
    }
}
