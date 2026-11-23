package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;

/**
 * The best-spot finder: the phone's own signal and delay once a second, so
 * the user can move the phone, mark a spot and see which spot was the best.
 * The samples live in memory only and the screen keeps the screen on while
 * it is open. A session ends after the length the core gives it, and the
 * user can stop it earlier. Every line comes from the core.
 */
public class BestSpotActivity extends Activity {
    private static final long TICK_MS = 1000;
    /** Where the session state sits in the bundle, over a rotation. */
    private static final String STATE_SESSION = "best_spot_session";

    private ThemeMode appliedTheme;
    private BestSpot session;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private PhoneNetworkMonitor monitor;
    private SharerService service;
    private boolean bound;
    private TextView live;
    private TextView spots;
    private TextView verdict;

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

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (session == null || !session.running()) {
                return;
            }
            long now = System.currentTimeMillis();
            if (session.expired(now)) {
                // The session has run its length: it ends by itself and the
                // timer stops with it.
                session.stop();
                render();
                return;
            }
            collect(now);
            render();
            handler.postDelayed(this, TICK_MS);
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
        if (!Autostart.shared(this).getBoolean("best_spot", false)) {
            finish();
            return;
        }
        setContentView(R.layout.activity_best_spot);
        ScreenHeader.bind(this, R.string.best_spot_title);
        SystemBars.apply(this);

        live = findViewById(R.id.spotLive);
        spots = findViewById(R.id.spotList);
        verdict = findViewById(R.id.spotVerdict);
        monitor = new PhoneNetworkMonitor(this);
        // The core owns the length of a session; the phone only asks.
        session = new BestSpot(Native.bestSpotSessionMs());
        if (savedInstanceState != null) {
            // A rotation must not lose a running session.
            session.restoreState(savedInstanceState.getStringArray(STATE_SESSION));
        }

        findViewById(R.id.spotStart).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startSession();
            }
        });
        findViewById(R.id.spotStop).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                stopSession();
            }
        });
        findViewById(R.id.spotMark).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                session.mark(System.currentTimeMillis());
                render();
            }
        });
        render();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        if (session != null) {
            outState.putStringArray(STATE_SESSION, session.saveState());
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        // The service only carries the last summary line (the delay); the
        // screen works without it.
        bound = bindService(new Intent(this, SharerService.class),
                connection, Context.BIND_AUTO_CREATE);
    }

    @Override
    protected void onResume() {
        super.onResume();
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (monitor != null && monitor.permissionGranted()) {
            monitor.start();
        }
        if (session != null && session.running()) {
            handler.post(tick);
        }
    }

    @Override
    protected void onPause() {
        handler.removeCallbacks(tick);
        if (monitor != null) {
            monitor.stop();
        }
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        super.onPause();
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

    /** Start the session: the old samples are gone and the timer runs. */
    private void startSession() {
        session.start(System.currentTimeMillis());
        handler.removeCallbacks(tick);
        handler.post(tick);
    }

    /** Stop the session: the samples stay, the timer stops. */
    private void stopSession() {
        session.stop();
        handler.removeCallbacks(tick);
        render();
    }

    /** One sample of this phone: the network, the signal and the delay. */
    private void collect(long now) {
        int level = monitor == null ? -1 : monitor.signalLevel();
        if (level < 0) {
            return;
        }
        int dbm = monitor == null ? -1 : monitor.dbm();
        int rtt = service == null ? -1
                : BestSpot.rttFromSummary(service.phoneSummaryLine());
        session.addSample(BestSpot.sampleLine(now, netWord(), level, dbm, rtt),
                now);
    }

    /** The core wire word of the network this phone is on. */
    private String netWord() {
        String transport = PhoneSampler.networkWord(transport());
        if (!"cellular".equals(transport)) {
            return transport;
        }
        String generation = monitor == null ? "" : monitor.generationWord();
        return generation.isEmpty() ? "mobile" : generation;
    }

    /** The transport word of the active network. */
    private String transport() {
        ConnectivityManager connectivity = (ConnectivityManager)
                getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connectivity == null || Build.VERSION.SDK_INT < 23) {
            return "none";
        }
        Network active = connectivity.getActiveNetwork();
        NetworkCapabilities caps = active == null ? null
                : connectivity.getNetworkCapabilities(active);
        if (caps == null) {
            return "none";
        }
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
            return "wifi";
        }
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
            return "cellular";
        }
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) {
            return "ethernet";
        }
        return "none";
    }

    /** The core lines: the live line, the spot lines and the verdict. */
    private void render() {
        if (live == null) {
            return;
        }
        String text = Native.bestSpotLines(session.samplesBlob(), session.marksBlob());
        String[] parts = BestSpot.splitBlock(text);
        live.setText(parts[0]);
        spots.setText(parts[1]);
        verdict.setText(parts[2]);
    }
}
