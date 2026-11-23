package com.siliconstencil.jafnify;

import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

/**
 * The effective value of a feature flag on the phone.
 *
 * <p>The core is the one source of the table. Rows in the standard bucket
 * have no switch and are always on; the phone reads a stored choice only for
 * the rows the core does not name. The old network_report and signal_history
 * switches became the merged network_details row: an install that had either
 * one on keeps it on until the user makes a new choice.
 */
public final class FlagValues {
    /** The core id of the merged phone-network row. */
    static final String NETWORK_DETAILS = "network_details";
    /** The two removed rows the merged row replaces. */
    static final String OLD_NETWORK_REPORT = "network_report";
    static final String OLD_SIGNAL_HISTORY = "signal_history";

    private static volatile Set<String> standard;

    private FlagValues() {
    }

    /** The always-on ids, as the native side lists them; cached for the run. */
    public static Set<String> standardIds() {
        Set<String> cached = standard;
        if (cached != null) {
            return cached;
        }
        Set<String> ids = new HashSet<>();
        String text = "";
        try {
            text = Native.standardFlags();
        } catch (Throwable ignored) {
            // A plain JVM test has no native library: no row is always on.
        }
        if (text != null) {
            for (String line : text.split("\n", -1)) {
                String id = line.trim();
                if (!id.isEmpty()) {
                    ids.add(id);
                }
            }
        }
        Set<String> fixed = Collections.unmodifiableSet(ids);
        standard = fixed;
        return fixed;
    }

    /** Whether a flag is on: standard rows always, the merged row by rule. */
    public static boolean enabled(Autostart.Store store, String id) {
        return enabled(standardIds(), store, id);
    }

    static boolean enabled(Set<String> standardIds, Autostart.Store store, String id) {
        if (standardIds.contains(id)) {
            return true;
        }
        if (NETWORK_DETAILS.equals(id)) {
            return networkDetails(store);
        }
        return store != null && store.getBoolean(id, false);
    }

    /** The merged row: the new stored choice wins, else either old key. */
    public static boolean networkDetails(Autostart.Store store) {
        if (store == null) {
            return false;
        }
        return mergedNetworkDetails(store.contains(NETWORK_DETAILS),
                store.getBoolean(NETWORK_DETAILS, false),
                store.getBoolean(OLD_NETWORK_REPORT, false),
                store.getBoolean(OLD_SIGNAL_HISTORY, false));
    }

    static boolean mergedNetworkDetails(boolean hasNew, boolean newValue,
            boolean oldReport, boolean oldSignal) {
        return hasNew ? newValue : (oldReport || oldSignal);
    }
}
