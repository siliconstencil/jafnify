package com.siliconstencil.jafnify;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.view.View;

/**
 * The start-sharing gate shared by the home screen and Devices: the one-time
 * terms, the Android 13+ notification rationale and the service start. The
 * Activity owns the permission request; this helper owns the decision so both
 * screens behave the same.
 */
final class SharingActions {
    private static final int REQ_POST_NOTIFICATIONS = 1;
    private static final String PREF_NOTIF_PROMPTED = "notif_prompted";

    private final Activity activity;
    private String pendingAction = SharerService.ACTION_START;
    private boolean notifPrompted;

    SharingActions(Activity activity) {
        this.activity = activity;
        this.notifPrompted = activity
                .getSharedPreferences(SharerService.PREFS, Activity.MODE_PRIVATE)
                .getBoolean(PREF_NOTIF_PROMPTED, false);
    }

    /** Starts after the terms are accepted, asking once for notifications. */
    void start(String action) {
        start(action, null);
    }

    /**
     * Starts after the terms are accepted. A declined or dismissed terms
     * dialog runs {@code onCancelled}: a start that never happens must not
     * leave the caller saying "Starting..." for ever.
     */
    void start(String action, Runnable onCancelled) {
        if (!Terms.accepted(activity
                .getSharedPreferences(SharerService.PREFS, Activity.MODE_PRIVATE)
                .getInt(Terms.PREF_KEY, 0))) {
            showTerms(action, onCancelled);
            return;
        }
        pendingAction = action;
        if (UiText.shouldAskNotifications(Build.VERSION.SDK_INT >= 33,
                notificationPermissionGranted(), notifPrompted)) {
            showNotifRationale(action);
            return;
        }
        startService(action);
    }

    /** Returns true when the result belongs to this helper. */
    boolean onRequestPermissionsResult(int requestCode) {
        if (requestCode != REQ_POST_NOTIFICATIONS) {
            return false;
        }
        markNotifPrompted();
        startService(pendingAction);
        return true;
    }

    /** The inline warning banner after a denied prompt, where the screen has one. */
    void refreshNotifNote(View banner) {
        if (banner == null) {
            return;
        }
        boolean show = UiText.notificationsOffNote(Build.VERSION.SDK_INT >= 33,
                notificationPermissionGranted(), notifPrompted);
        banner.setVisibility(show ? View.VISIBLE : View.GONE);
    }

    static void openNotificationSettings(Activity activity) {
        if (Build.VERSION.SDK_INT >= 26) {
            Intent i = new Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE,
                            activity.getPackageName());
            try {
                activity.startActivity(i);
                return;
            } catch (android.content.ActivityNotFoundException e) {
                // Fall through to the app details screen.
            }
        }
        activity.startActivity(new Intent(
                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                android.net.Uri.parse("package:" + activity.getPackageName())));
    }

    private boolean notificationPermissionGranted() {
        return Build.VERSION.SDK_INT < 33
                || activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                        == PackageManager.PERMISSION_GRANTED;
    }

    private void showNotifRationale(final String action) {
        new AlertDialog.Builder(activity)
                .setTitle(R.string.notif_rationale_title)
                .setMessage(R.string.notif_rationale_message)
                .setNegativeButton(R.string.notif_not_now,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                markNotifPrompted();
                                startService(action);
                            }
                        })
                .setPositiveButton(R.string.notif_continue,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                pendingAction = action;
                                if (Build.VERSION.SDK_INT < 33) {
                                    // The prompt exists from API 33 on; older
                                    // systems need no permission to notify.
                                    startService(action);
                                    return;
                                }
                                activity.requestPermissions(
                                        new String[] {Manifest.permission.POST_NOTIFICATIONS},
                                        REQ_POST_NOTIFICATIONS);
                            }
                        })
                // Dismissing the rationale is the same as "Not now": the user
                // asked to share, and notifications are optional.
                .setOnCancelListener(new DialogInterface.OnCancelListener() {
                    @Override
                    public void onCancel(DialogInterface dialog) {
                        markNotifPrompted();
                        startService(action);
                    }
                })
                .show();
    }

    private void showTerms(final String action, final Runnable onCancelled) {
        new AlertDialog.Builder(activity)
                .setTitle(R.string.terms_title)
                .setMessage(R.string.terms_body)
                .setNegativeButton(R.string.terms_decline,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                if (onCancelled != null) {
                                    onCancelled.run();
                                }
                            }
                        })
                .setPositiveButton(R.string.terms_accept,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                activity.getSharedPreferences(SharerService.PREFS,
                                                Activity.MODE_PRIVATE)
                                        .edit()
                                        .putInt(Terms.PREF_KEY, Terms.VERSION)
                                        .apply();
                                start(action, onCancelled);
                            }
                        })
                .setOnCancelListener(new DialogInterface.OnCancelListener() {
                    @Override
                    public void onCancel(DialogInterface dialog) {
                        if (onCancelled != null) {
                            onCancelled.run();
                        }
                    }
                })
                .show();
    }

    private void markNotifPrompted() {
        notifPrompted = true;
        activity.getSharedPreferences(SharerService.PREFS, Activity.MODE_PRIVATE)
                .edit().putBoolean(PREF_NOTIF_PROMPTED, true).apply();
    }

    private void startService(String action) {
        Intent i = new Intent(activity, SharerService.class).setAction(action);
        if (Build.VERSION.SDK_INT >= 26) {
            activity.startForegroundService(i);
        } else {
            activity.startService(i);
        }
    }
}
