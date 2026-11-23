package com.siliconstencil.jafnify;

import android.app.ActivityManager;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.net.ConnectivityManager;
import android.os.BatteryManager;
import android.os.Build;
import android.os.PowerManager;
import android.os.SystemClock;

/**
 * The phone's own facts for the device panel: the maker, the model, the
 * Android version, the app version, the battery, the limits Android puts on
 * Jafnify and the memory. The facts go to the core as one blob; the core owns
 * the wording. Nothing is sent anywhere: the panel reads the phone itself and
 * asks for no new permission.
 */
public final class DeviceFacts {
    private DeviceFacts() {
    }

    /** The facts of this phone as one blob: key=value;key=value. */
    public static String collect(Context context) {
        StringBuilder blob = new StringBuilder();
        put(blob, "maker", Build.MANUFACTURER);
        put(blob, "model", Build.MODEL);
        put(blob, "android", Build.VERSION.RELEASE);
        put(blob, "sdk", String.valueOf(Build.VERSION.SDK_INT));
        put(blob, "app_version", versionName(context));

        Intent battery = battery(context);
        if (battery != null) {
            int level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
            int scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
            if (level >= 0 && scale > 0) {
                put(blob, "battery_pct", String.valueOf(level * 100 / scale));
            }
            int status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
            if (status == BatteryManager.BATTERY_STATUS_CHARGING
                    || status == BatteryManager.BATTERY_STATUS_FULL) {
                put(blob, "charging", "true");
            } else if (status != -1) {
                put(blob, "charging", "false");
            }
            int temp = battery.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Integer.MIN_VALUE);
            if (temp != Integer.MIN_VALUE) {
                put(blob, "temp_c_x10", String.valueOf(temp));
            }
        }

        PowerManager power = (PowerManager) context.getSystemService(Context.POWER_SERVICE);
        if (power != null) {
            put(blob, "battery_saver", String.valueOf(power.isPowerSaveMode()));
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                put(blob, "thermal", String.valueOf(power.getCurrentThermalStatus()));
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                put(blob, "exempt_from_optimisation", String.valueOf(
                        power.isIgnoringBatteryOptimizations(context.getPackageName())));
            }
        }

        ActivityManager manager = (ActivityManager)
                context.getSystemService(Context.ACTIVITY_SERVICE);
        if (manager != null) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                put(blob, "background_restricted",
                        String.valueOf(manager.isBackgroundRestricted()));
            }
            ActivityManager.MemoryInfo memory = new ActivityManager.MemoryInfo();
            manager.getMemoryInfo(memory);
            put(blob, "mem_free_mb", String.valueOf(memory.availMem / (1024L * 1024L)));
            put(blob, "mem_total_mb", String.valueOf(memory.totalMem / (1024L * 1024L)));
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            ConnectivityManager connectivity = (ConnectivityManager)
                    context.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (connectivity != null) {
                put(blob, "data_saver",
                        String.valueOf(connectivity.getRestrictBackgroundStatus()));
            }
        }

        put(blob, "uptime_secs", String.valueOf(SystemClock.uptimeMillis() / 1000L));
        return blob.toString();
    }

    /** The app version of this phone, empty when the system does not know it. */
    private static String versionName(Context context) {
        try {
            return context.getPackageManager()
                    .getPackageInfo(context.getPackageName(), 0).versionName;
        } catch (PackageManager.NameNotFoundException e) {
            return "";
        }
    }

    /** The sticky battery broadcast, or null when the system has none. */
    private static Intent battery(Context context) {
        try {
            return context.registerReceiver(
                    null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        } catch (RuntimeException e) {
            return null;
        }
    }

    /** Adds one fact; an empty value leaves the key out. */
    private static void put(StringBuilder blob, String key, String value) {
        if (value == null || value.isEmpty()) {
            return;
        }
        String clean = value.replace(';', ' ').replace('=', ' ').trim();
        if (clean.isEmpty()) {
            return;
        }
        if (blob.length() > 0) {
            blob.append(';');
        }
        blob.append(key).append('=').append(clean);
    }
}
