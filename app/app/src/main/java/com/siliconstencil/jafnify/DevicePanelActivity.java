package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.view.View;
import android.widget.TextView;

/**
 * What this phone can do right now: the battery, the memory and the limits
 * Android puts on Jafnify. The lines come from the core, so the phone and the
 * laptop say the same. Nothing is sent anywhere.
 */
public class DevicePanelActivity extends Activity {
    private ThemeMode appliedTheme;
    private TextView lines;

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if (!Autostart.shared(this).getBoolean("device_panel", false)) {
            finish();
            return;
        }
        setContentView(R.layout.activity_device_panel);
        ScreenHeader.bind(this, R.string.device_panel_title);
        SystemBars.apply(this);

        lines = findViewById(R.id.panelLines);
        findViewById(R.id.bBatteryHelp).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(new Intent(DevicePanelActivity.this, SettingsActivity.class));
            }
        });
        render();
    }

    @Override
    protected void onResume() {
        super.onResume();
        render();
    }

    /** The core lines of the facts of this phone. */
    private void render() {
        if (lines == null) {
            return;
        }
        String text = Native.devicePanel(DeviceFacts.collect(this));
        lines.setText(text == null ? "" : text);
    }
}
