package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.provider.Settings;

/**
 * Opens the hotspot controls. The tether screen is not public API, so the
 * wireless settings are the fallback; the app's own home is the last resort,
 * so a tap can never crash on a device that has neither.
 */
final class HotspotSettings {
    private static final String TETHER_ACTION = "android.settings.TETHER_SETTINGS";

    private HotspotSettings() {
    }

    static Intent intent(Context context) {
        Intent tether = new Intent(TETHER_ACTION);
        if (resolves(context, tether)) {
            return tether;
        }
        Intent wireless = new Intent(Settings.ACTION_WIRELESS_SETTINGS);
        if (resolves(context, wireless)) {
            return wireless;
        }
        return new Intent(context, MainActivity.class);
    }

    private static boolean resolves(Context context, Intent intent) {
        try {
            PackageManager pm = context.getPackageManager();
            return pm != null && intent.resolveActivity(pm) != null;
        } catch (RuntimeException e) {
            return false;
        }
    }
}
