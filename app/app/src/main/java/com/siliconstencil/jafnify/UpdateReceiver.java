package com.siliconstencil.jafnify;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Build;
import android.util.Log;

/**
 * Brings a live sharing session back after an app update. Installing an
 * update kills the process, so the clean teardown never runs and
 * {@link SharerService#KEY_SHARING_LIVE} stays true. The system sends
 * ACTION_MY_PACKAGE_REPLACED to the app itself, it is one of the documented
 * exemptions from the background foreground-service start ban, and it is not
 * on the Android 15 type ban list. A user Stop still wins, and a session that
 * was not live before the install is never started.
 */
public class UpdateReceiver extends BroadcastReceiver {
    private static final String TAG = "JafnifyUpdate";

    @Override
    public void onReceive(Context context, Intent intent) {
        if (intent == null
                || !Intent.ACTION_MY_PACKAGE_REPLACED.equals(intent.getAction())) {
            return;
        }
        try {
            SharedPreferences prefs = context.getSharedPreferences(
                    SharerService.PREFS, Context.MODE_PRIVATE);
            boolean sharingWasLive = prefs.getBoolean(
                    SharerService.KEY_SHARING_LIVE, false);
            boolean stopped = prefs.getBoolean(SharerService.KEY_USER_STOPPED, false);
            boolean terms = Terms.accepted(prefs.getInt(Terms.PREF_KEY, 0));
            boolean hasPair = hasStoredPair(context);
            if (!StartPolicy.shouldStart(StartPolicy.Trigger.UPDATE, sharingWasLive,
                    hasPair, terms, stopped)) {
                return;
            }
            Intent start = new Intent(context, SharerService.class);
            start.setAction(SharerService.ACTION_START);
            start.putExtra(SharerService.EXTRA_START_REASON,
                    StartReason.UPDATE.wire());
            if (Build.VERSION.SDK_INT >= 26) {
                context.startForegroundService(start);
            } else {
                context.startService(start);
            }
        } catch (RuntimeException | LinkageError e) {
            // An OEM build may still refuse the start; log once and stop. The
            // user can open the app as usual.
            Log.w(TAG, "update restart skipped", e);
        }
    }

    private static boolean hasStoredPair(Context context) {
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
