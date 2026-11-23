package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;
import android.widget.RadioGroup;
import android.widget.TextView;

import java.io.File;
import java.util.List;
import java.util.TimeZone;

/** Today, this week and this month of local usage, from the stored buckets. */
public class StatisticsActivity extends Activity {
    private ThemeMode appliedTheme;
    private UsageLineStore store;
    private RadioGroup windows;
    private TextView lines;
    private TextView network;

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // Usage statistics are standard now; the screen is always reachable.
        setContentView(R.layout.activity_statistics);
        ScreenHeader.bind(this, R.string.statistics_title);
        SystemBars.apply(this);

        store = new UsageLineStore(new File(getFilesDir(), "phone-stats.tsv"));
        windows = findViewById(R.id.statWindows);
        lines = findViewById(R.id.statLines);
        network = findViewById(R.id.statNetwork);
        windows.check(R.id.statToday);
        windows.setOnCheckedChangeListener(new RadioGroup.OnCheckedChangeListener() {
            @Override
            public void onCheckedChanged(RadioGroup group, int checkedId) {
                render();
            }
        });
        render();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (windows != null) {
            render();
        }
    }

    private void render() {
        if (store == null || lines == null) {
            return;
        }
        int days;
        if (windows.getCheckedRadioButtonId() == R.id.statWeek) {
            days = 7;
        } else if (windows.getCheckedRadioButtonId() == R.id.statMonth) {
            days = 30;
        } else {
            days = 1;
        }
        long now = System.currentTimeMillis();
        int offsetMinutes = TimeZone.getDefault().getOffset(now) / 60_000;
        List<String> stored = store.load();
        StringBuilder text = new StringBuilder();
        for (String line : stored) {
            if (text.length() > 0) {
                text.append('\n');
            }
            text.append(line);
        }
        String summary = Native.usageSummary(text.toString(), days, now, offsetMinutes);
        lines.setText(summary == null ? "" : summary);
        renderNetwork(now, offsetMinutes);
    }

    private void renderNetwork(long now, int offsetMinutes) {
        if (network == null) {
            return;
        }
        boolean detailsOn = FlagValues.networkDetails(Autostart.shared(this));
        if (!detailsOn) {
            network.setVisibility(TextView.GONE);
            return;
        }
        network.setVisibility(TextView.VISIBLE);
        String report = Native.networkReport(
                join(store.loadNetwork()),
                join(store.loadSignal()),
                now, offsetMinutes);
        network.setText(getString(R.string.statistics_network_block, report));
    }

    private static String join(List<String> values) {
        StringBuilder text = new StringBuilder();
        for (String value : values) {
            if (text.length() > 0) {
                text.append('\n');
            }
            text.append(value);
        }
        return text.toString();
    }
}
