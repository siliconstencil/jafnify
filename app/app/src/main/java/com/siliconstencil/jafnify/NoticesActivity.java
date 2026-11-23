package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;
import android.widget.ArrayAdapter;
import android.widget.ListView;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.List;

/**
 * The generated third-party notices, in small chunks in one list. The asset is
 * staged by tools/release/android.sh; a debug build without it never shows this
 * screen because the About row is hidden then.
 *
 * The file is 3.3 MB, so it is read and split off the main thread: no single
 * TextView lays out the whole file.
 */
public class NoticesActivity extends Activity {
    static final String ASSET = "THIRD_PARTY_NOTICES.txt";

    private ThemeMode appliedTheme;
    private ArrayAdapter<String> adapter;

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_notices);
        ScreenHeader.bind(this, Native.thirdPartyNoticesTitle());
        SystemBars.apply(this);
        ListView list = findViewById(R.id.listNotices);
        adapter = new ArrayAdapter<>(this, R.layout.row_notices, R.id.tChunk,
                new ArrayList<String>());
        list.setAdapter(adapter);
        load();
    }

    private void load() {
        new Thread(new Runnable() {
            @Override
            public void run() {
                final List<String> chunks = NoticesText.sections(read());
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        adapter.addAll(chunks);
                    }
                });
            }
        }, "notices").start();
    }

    private String read() {
        StringBuilder out = new StringBuilder();
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(
                getAssets().open(ASSET), "UTF-8"))) {
            String line;
            while ((line = reader.readLine()) != null) {
                out.append(line).append('\n');
            }
        } catch (IOException e) {
            return "";
        }
        return out.toString();
    }
}
