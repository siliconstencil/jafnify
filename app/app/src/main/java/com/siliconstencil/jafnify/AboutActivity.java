package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Bundle;
import android.view.View;
import android.widget.Button;
import android.widget.TextView;

/** Version, licence and an optional donation link opened in the browser. */
public class AboutActivity extends Activity {
    private ThemeMode appliedTheme;

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_about);
        ScreenHeader.bind(this, R.string.about_title);
        SystemBars.apply(this);

        TextView version = findViewById(R.id.tVersion);
        version.setText(getString(R.string.about_version, versionName()));

        // The notices row exists only when the release build staged the
        // asset; the words are the core's.
        final Button notices = findViewById(R.id.bNotices);
        if (hasNotices()) {
            notices.setText(Native.thirdPartyNoticesTitle());
            notices.setOnClickListener(new View.OnClickListener() {
                @Override
                public void onClick(View v) {
                    startActivity(new Intent(AboutActivity.this, NoticesActivity.class));
                }
            });
            notices.setVisibility(View.VISIBLE);
        } else {
            notices.setVisibility(View.GONE);
        }

        final String donateUrl = getString(R.string.donate_url).trim();
        Button donate = findViewById(R.id.bDonate);
        if (donateUrl.isEmpty()) {
            donate.setVisibility(View.GONE);
        } else {
            donate.setOnClickListener(new View.OnClickListener() {
                @Override
                public void onClick(View v) {
                    try {
                        startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(donateUrl)));
                    } catch (ActivityNotFoundException e) {
                        // No browser: the link is optional, so stay here.
                    }
                }
            });
        }
    }

    private String versionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (PackageManager.NameNotFoundException e) {
            return getString(R.string.about_version_unknown);
        }
    }

    private boolean hasNotices() {
        try {
            getAssets().open(NoticesActivity.ASSET).close();
            return true;
        } catch (java.io.IOException e) {
            return false;
        }
    }
}
