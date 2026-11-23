package com.siliconstencil.jafnify;

import android.Manifest;
import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.os.BatteryManager;
import android.os.Build;
import android.telephony.CellInfo;
import android.telephony.CellSignalStrength;
import android.telephony.SignalStrength;
import android.telephony.TelephonyCallback;
import android.telephony.TelephonyDisplayInfo;
import android.telephony.TelephonyManager;

import java.util.List;

/**
 * The phone's own network facts: the modem generation, the signal level, the
 * signal strength in dBm, the charge counter and the charging flag.
 *
 * <p>Nothing is polled and no query is sent: the generation and the signal
 * arrive as events from the system. The permission is only read, never asked
 * for here; the settings screen asks once, from the switch.
 */
final class PhoneNetworkMonitor {
    private static final String KIND_LTE = "lte";
    private static final String KIND_LTE_CA = "lte_ca";
    private static final String KIND_LTE_ADVANCED_PRO = "lte_advanced_pro";
    private static final String KIND_NR = "nr";
    private static final String KIND_NR_NSA = "nr_nsa";
    private static final String KIND_NR_NSA_MMWAVE = "nr_nsa_mmwave";
    private static final String KIND_NR_ADVANCED = "nr_advanced";

    private final Context context;
    private final NetworkSampler sampler;
    private TelephonyCallback callback;
    private String lastWord = "";
    private int signalLevel = -1;
    private int dbm = -1;

    PhoneNetworkMonitor(Context context, NetworkSampler sampler) {
        this.context = context;
        this.sampler = sampler;
    }

    /** A monitor for a screen: it reads the phone, it stores nothing. */
    PhoneNetworkMonitor(Context context) {
        this(context, null);
    }

    /** True when the app may read the network generation and the signal. */
    boolean permissionGranted() {
        if (Build.VERSION.SDK_INT < 23) {
            return false;
        }
        try {
            return context.checkSelfPermission(Manifest.permission.READ_PHONE_STATE)
                    == PackageManager.PERMISSION_GRANTED;
        } catch (RuntimeException e) {
            return false;
        }
    }

    /** The core wire word of the last generation, empty when unknown. */
    String generationWord() {
        return lastWord;
    }

    /** The last signal level 0..4, or -1 before the first update. */
    int signalLevel() {
        return signalLevel;
    }

    /** The last signal strength in dBm, or -1 when the modem has none. */
    int dbm() {
        return dbm;
    }

    /**
     * The first valid dBm of a list of cell strengths; -1 when there is none.
     * The modem reports {@link CellInfo#UNAVAILABLE} for a strength it does
     * not know, so those entries are skipped.
     */
    static int pickDbm(int[] dbms) {
        return BestSpot.pickDbm(dbms);
    }

    /** Listen for the generation and the signal; the callback is registered once. */
    void start() {
        if (callback != null || Build.VERSION.SDK_INT < 31) {
            return;
        }
        TelephonyManager tm = telephony();
        if (tm == null) {
            return;
        }
        try {
            callback = new Callback();
            tm.registerTelephonyCallback(context.getMainExecutor(), callback);
        } catch (RuntimeException e) {
            // A vendor that refuses the callback must not break the service.
            callback = null;
        }
    }

    /** Stop listening. */
    void stop() {
        if (callback == null || Build.VERSION.SDK_INT < 31) {
            return;
        }
        try {
            TelephonyManager tm = telephony();
            if (tm != null) {
                tm.unregisterTelephonyCallback(callback);
            }
        } catch (RuntimeException ignored) {
            // The callback may already be gone; nothing to do.
        }
        callback = null;
        lastWord = "";
        signalLevel = -1;
        dbm = -1;
    }

    /** The charge counter in mAh, or -1 when the system has no sane value. */
    long chargeCounterMah() {
        try {
            BatteryManager bm = (BatteryManager) context
                    .getSystemService(Context.BATTERY_SERVICE);
            if (bm == null) {
                return -1;
            }
            int value = bm.getIntProperty(
                    BatteryManager.BATTERY_PROPERTY_CHARGE_COUNTER);
            if (value == Integer.MIN_VALUE) {
                return -1;
            }
            return value;
        } catch (RuntimeException e) {
            return -1;
        }
    }

