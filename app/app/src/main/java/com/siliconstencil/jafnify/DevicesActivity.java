package com.siliconstencil.jafnify;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ComponentName;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Bundle;
import android.os.IBinder;
import android.view.View;
import android.widget.Button;
import android.widget.TextView;

/** The paired device: who it is, pair a new one, forget it. */
public class DevicesActivity extends Activity {
    private TextView tPairedName;
    private TextView tForgetError;
    private String pairedName = "";
    private SharerService service;
    private boolean bound = false;
    private SharingActions sharing;
    private ThemeMode appliedTheme;

    private final ServiceConnection connection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            service = ((SharerService.LocalBinder) binder).getService();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            service = null;
        }
    };

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_devices);
        ScreenHeader.bind(this, R.string.devices_title);
        SystemBars.apply(this);

        sharing = new SharingActions(this);

        tPairedName = findViewById(R.id.tPairedName);
        tForgetError = findViewById(R.id.tForgetError);
        Button pairNew = findViewById(R.id.bPairNew);
        Button forget = findViewById(R.id.bForget);
        pairNew.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                confirmPairNew();
            }
        });
        forget.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                confirmForget();
            }
        });
    }

    @Override
    protected void onStart() {
        super.onStart();
        bound = bindService(new Intent(this, SharerService.class),
                connection, Context.BIND_AUTO_CREATE);
    }

    @Override
    protected void onResume() {
        super.onResume();
        Clock.follow(this);
        refreshPaired();
    }

    @Override
    protected void onStop() {
        if (bound) {
            unbindService(connection);
            bound = false;
        }
        service = null;
        super.onStop();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        sharing.onRequestPermissionsResult(requestCode);
    }

    private void refreshPaired() {
        String name = Native.pairedName(Native.ensure(this));
        pairedName = name == null ? "" : name;
        tPairedName.setText(pairedName.isEmpty()
                ? getString(R.string.paired_none)
                : getString(R.string.paired_device_name, pairedName));
    }

    private void confirmPairNew() {
        String replaceMessage = getString(R.string.pair_new_message, pairedName);
        new AlertDialog.Builder(this)
                .setTitle(R.string.pair_new_title)
                .setMessage(DevicesText.pairNewMessage(pairedName, replaceMessage,
                        getString(R.string.pair_new_none_message)))
                .setNegativeButton(R.string.btn_cancel, null)
                .setPositiveButton(R.string.pair_new_confirm,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                sharing.start(SharerService.ACTION_PAIR_NEW);
                            }
                        })
                .show();
    }

    private void confirmForget() {
        new AlertDialog.Builder(this)
                .setTitle(R.string.forget_title)
                .setMessage(R.string.forget_message)
                .setNegativeButton(R.string.forget_cancel, null)
                .setPositiveButton(R.string.forget_confirm,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                forgetPeer();
                            }
                        })
                .show();
    }

    /** Works while sharing: the service stops the tunnel, then deletes the pair. */
    private void forgetPeer() {
        String error;
        if (service != null) {
            error = service.forgetPair();
            if (error == null || error.isEmpty()) {
                service.addLog("session", getString(R.string.log_forgotten));
            } else {
                service.addLog("error", getString(R.string.log_error, error));
            }
        } else {
            error = Native.forgetPeer(Native.ensure(this));
            if (error == null || error.isEmpty()) {
                startService(new Intent(this, SharerService.class)
                        .setAction(SharerService.ACTION_STOP));
            }
        }
        if (error != null && !error.isEmpty()) {
            tForgetError.setText(getString(R.string.log_error, error));
            tForgetError.setVisibility(View.VISIBLE);
            return;
        }
        tForgetError.setVisibility(View.GONE);
        getSharedPreferences(SharerService.PREFS, MODE_PRIVATE).edit()
                .remove(SharerService.KEY_LAST_CONNECTED).apply();
        refreshPaired();
    }
}
