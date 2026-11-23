package com.siliconstencil.jafnify;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.util.Log;

/**
 * Starts sharing after the phone boots, when the user opted in and a pair is
 * stored. The receiver is not direct-boot aware, so BOOT_COMPLETED arrives
 * after the first unlock. A failure is logged and dropped, never retried in a
 * loop.
 */
public class BootReceiver extends BroadcastReceiver {
    private static final String TAG = "JafnifyBoot";

    @Override
    public void onReceive(Context context, Intent intent) {
        if (intent == null
                || !Intent.ACTION_BOOT_COMPLETED.equals(intent.getAction())) {
            return;
        }
        try {
            if (!Autostart.loadStartAfterRestart(Autostart.shared(context))) {
                return;
            }
            boolean hasPair = hasStoredPair(context);
            boolean terms = Terms.accepted(context
                    .getSharedPreferences(SharerService.PREFS, Context.MODE_PRIVATE)
                    .getInt(Terms.PREF_KEY, 0));
            boolean stopped = context
                    .getSharedPreferences(SharerService.PREFS, Context.MODE_PRIVATE)
                    .getBoolean(SharerService.KEY_USER_STOPPED, false);
            if (!StartPolicy.shouldStart(StartPolicy.Trigger.BOOT, true,
                    hasPair, terms, stopped)) {
                return;
            }
            Intent start = new Intent(context, SharerService.class);
            start.setAction(SharerService.ACTION_START);
            start.putExtra(SharerService.EXTRA_START_REASON,
                    StartReason.BOOT.wire());
            if (Build.VERSION.SDK_INT >= 26) {
                context.startForegroundService(start);
            } else {
                context.startService(start);
            }
        } catch (RuntimeException | LinkageError e) {
            // API 31+ can refuse a background foreground-service start, and
            // some OEM builds add their own refusals. Log once and stop; the
            // user can still open the app.
            Log.w(TAG, "boot start skipped", e);
        }
    }

    private static boolean hasStoredPair(Context context) {
        // ensure() returns the instance handle, 0 on failure. Only an instance
        // this check created is destroyed; one the app already holds stays.
        long before = Native.instance;
        long instance = Native.ensure(context);
        if (instance == 0) {
            return false;
        }
        try {
            return !Native.pairedName(instance).isEmpty();
        } finally {
            if (before == 0) {
                Native.destroy();
            }
        }
    }
}