    /** True when the phone is charging right now. */
    boolean charging() {
        Intent battery = stickyBattery();
        if (battery == null) {
            return false;
        }
        int status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
        return status == BatteryManager.BATTERY_STATUS_CHARGING
                || status == BatteryManager.BATTERY_STATUS_FULL;
    }

    /** The battery level in percent, or -1 when the system has no value. */
    int batteryPercent() {
        Intent battery = stickyBattery();
        if (battery == null) {
            return -1;
        }
        int level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
        int scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
        return level >= 0 && scale > 0 ? level * 100 / scale : -1;
    }

    private TelephonyManager telephony() {
        return (TelephonyManager) context.getSystemService(Context.TELEPHONY_SERVICE);
    }

    private Intent stickyBattery() {
        try {
            return context.registerReceiver(
                    null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        } catch (RuntimeException e) {
            return null;
        }
    }

    /** One TelephonyCallback subclass; the API has no multi-listener constructor. */
    @TargetApi(31)
    private final class Callback extends TelephonyCallback
            implements TelephonyCallback.DisplayInfoListener,
            TelephonyCallback.SignalStrengthsListener {
        @Override
        public void onDisplayInfoChanged(TelephonyDisplayInfo info) {
            String kind = kindOf(info);
            if (kind == null) {
                return;
            }
            lastWord = Native.generationWord(kind);
            if (sampler != null) {
                sampler.onGeneration(kind, System.currentTimeMillis());
            }
        }

        @Override
        public void onSignalStrengthsChanged(SignalStrength strength) {
            signalLevel = strength.getLevel();
            dbm = dbmOf(strength);
            if (sampler != null) {
                sampler.onSignal(strength.getLevel(), System.currentTimeMillis());
            }
        }
    }

    /** The first dBm the modem reports, or -1 when it reports none. */
    @TargetApi(29)
    private int dbmOf(SignalStrength strength) {
        List<CellSignalStrength> strengths;
        try {
            strengths = strength.getCellSignalStrengths();
        } catch (RuntimeException e) {
            return -1;
        }
        if (strengths == null || strengths.isEmpty()) {
            return -1;
        }
        int[] values = new int[strengths.size()];
        for (int i = 0; i < values.length; i++) {
            values[i] = strengths.get(i).getDbm();
        }
        return pickDbm(values);
    }

    /** The canonical kind of one display info update, or null when unknown. */
    @TargetApi(30)
    private String kindOf(TelephonyDisplayInfo info) {
        if (info == null) {
            return null;
        }
        int override = info.getOverrideNetworkType();
        // Only read the data network type when the override does not decide.
        if (kindFor(override, TelephonyManager.NETWORK_TYPE_UNKNOWN) != null) {
            return kindFor(override, TelephonyManager.NETWORK_TYPE_UNKNOWN);
        }
        return kindFor(override, dataNetworkType());
    }

    /**
     * Pure mapping from the modem's override and data network type to the core
     * kind. Uses the SDK constants (compile-time ints): LTE_CA=1,
     * LTE_ADVANCED_PRO=2, NR_NSA=3, NR_NSA_MMWAVE=4, NR_ADVANCED=5. A Verizon
     * "5G UW" icon is NR_ADVANCED.
     */
    static String kindFor(int override, int dataType) {
        switch (override) {
            case TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_CA:
                return KIND_LTE_CA;
            case TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_ADVANCED_PRO:
                return KIND_LTE_ADVANCED_PRO;
            case TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA:
                return KIND_NR_NSA;
            case TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA_MMWAVE:
                return KIND_NR_NSA_MMWAVE;
            case TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_ADVANCED:
                return KIND_NR_ADVANCED;
            default:
                break;
        }
        if (dataType == TelephonyManager.NETWORK_TYPE_NR) {
            return KIND_NR;
        }
        if (dataType == TelephonyManager.NETWORK_TYPE_LTE) {
            return KIND_LTE;
        }
        return null;
    }

    /** The data network type; a SecurityException means "not allowed". */
    @SuppressLint("MissingPermission")
    @TargetApi(31)
    private int dataNetworkType() {
        TelephonyManager tm = telephony();
        if (tm == null) {
            return TelephonyManager.NETWORK_TYPE_UNKNOWN;
        }
        try {
            return tm.getDataNetworkType();
        } catch (RuntimeException e) {
            return TelephonyManager.NETWORK_TYPE_UNKNOWN;
        }
    }
}
