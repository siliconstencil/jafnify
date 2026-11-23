package com.siliconstencil.jafnify;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.NotificationManager;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.ServiceConnection;
import android.content.pm.ApplicationInfo;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.os.SystemClock;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.MenuItem;
import android.view.View;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.PopupMenu;
import android.widget.TextView;

import java.io.File;
import java.util.TimeZone;

/**
 * Sharer home, one card: the product name, a state glyph plus the live status
 * line, elapsed time and data counters, the pairing code while a code runs,
 * and one filled primary button. The overflow opens Devices, Settings,
 * Diagnostics and About; the technical details live on Diagnostics. The
 * Activity owns no running native state: it starts and stops the service with
 * intents and, while visible, binds to it and redraws from its state every
 * 500 ms. All wording that depends on native status text goes through
 * {@link UiText} and the core, never through new sentences in Java.
 */
public class MainActivity extends Activity {
    private static final long NETWORK_CHECK_MS = 2000;
    /** After this long with no running service a "Starting..." is not true. */
    private static final long STARTING_GRACE_MS = 3000;
    /** The exit timestamp whose "sharing stopped" line was already shown. */
    private static final String KEY_STOPPED_NOTICE = "stopped_notice_exit";
    /** The sharing run whose battery hint the user dismissed, across rotation. */
    private static final String STATE_BATTERY_HINT_RUN = "battery_hint_run";

    private TextView tIntro;
    private TextView tHint;
    private View heroBlock;
    private View headArcs;
    private View heroArcs;
    private TextView tStatus;
    private TextView tHealth;
    private TextView tNotice;
    private TextView tElapsed;
    private TextView tTraffic;
    private TextView tLast;
    private View pairBlock;
    private TextView tCode;
    private TextView tCodeExpiry;
    private Button bNewCode;
    private Button bCancel;
    private Button bToggle;
    private Button bDetails;
    private View detailsPanel;
    private android.widget.LinearLayout detailsRows;
    private View statusCard;
    private String lastDetailsKey = "";
    private ImageButton bMore;
    /** The mode this Activity was built with; another screen may change it. */
    private ThemeMode appliedTheme;

    private SharingActions sharing;
    private final Handler sharerPoller = new Handler(Looper.getMainLooper());
    private SharerService service;
    private PhoneNetworkMonitor networkMonitor;
    private boolean networkPermission;
    private boolean bound = false;
    private boolean uiRunning = false;
    /** A start was requested and the service has not reported running yet. */
    private boolean starting = false;
    /** Code of the running pairing; "" when idle or serving the stored pair. */
    private String currentCode = "";
    /** Display name of the paired device; "" when none is stored. */
    private String pairedName = "";
    private UiText.Parsed parsed = UiText.parse(null);
    /** Native display lines fetched with every poll; "" when not applicable. */
    private String codeExpiry = "";
    /** The core says the code on screen has outlived its lifetime. */
    private boolean codeExpired;
    private String healthLine = "";
    /** The handshake age, shown only in the expanded Details. */
    private String healthDetail = "";
    /** One line under the status: a system stop, or notifications off. */
    private String stoppedNotice = "";
    /** Details is an inline collapsible panel under the status card. */
    private boolean detailsVisible = false;
    // The tile home is the app: the card, the grid of tiles and the row of
    // buttons. The grid and the slot order come from the saved layout.
    private View tileHome;
    private View tileDot;
    private TextView tTileTitle;
    private TextView tTileSub;
    private TextView tTileNotice;
    private View batteryBanner;
    /** The sharing run for which the battery hint was dismissed; MIN = none. */
    private long dismissedBatteryHintRun = Long.MIN_VALUE;
    private Button bTileStop;
    private Button bTileHotspot;
    private View pairTileBox;
    private TextView tTileCode;
    private TextView tTileCodeExpiry;
    private Button bTileNewCode;
    private LinearLayout tileGrid;
    private Button bTileBestSpot;
    private Button bTilePhone;
    private Button bTileStats;
    private ImageButton bTileMore;
    /** The effective switch values; standard rows are always on. */
    private Autostart.Store flagStore;
    /** The saved layout of the home and the text it was parsed from. */
    private HomeLayout homeLayout;
    private String homeLayoutText = "";
    /** The tiles of the grid, built once per layout and filled on every poll. */
    private final java.util.List<TileViews> tileViews = new java.util.ArrayList<>();
    /** The rows of the grid, kept so an empty row can be hidden. */
    private final java.util.List<LinearLayout> tileRows = new java.util.ArrayList<>();
    /** The tone of the card dot last drawn; "" before the first draw. */
    private String cardTone = "";
    /** Whether the tile card shows the expired-code error (colour and icon). */
    private boolean tileExpiryError;
    /** Whether the classic card shows the expired-code error. */
    private boolean oldExpiryError;
    /** Whether the window keeps the screen on; the flag is set only on change. */
    private boolean keepScreenOn;
    /** When the user asked for a start; used to stop a stuck "Starting...". */
    private long startRequestedAt = 0;
    /** Whether the screen is started; the poll runs only while it is. */
    private boolean visible = false;
    /** The stored usage lines of the phone, one per hour. */
    private UsageLineStore usageStore;
    /** The facts of the phone; they change rarely, so they are not asked for on every poll. */
    private final PhoneFactsCache factsCache = new PhoneFactsCache();
    private final PhoneFactsCache usageCache = new PhoneFactsCache();
    /** Run id of the sharer this screen has already seen. */
    private long seenRun = Long.MIN_VALUE;
    private String lastStatusShown = "";
    /** The coarse live-region key last announced; moving seconds are excluded. */
    private String announcedKey = null;
    /** Raw text of the last start error; null = none (may be empty). */
    private String lastError = null;
    /** Cached setup check; interface enumeration is too costly for every poll. */
    private long networkCheckedAt = 0;
    private boolean networkReady = true;
    /** Cached charging state for the battery hint; the sticky read is 2 s old at most. */
    private long batteryCheckedAt = 0;
    private boolean chargingNow = true;
    /** The scope of every tile that can be tapped, one word per tile id. */
    private TileScopePrefs.Store tileScopes;
    /** The words that say a tile can be tapped; the core owns them. */
    private String tapHint = "";

    private final ServiceConnection connection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            service = ((SharerService.LocalBinder) binder).getService();
            pollNow();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            service = null;
        }
    };

    private final Runnable sharerPoll = new Runnable() {
        @Override
        public void run() {
            if (service != null) {
                String error = service.consumeError();
                if (error != null) {
                    lastError = error;
                    starting = false;
                    service.addLog("error", getString(R.string.log_error, error.isEmpty()
                            ? getString(R.string.log_error_unknown) : error));
                }
                if (service.isRunning()) {
                    long run = service.getRunSeq();
                    if (run != seenRun) {
                        seenRun = run;
                        onRunStarted(service.getCode());
                    }
                    parsed = UiText.parse(service.getStatusRaw());
                    codeExpiry = service.getCodeExpiry();
                    codeExpired = service.getCodeExpired();
                    healthLine = service.getHealthLine();
                    healthDetail = service.getHealthDetail();
                    onStatusChanged();
                    // Keep the notification on the same state as this card; the
                    // service's own poll covers the screen-off case.
                    service.refreshNotification();
                } else if (uiRunning) {
                    onRunStopped();
                }
            }
            // A start whose service never came up must not say "Starting..."
            // for ever: the card would contradict the idle notification.
            if (starting && !runningNow()
                    && SystemClock.elapsedRealtime() - startRequestedAt > STARTING_GRACE_MS) {
                starting = false;
            }
            if (visible) {
                render();
                sharerPoller.postDelayed(this, 500);
            }
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
        setContentView(R.layout.activity_main);
        SystemBars.apply(this);

        // Built here, never as a field initializer: the Activity is not
        // attached to a Context when its fields are constructed.
        sharing = new SharingActions(this);
        networkMonitor = new PhoneNetworkMonitor(this);

        tIntro = findViewById(R.id.tIntro);
        tHint = findViewById(R.id.tHint);
        heroBlock = findViewById(R.id.heroBlock);
        headArcs = findViewById(R.id.headArcs);
        heroArcs = findViewById(R.id.heroArcs);
        tStatus = findViewById(R.id.tStatus);
        tHealth = findViewById(R.id.tHealth);
        tNotice = findViewById(R.id.tNotice);
        tElapsed = findViewById(R.id.tElapsed);
        tTraffic = findViewById(R.id.tTraffic);
        tLast = findViewById(R.id.tLast);
        pairBlock = findViewById(R.id.pairBlock);
        tCode = findViewById(R.id.tCode);
        tCode.setLetterSpacing(UiText.CODE_TRACKING);
        tCodeExpiry = findViewById(R.id.tCodeExpiry);
        bNewCode = findViewById(R.id.bNewCode);
        bCancel = findViewById(R.id.bCancel);
        bToggle = findViewById(R.id.bToggle);
        bDetails = findViewById(R.id.bDetails);
        detailsPanel = findViewById(R.id.detailsPanel);
        detailsRows = findViewById(R.id.detailsRows);
        bMore = findViewById(R.id.bMore);
        statusCard = findViewById(R.id.statusCard);
        tileHome = findViewById(R.id.tileHome);
        tileDot = findViewById(R.id.tileDot);
        tTileTitle = findViewById(R.id.tTileTitle);
        tTileSub = findViewById(R.id.tTileSub);
        tTileNotice = findViewById(R.id.tTileNotice);
        batteryBanner = findViewById(R.id.batteryBanner);
        if (savedInstanceState != null) {
            dismissedBatteryHintRun = savedInstanceState.getLong(
                    STATE_BATTERY_HINT_RUN, Long.MIN_VALUE);
        }
        findViewById(R.id.tBatteryNote).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                openBatteryExemption();
            }
        });
        findViewById(R.id.bBatteryDismiss).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissedBatteryHintRun = currentRun();
                batteryBanner.setVisibility(View.GONE);
            }
        });
        bTileStop = findViewById(R.id.bTileStop);
        bTileHotspot = findViewById(R.id.bTileHotspot);
        pairTileBox = findViewById(R.id.pairTileBox);
        tTileCode = findViewById(R.id.tTileCode);
        tTileCode.setLetterSpacing(UiText.CODE_TRACKING);
        tTileCodeExpiry = findViewById(R.id.tTileCodeExpiry);
        bTileNewCode = findViewById(R.id.bTileNewCode);
        tileGrid = findViewById(R.id.tileGrid);
        bTileBestSpot = findViewById(R.id.bTileBestSpot);
        bTilePhone = findViewById(R.id.bTilePhone);
        bTileStats = findViewById(R.id.bTileStats);
        bTileMore = findViewById(R.id.bTileMore);
        // The tiles are built once per saved layout; every poll only fills
        // their text. A layout change rebuilds them on the next resume.
        flagStore = Autostart.shared(this);
        refreshHomeLayout();
        tileScopes = TileScopePrefs.shared(this);
        usageStore = new UsageLineStore(new File(getFilesDir(), "phone-stats.tsv"));
        // The text ticks every second (handshake age, retry countdown), so the
        // live region must not auto-announce it. The coarse announceKey below
        // decides when a screen reader actually speaks.
        tStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_NONE);
        tHealth.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_NONE);

        // The code shrinks to fit its width; the text size is set here because
        // autoSizeTextType needs API 26 and minSdk is 21.
        tCode.addOnLayoutChangeListener(new View.OnLayoutChangeListener() {
            @Override
            public void onLayoutChange(View v, int left, int top, int right, int bottom,
                    int oldLeft, int oldTop, int oldRight, int oldBottom) {
                if (right - left != oldRight - oldLeft) {
                    // A size set inside the layout pass is not measured again
                    // until the next one, which clipped the digits: post it.
                    tCode.post(new Runnable() {
                        @Override
                        public void run() {
                            applyCodeTextSize(tCode);
                        }
                    });
                }
            }
        });
        tTileCode.addOnLayoutChangeListener(new View.OnLayoutChangeListener() {
            @Override
            public void onLayoutChange(View v, int left, int top, int right, int bottom,
                    int oldLeft, int oldTop, int oldRight, int oldBottom) {
                if (right - left != oldRight - oldLeft) {
                    // A size set inside the layout pass is not measured again
                    // until the next one, which clipped the digits: post it.
                    tTileCode.post(new Runnable() {
                        @Override
                        public void run() {
                            applyCodeTextSize(tTileCode);
                        }
                    });
                }
            }
        });

        bToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (uiRunning || starting) {
                    stopSharing();
                } else {
                    startSharing(SharerService.ACTION_START);
                }
            }
        });
        bCancel.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                stopSharing();
            }
        });
        // A code is never swapped silently, but a fresh one is one tap away
        // while the current code still works.
        bNewCode.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startSharing(SharerService.ACTION_PAIR_NEW);
            }
        });
        bDetails.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                setDetailsVisible(!detailsVisible);
            }
        });
        bMore.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                showOverflow(v);
            }
        });
        // The tile home has its own action button: the same stop and start,
        // and the same overflow, so nothing of the old screen is lost.
        bTileStop.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (uiRunning || starting) {
                    stopSharing();
                } else {
                    startSharing(SharerService.ACTION_START);
                }
            }
        });
        // The card offers the same tether screen the hotspot-off alert does.
        bTileHotspot.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(HotspotSettings.intent(MainActivity.this));
            }
        });
        bTileNewCode.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startSharing(SharerService.ACTION_PAIR_NEW);
            }
        });
        bTileMore.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                showOverflow(v);
            }
        });
        bTileBestSpot.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(new Intent(MainActivity.this, BestSpotActivity.class));
            }
        });
        bTilePhone.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(new Intent(MainActivity.this, DevicePanelActivity.class));
            }
        });
        bTileStats.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(new Intent(MainActivity.this, StatisticsActivity.class));
            }
        });
        // Debug builds only: long-press the status card to run the discovery
        // probe (what the sharer hears, no tunnel, UDP 9797 untouched). The
        // title itself lives in the action bar, once.
        if ((getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0) {
            findViewById(R.id.statusCard).setOnLongClickListener(
                    new View.OnLongClickListener() {
                @Override
                public boolean onLongClick(View v) {
                    showProbe();
                    return true;
                }
            });
        }

        refreshPaired();
        maybeReadStoppedNotice();
        render();
        // A cold open starts a session only when the user opted in and a pair
        // is stored. Rotation/process-state restore never starts: a Stop must
        // stay a Stop.
        boolean startOnOpen = Autostart.load(Autostart.shared(this));
        boolean terms = Terms.accepted(getSharedPreferences(
                SharerService.PREFS, MODE_PRIVATE).getInt(Terms.PREF_KEY, 0));
        StartPolicy.Trigger trigger = savedInstanceState == null
                ? StartPolicy.Trigger.COLD_OPEN : StartPolicy.Trigger.RESTORE;
        if (StartPolicy.shouldStart(trigger, startOnOpen, !pairedName.isEmpty(),
                terms, false)) {
            startSharing(SharerService.ACTION_START);
        }
    }

    /**
     * A start was requested: show "Starting..." until the service reports,
     * and do not keep showing the previous run's error over the new attempt.
     */
    private void startSharing(String action) {
        starting = true;
        startRequestedAt = SystemClock.elapsedRealtime();
        lastError = null;
        stoppedNotice = "";
        sharing.start(action, new Runnable() {
            @Override
            public void run() {
                // The user declined the terms: no start was requested at all.
                starting = false;
                render();
            }
        });
        pollNow();
    }

    /** The overflow: four navigation entries, no new dependency. */
    private void showOverflow(View anchor) {
        PopupMenu menu = new PopupMenu(this, anchor);
        menu.getMenu().add(0, 1, 0, R.string.menu_devices);
        menu.getMenu().add(0, 2, 1, R.string.menu_settings);
        menu.getMenu().add(0, 5, 2, R.string.menu_statistics);
        if (switchOn("best_spot")) {
            menu.getMenu().add(0, 6, 3, R.string.menu_best_spot);
        }
        if (switchOn("device_panel")) {
            menu.getMenu().add(0, 7, 4, R.string.menu_device_panel);
        }
        menu.getMenu().add(0, 3, 5, R.string.menu_diagnostics);
        menu.getMenu().add(0, 4, 6, R.string.menu_about);
        menu.setOnMenuItemClickListener(new PopupMenu.OnMenuItemClickListener() {
            @Override
            public boolean onMenuItemClick(MenuItem item) {
                switch (item.getItemId()) {
                    case 1:
                        startActivity(new Intent(MainActivity.this, DevicesActivity.class));
                        return true;
                    case 2:
                        startActivity(new Intent(MainActivity.this, SettingsActivity.class));
                        return true;
                    case 5:
                        startActivity(new Intent(MainActivity.this, StatisticsActivity.class));
                        return true;
                    case 6:
                        startActivity(new Intent(MainActivity.this, BestSpotActivity.class));
                        return true;
                    case 7:
                        startActivity(new Intent(MainActivity.this, DevicePanelActivity.class));
                        return true;
                    case 3:
                        startActivity(new Intent(MainActivity.this, DiagnosticsActivity.class));
                        return true;
                    case 4:
                        startActivity(new Intent(MainActivity.this, AboutActivity.class));
                        return true;
                    default:
                        return false;
                }
            }
        });
        menu.show();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        sharing.onRequestPermissionsResult(requestCode);
    }

    @Override
    protected void onStart() {
        super.onStart();
        // BIND_AUTO_CREATE only creates an idle bound service; the sharer
        // itself runs only after a start action (and keeps running after we
        // unbind in onStop). The poll runs from here on, even before the
        // binding connects, so a service that starts late is still seen.
        visible = true;
        bound = bindService(new Intent(this, SharerService.class),
                connection, Context.BIND_AUTO_CREATE);
        pollNow();
    }

    @Override
    protected void onResume() {
        super.onResume();
        Clock.follow(this);
        refreshTheme();
        // Returning from Settings or the layout screen may have changed the
        // notification setting or the grid; both are picked up here.
        refreshHomeLayout();
        pollNow();
        render();
    }

    @Override
    protected void onStop() {
        visible = false;
        sharerPoller.removeCallbacks(sharerPoll);
        if (bound) {
            unbindService(connection);
            bound = false;
        }
        service = null;
        // The service keeps running: keep uiRunning/currentCode/seenRun so the
        // next onStart does not log the same run and its "Connected" again.
        starting = false;
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        keepScreenOn = false;
        super.onStop();
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        out.putLong(STATE_BATTERY_HINT_RUN, dismissedBatteryHintRun);
    }

    /**
     * The screen stays on while Jafnify shares and the switch is on. The
     * window flag is enough: no wake lock, no permission. The flag goes when
     * the sharing stops, when the switch goes off or when the screen leaves.
     */
    private void applyKeepScreenOn() {
        boolean keep = uiRunning
                && Autostart.shared(this).getBoolean("keep_screen_on", false);
        if (keep == keepScreenOn) {
            return;
        }
        keepScreenOn = keep;
        if (keep) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
    }

    /** Recreates this Activity when the Settings screen changed the theme. */
    private void refreshTheme() {
        if (isFinishing()) {
            return;
        }
        ThemeMode stored = ThemePrefs.load(ThemePrefs.shared(this));
        if (stored != appliedTheme) {
            recreate();
        }
    }

    /** The cached setup check used by the 500 ms render loop. */
    private boolean networkReady() {
        long now = SystemClock.elapsedRealtime();
        if (now - networkCheckedAt >= NETWORK_CHECK_MS) {
            networkCheckedAt = now;
            networkReady = !UiText.noNetwork(NetIfaces.hasUplink(this));
        }
        return networkReady;
    }

    /** The live sharing run, or seenRun before the first poll fills it. */
    private long currentRun() {
        return service != null && service.isRunning() ? service.getRunSeq() : seenRun;
    }

    /** On external power. Unknown counts as charging, so it never nags. */
    private boolean isCharging() {
        long now = SystemClock.elapsedRealtime();
        if (now - batteryCheckedAt < NETWORK_CHECK_MS) {
            return chargingNow;
        }
        batteryCheckedAt = now;
        Intent battery = registerReceiver(
                null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        int status = battery == null ? -1
                : battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
        chargingNow = status == -1
                || status == BatteryManager.BATTERY_STATUS_CHARGING
                || status == BatteryManager.BATTERY_STATUS_FULL;
        return chargingNow;
    }

    /** Whether Android already exempts the app from battery optimisation. */
    private boolean ignoringOptimisations() {
        if (Build.VERSION.SDK_INT < 23) {
            return true;
        }
        PowerManager power = (PowerManager) getSystemService(Context.POWER_SERVICE);
        return power == null || power.isIgnoringBatteryOptimizations(getPackageName());
    }

    /**
     * The system battery-optimisation list. The app never asks for the
     * exemption with its own dialog (Play restricts that); the user makes the
     * choice on the documented screen. App details is the fallback.
     */
    private void openBatteryExemption() {
        if (startSettings(new Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))) {
            return;
        }
        startSettings(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.parse("package:" + getPackageName())));
    }

    private boolean startSettings(Intent intent) {
        try {
            startActivity(intent);
            return true;
        } catch (ActivityNotFoundException e) {
            return false;
        }
    }

    private void pollNow() {
        sharerPoller.removeCallbacks(sharerPoll);
        if (visible) {
            sharerPoller.post(sharerPoll);
        }
    }

    /** Whether the sharer is running, from the bound service or the last poll. */
    private boolean runningNow() {
        return uiRunning || (service != null && service.isRunning());
    }

    private void stopSharing() {
        starting = false;
        startService(new Intent(this, SharerService.class).setAction(SharerService.ACTION_STOP));
        pollNow();
    }

    /**
     * Debug probe dialog: shows the code to give `jafnify probe --code` and
     * the report as it grows (who was heard, on which interface). Holds the
     * Wi-Fi multicast lock while open.
     */
    private void showProbe() {
        final long inst = Native.ensure(this);
        final String ifaces = NetIfaces.discoverLabelled(this);
        final TextView text = new TextView(this);
        text.setTypeface(android.graphics.Typeface.MONOSPACE);
        text.setTextSize(12);
        text.setTextIsSelectable(true);
        float density = getResources().getDisplayMetrics().density;
        int side = Math.round(32 * density);
        int top = Math.round(24 * density);
        text.setPadding(side, top, side, 0);
        String code = ifaces.isEmpty() || inst == 0 ? null
                : Native.probeStart(inst, ifaces, 60);
        if (code == null || code.startsWith("error")) {
            text.setText(code == null ? getString(R.string.probe_no_iface)
                    : code + "\n" + getString(R.string.probe_no_iface));
        } else {
            text.setText(getString(R.string.probe_intro, code));
        }
        final android.net.wifi.WifiManager.MulticastLock lock =
                ((android.net.wifi.WifiManager) getApplicationContext()
                        .getSystemService(Context.WIFI_SERVICE))
                        .createMulticastLock("jafnify:probe");
        lock.setReferenceCounted(false);
        lock.acquire();
        final Handler poll = new Handler(Looper.getMainLooper());
        final String intro = text.getText().toString();
        final Runnable tick = new Runnable() {
            @Override
            public void run() {
                if (Native.instance == 0) {
                    return;
                }
                String status = Native.probeStatus(Native.instance);
                if (!status.isEmpty()) {
                    text.setText(getString(R.string.probe_status, intro, status));
                }
                if (Native.probeRunning(Native.instance)) {
                    poll.postDelayed(this, 500);
                }
            }
        };
        poll.post(tick);
        new AlertDialog.Builder(this)
                .setTitle(R.string.probe_title)
                .setView(text)
                .setPositiveButton(R.string.probe_close, null)
                .setOnDismissListener(new DialogInterface.OnDismissListener() {
                    @Override
                    public void onDismiss(DialogInterface d) {
                        poll.removeCallbacksAndMessages(null);
                        if (lock.isHeld()) {
                            lock.release();
                        }
                    }
                })
                .show();
    }

    /** Re-reads the stored pair's display name. */
    private void refreshPaired() {
        String name = Native.pairedName(Native.ensure(this));
        pairedName = name == null ? "" : name;
    }

    /**
     * Read, once, why the previous process ended. A system stop that happened
     * recently and was not shown before gets one line under the status, in the
     * core's words. A user stop, an update and an old exit say nothing.
     */
    private void maybeReadStoppedNotice() {
        if (Build.VERSION.SDK_INT < ExitReasons.FIRST_SDK) {
            return;
        }
        long[] value;
        try {
            value = ExitReasons.systemReader(this).read();
        } catch (RuntimeException e) {
            return;
        }
        if (value == null || value.length < 2) {
            return;
        }
        long age = System.currentTimeMillis() - value[1];
        long shown = getSharedPreferences(SharerService.PREFS, MODE_PRIVATE)
                .getLong(KEY_STOPPED_NOTICE, 0);
        boolean wasSharing = getSharedPreferences(SharerService.PREFS, MODE_PRIVATE)
                .getBoolean(SharerService.KEY_SHARING_LIVE, false);
        if (!ShareStoppedNotice.shouldShow(uiRunning, wasSharing, age, (int) value[0],
                value[1], shown)) {
            return;
        }
        String line = Native.stoppedLine((int) value[0], age);
        stoppedNotice = line == null ? "" : line;
        getSharedPreferences(SharerService.PREFS, MODE_PRIVATE)
                .edit().putLong(KEY_STOPPED_NOTICE, value[1]).apply();
    }

    /** Whether the system will show the ongoing notification's Stop action. */
    private boolean notificationsEnabled() {
        if (Build.VERSION.SDK_INT < 24) {
            return true;
        }
        NotificationManager nm =
                (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        return nm == null || nm.areNotificationsEnabled();
    }

    private void onRunStarted(String code) {
        uiRunning = true;
        starting = false;
        stoppedNotice = "";
        currentCode = code;
        codeExpiry = "";
        codeExpired = false;
        healthLine = "";
        healthDetail = "";
        refreshPaired();
    }

    private void onRunStopped() {
        uiRunning = false;
        starting = false;
        currentCode = "";
        parsed = UiText.parse(null);
        codeExpiry = "";
        codeExpired = false;
        healthLine = "";
        healthDetail = "";
        lastStatusShown = "";
        announcedKey = null;
        refreshPaired();
    }

    /** Refresh the paired name when the native status line changes. */
    private void onStatusChanged() {
        String shown = UiText.displayLine(parsed);
        if (!shown.equals(lastStatusShown)) {
            lastStatusShown = shown;
            refreshPaired();
        }
    }

    /** Shrinks the code to fit its width on one line (API 21 safe). */
    private void applyCodeTextSize(TextView code) {
        int widthPx = code.getWidth();
        if (widthPx <= 0) {
            return;
        }
        float density = getResources().getDisplayMetrics().density;
        float fontScale = getResources().getConfiguration().fontScale;
        float widthDp = widthPx / density;
        int sp = UiText.codeTextSizeSp(widthDp, fontScale);
        float targetPx = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, sp,
                getResources().getDisplayMetrics());
        // The same size twice would still relayout the card; set it once.
        if (Math.abs(code.getTextSize() - targetPx) > 0.5f) {
            code.setTextSize(TypedValue.COMPLEX_UNIT_SP, sp);
        }
    }

    /** Draws the whole screen from the fields above. */
    private void render() {
        boolean paired = !pairedName.isEmpty();
        // The service is the truth; the last poll only fills the status text.
        boolean running = runningNow();
        boolean pairing = running && !currentCode.isEmpty();
        UiText.Status st = uiRunning ? UiText.status(parsed.line) : UiText.Status.IDLE;
        boolean waiting = uiRunning && service != null && service.isWaitingForHotspot();
        boolean waitingStart = uiRunning && service != null
                && service.isWaitingForHotspotAtStart();
        String waitingText = getString(waitingStart
                ? R.string.sharer_start_hotspot : R.string.sharer_waiting_hotspot);
        // The native neutral flag is display-only: line 0 is still Connected
        // for the session tracker, but the screen must not claim a link.
        boolean neutral = uiRunning && parsed.neutral;
        UiText.Status displayStatus = waiting ? UiText.Status.SEARCHING
                : neutral ? UiText.Status.CONNECTING : st;
        boolean connected = uiRunning && st == UiText.Status.CONNECTED && !neutral;

        // The code may be on screen before the first poll of the new run; the
        // bound service is asked directly so a pairing code is never missed.
        String code = currentCode;
        String expiry = codeExpiry;
        boolean expired = codeExpired;
        if (service != null && service.isRunning() && code.isEmpty()) {
            String fromService = service.getCode();
            code = fromService == null ? "" : fromService;
            if (expiry.isEmpty()) {
                expiry = service.getCodeExpiry();
            }
            expired = service.getCodeExpired();
        }
        boolean showCode = pairing && !connected;
        // The path line is a claim about the tunnel: stopped or waiting for
        // the hotspot, it is not current and the card must not show it.
        String linkLine = UiText.linkLine(healthLine, uiRunning, waiting);

        // The welcome only when idle. A start or a running session needs the
        // room for the card and the tiles; the card owns the state then.
        boolean idle = !running && !starting;
        boolean showHero = idle;
        setVisible(heroBlock, showHero);
        setVisible(tIntro, idle);
        if (idle) {
            setText(tIntro, getString(R.string.intro_unpaired));
            setVisible(tHint, !paired);
        } else {
            setVisible(tHint, false);
        }

        // The status line: a state glyph and text on the old card; on the tile
        // home the core's card carries the same words.
        String statusText;
        boolean warning = false;
        UiText.Problem retryProblem = UiText.problem(UiText.retryReason(parsed.line));
        boolean retryIsError = retryProblem == UiText.Problem.PORT_BUSY
                || retryProblem == UiText.Problem.NO_NETWORK;
        boolean networkMissing = (st == UiText.Status.SEARCHING
                || st == UiText.Status.PAIRING
                || st == UiText.Status.CONNECTING)
                && !networkReady();
        String peer = parsed.peerName.isEmpty() ? pairedName : parsed.peerName;
        if (!uiRunning) {
            if (lastError != null) {
                statusText = errorSentence(lastError);
                warning = true;
            } else if (starting || running) {
                statusText = getString(R.string.status_starting);
            } else {
                statusText = Native.idleStatus();
            }
        } else if (waiting) {
            statusText = waitingText;
        } else if (neutral) {
            statusText = UiText.RECONNECTING;
        } else if (networkMissing) {
            statusText = getString(R.string.status_no_network_setup);
            warning = true;
        } else if (st == UiText.Status.RETRYING && retryIsError) {
            statusText = errorSentence(UiText.retryReason(parsed.line));
            warning = true;
        } else {
            // Every normal state, including the retry countdown, is the core's
            // own line. Only the documented peer suffix is added here.
            statusText = UiText.displayLine(parsed);
        }
        if (!warning && !waiting && !neutral) {
            statusText = UiText.withPeer(statusText, peer);
        }
        StateArt art = warning ? StateArt.error() : StateArt.of(displayStatus);

        boolean oldCard = tileHomeBroken;
        if (oldCard) {
            // The pairing code as one letter-spaced run, Cancel instead of the
            // primary button, and a quiet New code while it still works.
            setVisible(pairBlock, showCode);
            setText(tCode, code.isEmpty() ? getString(R.string.code_placeholder)
                    : UiText.codeDisplay(code));
            if (showCode) {
                applyCodeTextSize(tCode);
            }
            // The code is never swapped silently: the expired one stays on
            // screen until the user asks for a new one.
            setText(tCodeExpiry, expiry);
            setGone(tCodeExpiry, !(showCode && !expiry.isEmpty()));
            if (showCode && !expiry.isEmpty() && oldExpiryError != expired) {
                oldExpiryError = expired;
                if (expired) {
                    // An error always carries an icon and text, not colour alone.
                    tCodeExpiry.setTextColor(Palette.colour(this, R.color.jafnify_err));
                    setStatusIcon(tCodeExpiry, StateArt.error());
                } else {
                    tCodeExpiry.setTextColor(Palette.colour(this, R.color.jafnify_muted));
                    tCodeExpiry.setCompoundDrawablesRelative(null, null, null, null);
                }
            }
            setVisible(bNewCode, showCode);
            setGone(bToggle, showCode);
            setText(bToggle, getString(uiRunning ? R.string.btn_stop : R.string.btn_share));

            // The visible text may tick; only a coarse change speaks.
            if (UiText.textChanged(tStatus.getText().toString(), statusText)) {
                tStatus.setText(statusText);
            }
            String key = UiText.announceKey(displayStatus, linkLine, peer);
            if (UiText.textChanged(announcedKey, key)) {
                announcedKey = key;
                tStatus.announceForAccessibility(statusText);
                if (!linkLine.isEmpty()) {
                    tHealth.announceForAccessibility(linkLine);
                }
            }
            setStatusIcon(tStatus, art);
            tStatus.setTextColor(statusColour(warning, displayStatus));
            SignalPulse.apply(headArcs, displayStatus, warning);
            SignalPulse.apply(heroArcs, displayStatus, warning);

            // The link health verdict, muted, from the core. The moving
            // handshake age is in Details, so this line no longer ticks.
            setText(tHealth, linkLine);
            setGone(tHealth, linkLine.isEmpty());

            // One line under the status. While sharing it warns that the
            // notification's Stop action is missing; when idle it says why the
            // previous sharing stopped, once.
            String notice = "";
            if (uiRunning) {
                if (!notificationsEnabled()) {
                    String line = Native.notificationsOffLine();
                    notice = line == null ? "" : line;
                }
            } else if (!starting) {
                notice = stoppedNotice;
            }
            setText(tNotice, notice);
            setGone(tNotice, notice.isEmpty());

            // Elapsed time and data counters. For the sharer, rx is what the
            // laptop sent through this phone (up) and tx came back (down).
            long since = service != null ? service.getConnectedSinceMs() : 0;
            if (connected && since != 0) {
                long seconds = (SystemClock.elapsedRealtime() - since) / 1000;
                setText(tElapsed, getString(R.string.elapsed_connected,
                        UiText.formatElapsed(seconds)));
                setVisible(tElapsed, true);
            } else {
                setGone(tElapsed, true);
            }
            if (connected) {
                setText(tTraffic, UiText.phoneTraffic(parsed.rx, parsed.tx));
                setVisible(tTraffic, true);
            } else {
                setGone(tTraffic, true);
            }

            // Idle with a stored pair: when it was last connected.
            if (paired && !uiRunning && !starting) {
                long when = getSharedPreferences(SharerService.PREFS, MODE_PRIVATE)
                        .getLong(SharerService.KEY_LAST_CONNECTED, 0);
                setText(tLast, when == 0
                        ? getString(R.string.last_connected_unknown, pairedName)
                        : getString(R.string.last_connected, pairedName,
                                UiText.formatWhen(when, TimeZone.getDefault())));
                setVisible(tLast, true);
            } else {
                setGone(tLast, true);
            }
        }
        // Details sits below the tile home too; it is not part of either card.
        if (detailsVisible) {
            renderDetails(peer, linkLine, waiting);
        }
        applyKeepScreenOn();

        // The tile home is the app. Only a broken tile home gives way to the
        // old card for this screen; the pairing code and the rest keep working.
        if (!tileHomeBroken) {
            setVisible(tileHome, true);
            setGone(statusCard, true);
            setGone(bToggle, true);
            setVisible(pairBlock, false);
            SignalPulse.still(headArcs);
            SignalPulse.still(heroArcs);
            try {
                renderTileHome(running, starting, connected, lastError != null,
                        statusText, linkLine, waiting, code, expiry, expired);
            } catch (RuntimeException e) {
                // A broken tile home must never lock the user out of the app:
                // fall back to the old card for this screen and say why in the log.
                android.util.Log.e("Jafnify", "tile home failed; showing the old card", e);
                tileHomeBroken = true;
                setGone(tileHome, true);
                setVisible(statusCard, true);
                render();
            }
        } else {
            setGone(tileHome, true);
            setVisible(statusCard, true);
        }
    }

    /** Sets a text only when it changed: an unchanged home must stay idle. */
    private void setText(TextView view, String text) {
        String next = text == null ? "" : text;
        if (!next.equals(view.getText().toString())) {
            view.setText(next);
        }
    }

    /** Shows or hides a view; the visibility is only touched when it changes. */
    private void setVisible(View view, boolean visible) {
        int next = visible ? View.VISIBLE : View.GONE;
        if (view.getVisibility() != next) {
            view.setVisibility(next);
        }
    }

    /** Hides a view with GONE; a no-op when it is already gone. */
    private void setGone(View view, boolean gone) {
        setVisible(view, !gone);
    }

    // ---- the tile home ----------------------------------------------------

    /** The views of one tile of the grid. */
    private static final class TileViews {
        View root;
        TextView label;
        TextView badge;
        TextView value;
        TextView unit;
        TileTrendView trend;
        TextView factOne;
        TextView factTwo;
        TextView note;
        /** A 7 or 8 grid keeps the tiles shorter; the text keeps its size. */
        boolean compact;
        /** The drawing last put in the trend view; "" before the first. */
        String trendKey = "";
        /** The tile id the tap was wired to; "" when the tile is not tappable. */
        String tapId = "";
        /** The colours last set, so an unchanged tile never invalidates. */
        int badgeColour = Integer.MIN_VALUE;
        int valueColour = Integer.MIN_VALUE;
    }

    /** Set once the tile home threw; the old card is shown until the screen is rebuilt. */
    private boolean tileHomeBroken;

    /** One of the app's switches; standard rows are always on. */
    private boolean switchOn(String id) {
        if (flagStore == null) {
            flagStore = Autostart.shared(this);
        }
        return FlagValues.enabled(flagStore, id);
    }

    /** A token size in pixels. */
    private int dimen(int id) {
        return getResources().getDimensionPixelSize(id);
    }

    /**
     * Reloads the saved home layout and rebuilds the tiles when it changed.
     * The layout screen saves at once, so this runs again on every resume.
     */
    private void refreshHomeLayout() {
        String text = HomeLayoutPrefs.loadText(this);
        if (text.equals(homeLayoutText)) {
            return;
        }
        homeLayoutText = text;
        homeLayout = HomeLayout.parse(text);
        buildTileGrid();
    }

    /**
     * Builds the tiles of the grid in slot order. A 7 grid puts one tile
     * across a whole row at the top or the bottom; every other row holds two.
     */
    private void buildTileGrid() {
        tileGrid.removeAllViews();
        tileViews.clear();
        tileRows.clear();
        int slots = homeLayout.slotCount();
        int wideSlot = homeLayout.wideSlot();
        int gap = dimen(R.dimen.jafnify_spacing_sm);
        // A 7 or 8 grid must fit the portrait screen with the card while
        // sharing: the tiles get tighter, the text keeps its size.
        boolean compact = homeLayout.grid >= 7;
        LinearLayout.LayoutParams gridParams =
                (LinearLayout.LayoutParams) tileGrid.getLayoutParams();
        int top = dimen(compact ? R.dimen.jafnify_spacing_sm
                : R.dimen.jafnify_spacing_lg);
        if (gridParams.topMargin != top) {
            gridParams.topMargin = top;
            tileGrid.setLayoutParams(gridParams);
        }
        int index = 0;
        while (index < slots) {
            LinearLayout line = new LinearLayout(this);
            line.setOrientation(LinearLayout.HORIZONTAL);
            if (index == wideSlot) {
                TileViews views = buildTile(compact);
                line.addView(views.root, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT));
                tileViews.add(views);
                index++;
            } else {
                int end = Math.min(index + TileGrid.COLUMNS, slots);
                if (wideSlot > index && wideSlot < end) {
                    end = wideSlot;
                }
                for (int column = index; column < end; column++) {
                    TileViews views = buildTile(compact);
                    // Both tiles of a row take the row's height, so it is even.
                    LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                            0, LinearLayout.LayoutParams.MATCH_PARENT, 1f);
                    if (column > index) {
                        params.setMarginStart(gap);
                    }
                    line.addView(views.root, params);
                    tileViews.add(views);
                }
                index = end;
            }
            LinearLayout.LayoutParams rowParams = new LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT);
            if (!tileRows.isEmpty()) {
                rowParams.setMargins(0, gap, 0, 0);
            }
            tileGrid.addView(line, rowParams);
            tileRows.add(line);
        }
    }

    /**
     * One tile of the grid: the label and the badge, the number with its
     * unit, the small drawing and the two short lines. The words and the
     * numbers come from the core; this only draws them.
     */
    private TileViews buildTile(boolean compact) {
        TileViews views = new TileViews();
        views.compact = compact;
        LinearLayout tile = new LinearLayout(this);
        tile.setOrientation(LinearLayout.VERTICAL);
        tile.setBackgroundResource(R.drawable.card_bg);
        int pad = dimen(compact ? R.dimen.jafnify_spacing_sm : R.dimen.jafnify_spacing_md);
        // A compact tile takes the height back above and below, not from the
        // words; the horizontal padding keeps the text off the card edge.
        int padY = compact ? dimen(R.dimen.jafnify_spacing_xs) : pad;
        tile.setPadding(pad, padY, pad, padY);
        // The whole tile is a tap target. Its content is taller than a finger,
        // but the minimum never falls below one; a 7 or 8 grid uses the body
        // line and the tighter padding, so four rows fit a portrait screen.
        int line = dimen(compact ? R.dimen.jafnify_line_height_body
                : R.dimen.jafnify_line_height_title);
        tile.setMinimumHeight(Math.max(2 * dimen(R.dimen.jafnify_spacing_xl),
                line + 2 * padY));
        // A long press on any tile opens the layout screen: the home belongs
        // to the user, and this is the shortest way there.
        tile.setLongClickable(true);
        tile.setOnLongClickListener(new View.OnLongClickListener() {
            @Override
            public boolean onLongClick(View v) {
                startActivity(new Intent(MainActivity.this, HomeLayoutActivity.class));
                return true;
            }
        });
        tile.setElevation(dimen(R.dimen.jafnify_elevation_low));

        LinearLayout head = new LinearLayout(this);
        head.setOrientation(LinearLayout.HORIZONTAL);
        views.label = new TextView(this);
        views.label.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_caption));
        views.label.setIncludeFontPadding(false);
        views.label.setTypeface(null, android.graphics.Typeface.BOLD);
        views.label.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        head.addView(views.label, new LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));
        views.badge = new TextView(this);
        views.badge.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_caption));
        views.badge.setIncludeFontPadding(false);
        views.badge.setTypeface(null, android.graphics.Typeface.BOLD);
        head.addView(views.badge, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        tile.addView(head, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));

        LinearLayout number = new LinearLayout(this);
        number.setOrientation(LinearLayout.HORIZONTAL);
        number.setGravity(android.view.Gravity.BOTTOM);
        views.value = new TextView(this);
        views.value.setTypeface(android.graphics.Typeface.MONOSPACE,
                android.graphics.Typeface.BOLD);
        views.value.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_title));
        views.value.setIncludeFontPadding(false);
        number.addView(views.value, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        views.unit = new TextView(this);
        views.unit.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        views.unit.setIncludeFontPadding(false);
        views.unit.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        number.addView(views.unit, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        tile.addView(number, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));

        views.trend = new TileTrendView(this);
        tile.addView(views.trend, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                dimen(compact ? R.dimen.jafnify_line_height_caption
                        : R.dimen.jafnify_line_height_title)));

        views.factOne = factView();
        views.factTwo = factView();
        views.note = factView();
        tile.addView(views.factOne, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        tile.addView(views.factTwo, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        tile.addView(views.note, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));

        views.root = tile;
        return views;
    }

    /** One short line of a tile. */
    private TextView factView() {
        TextView view = new TextView(this);
        view.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_caption));
        view.setIncludeFontPadding(false);
        view.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        // One line each: a wrapped fact would make one tile taller than the rest.
        view.setSingleLine(true);
        view.setEllipsize(android.text.TextUtils.TruncateAt.END);
        return view;
    }

    /** Draws the tile home: the card, the tiles and the row of buttons. */
    private void renderTileHome(boolean running, boolean starting, boolean connected,
            boolean failed, String stateLine, String linkLine, boolean waiting,
            String code, String expiry, boolean expired) {
        long now = System.currentTimeMillis();
        int offsetMinutes = TimeZone.getDefault().getOffset(now) / 60_000;
        String summary = service != null ? service.phoneSummaryLine() : "";
        int state = HomeCard.stateFor(running, starting, connected, failed);
        networkPermission = networkMonitor != null && networkMonitor.permissionGranted();
        String text = Native.phoneHome(Native.instance, summary, usageLines(now),
                facts(now), stateLine, linkLine, now, offsetMinutes, sharingUptime(),
                state, switchOn("carrier_quality"), switchOn("battery_status"),
                scopeOf("today"), scopeOf("speed"), sessionLine(), scopeOf("avg"),
                homeLayoutText, networkPermission);
        PhoneHome.Model home = PhoneHome.parse(text);
        if (waiting) {
            PhoneHome.showHotspotOff(home, stateLine);
            PhoneHome.hideLinkValues(home);
        }
        HomeCard card = HomeCard.of(home.status, code, expiry);
        setText(tTileTitle, card.title);
        setText(tTileSub, card.subline);
        setGone(tTileSub, card.subline.isEmpty());
        setText(tTileNotice, home.status.notice);
        setGone(tTileNotice, home.status.notice.isEmpty());
        // One sentence, only while a session runs on battery without the Doze
        // exemption; dismissed once for the rest of that sharing run.
        long batteryRun = currentRun();
        boolean batteryHint = BatteryHint.show(running, isCharging(),
                ignoringOptimisations(),
                batteryRun > 0 && dismissedBatteryHintRun == batteryRun);
        setVisible(batteryBanner, batteryHint);
        // The pairing code belongs to the card, so a new user sees it without
        // scrolling; the Stop button sits next to the New code.
        setVisible(pairTileBox, card.showCode);
        if (card.showCode) {
            setText(tTileCode, UiText.codeDisplay(code));
            applyCodeTextSize(tTileCode);
        } else {
            setText(tTileCode, "");
        }
        setText(tTileCodeExpiry, card.showExpiry ? expiry : "");
        setGone(tTileCodeExpiry, !card.showExpiry);
        if (card.showExpiry && tileExpiryError != expired) {
            tileExpiryError = expired;
            if (expired) {
                // An error always carries an icon and text, not colour alone.
                tTileCodeExpiry.setTextColor(Palette.colour(this, R.color.jafnify_err));
                setStatusIcon(tTileCodeExpiry, StateArt.error());
            } else {
                tTileCodeExpiry.setTextColor(Palette.colour(this, R.color.jafnify_muted));
                tTileCodeExpiry.setCompoundDrawablesRelative(null, null, null, null);
            }
        }
        setVisible(bTileNewCode, card.showNewCode && !waiting);
        setVisible(bTileHotspot, waiting);
        setText(bTileStop, card.action.isEmpty()
                ? getString(card.stop ? R.string.btn_stop : R.string.btn_share)
                : card.action);
        setVisible(bTileBestSpot, switchOn("best_spot"));
        // The dot follows the card: green while the tunnel is up, amber while
        // the phone is trying, grey when it is idle. The drawable is rebuilt
        // only when the tone changes.
        if (!home.status.tone.equals(cardTone)) {
            cardTone = home.status.tone;
            GradientDrawable dot = new GradientDrawable();
            dot.setShape(GradientDrawable.OVAL);
            int dotSize = dimen(R.dimen.jafnify_spacing_md);
            dot.setSize(dotSize, dotSize);
            dot.setColor(toneColour(home.status.tone));
            // An ImageView sizes itself to the drawable; a plain View took the
            // whole card.
            ((android.widget.ImageView) tileDot).setImageDrawable(dot);
        }
        fillTiles(home.tiles);
    }

    /** The facts of this phone; they are asked for again after a while. */
    private String facts(long now) {
        if (factsCache.stale(now)) {
            factsCache.put(DeviceFacts.collect(this), now);
        }
        return factsCache.get();
    }

    /** The usage lines of the phone; the file is read once in a while. */
    private String usageLines(long now) {
        if (usageCache.stale(now)) {
            usageCache.put(readUsageLines(), now);
        }
        return usageCache.get();
    }

    /** Reads the usage lines of the phone from its own file. */
    private String readUsageLines() {
        if (usageStore == null) {
            return "";
        }
        StringBuilder text = new StringBuilder();
        for (String line : usageStore.load()) {
            if (text.length() > 0) {
                text.append('\n');
            }
            text.append(line);
        }
        return text.toString();
    }

    /** Puts the tiles of the core into the grid; an empty slot is hidden. */
    private void fillTiles(java.util.List<PhoneHome.Tile> tiles) {
        for (int i = 0; i < tileViews.size(); i++) {
            TileViews views = tileViews.get(i);
            PhoneHome.Tile tile = i < tiles.size() ? tiles.get(i) : null;
            if (tile == null) {
                setGone(views.root, true);
                continue;
            }
            setVisible(views.root, true);
            setText(views.label, tile.label);
            setText(views.badge, tile.badge);
            setGone(views.badge, tile.badge.isEmpty());
            int badgeColour = toneColour(tile.badgeTone);
            if (views.badgeColour != badgeColour) {
                views.badgeColour = badgeColour;
                views.badge.setTextColor(badgeColour);
            }
            setText(views.value, tile.value);
            int valueColour = toneColour(tile.tone);
            if (views.valueColour != valueColour) {
                views.valueColour = valueColour;
                views.value.setTextColor(valueColour);
            }
            setText(views.unit, tile.unit);
            setGone(views.unit, tile.unit.isEmpty());
            drawTrend(views, tile);
            // The two fact lines always keep their room, so every tile has the
            // same height whether it has one fact or two. A 7 or 8 grid does
            // not reserve an empty line: four rows must fit the screen.
            keepFact(views, views.factOne, fact(tile, 0));
            keepFact(views, views.factTwo, fact(tile, 1));
            setFact(views.note, tile.note);
            wireTileTap(views, tile);
        }
        for (int row = 0; row < tileRows.size(); row++) {
            boolean any = false;
            for (int i = TileGrid.rowStart(row);
                    i < TileGrid.rowEnd(row, tileViews.size()); i++) {
                any = any || tileViews.get(i).root.getVisibility() == View.VISIBLE;
            }
            setVisible(tileRows.get(row), any);
        }
    }

    /**
     * Makes the whole tile the target of the tap when the tile has a scope to
     * change. The core owns the cycle: the phone asks for the next scope and
     * stores it. A tile without a scope stays a plain card. The listener is
     * wired once per tile id, not on every half-second redraw.
     */
    private void wireTileTap(TileViews views, PhoneHome.Tile tile) {
        if ("signal".equals(tile.id) && !networkPermission) {
            // The signal slot explains the missing permission; a tap takes the
            // owner to the switch that grants it.
            String description = tile.facts.isEmpty()
                    ? tile.label
                    : tile.label + ", " + tile.facts.get(0);
            if (!description.equals(views.root.getContentDescription())) {
                views.root.setContentDescription(description);
            }
            if (!"signal-permission".equals(views.tapId)) {
                views.tapId = "signal-permission";
                views.root.setClickable(true);
                views.root.setFocusable(true);
                views.root.setOnClickListener(v -> startActivity(
                        new Intent(MainActivity.this, SettingsActivity.class)));
            }
            return;
        }
        String next = Native.nextTileScope(tile.id, scopeOf(tile.id));
        if (next == null || next.isEmpty()) {
            if (!views.tapId.isEmpty()) {
                views.tapId = "";
                views.root.setOnClickListener(null);
                views.root.setClickable(false);
                views.root.setFocusable(false);
                views.root.setContentDescription(null);
            }
            return;
        }
        // The screen is redrawn every half second. The description may only
        // change when it really changes, or a screen reader reads the tile
        // again and again.
        String description = tile.label + ", " + tapHint();
        if (!description.equals(views.root.getContentDescription())) {
            views.root.setContentDescription(description);
        }
        if (!tile.id.equals(views.tapId)) {
            views.tapId = tile.id;
            views.root.setClickable(true);
            views.root.setFocusable(true);
            views.root.setOnClickListener(v -> cycleTileScope(tile.id));
        }
    }

    /** A tap on a tile cycles its scope and draws the screen again. */
    private void cycleTileScope(String tileId) {
        String next = Native.nextTileScope(tileId, scopeOf(tileId));
        if (next == null || next.isEmpty()) {
            return;
        }
        TileScopePrefs.save(tileScopes, tileId, next);
        render();
    }

    /** The scope the phone remembers for a tile; "" when it remembers none. */
    private String scopeOf(String tileId) {
        return TileScopePrefs.load(tileScopes, tileId);
    }

    /** The words that say a tile can be tapped; the core owns them. */
    private String tapHint() {
        if (tapHint.isEmpty()) {
            String hint = Native.tileTapHint();
            if (hint != null) {
                tapHint = hint;
            }
        }
        return tapHint;
    }

    /** The sharing session as one line for the core; "" without a session. */
    private String sessionLine() {
        if (service == null) {
            return "";
        }
        SessionRecord record = service.getCurrentSession();
        if (record == null) {
            return "";
        }
        return StatsText.tileSessionLine(record);
    }

    /**
     * Gives the drawing of a tile its shape. The drawing is applied only when
     * its data changed: an unchanged tile must not invalidate every poll.
     */
    private void drawTrend(TileViews views, PhoneHome.Tile tile) {
        boolean none = "none".equals(tile.kind);
        String key = trendKey(tile);
        if (!key.equals(views.trendKey)) {
            views.trendKey = key;
            views.trend.clear();
            if ("fill".equals(tile.kind)) {
                views.trend.setFill(tile.fill, tile.fillMax);
            } else if ("split".equals(tile.kind)) {
                views.trend.setSplit(tile.down, tile.up);
            } else if ("hours".equals(tile.kind)) {
                views.trend.setHours(HourMarks.roles(tile.cells));
            } else if (!none) {
                views.trend.setLine(tile.points);
                views.trend.setReference(tile.reference, tile.hasReference);
                if (tile.levelMax > 0) {
                    views.trend.setLevels(tile.level, tile.levelMax);
                }
            }
        }
        // A tile without a drawing does not reserve a line in a 7 or 8 grid.
        setGone(views.trend, views.compact && none);
    }

    /** The exact data of a drawing, so the same drawing is not redone. */
    private String trendKey(PhoneHome.Tile tile) {
        StringBuilder key = new StringBuilder(tile.kind);
        if ("fill".equals(tile.kind)) {
            key.append('|').append(tile.fill).append('/').append(tile.fillMax);
        } else if ("split".equals(tile.kind)) {
            key.append('|').append(tile.down).append('/').append(tile.up);
        } else if ("hours".equals(tile.kind)) {
            for (String cell : tile.cells) {
                key.append('|').append(cell);
            }
        } else {
            for (int point : tile.points) {
                key.append('|').append(point);
            }
            key.append("|r").append(tile.hasReference ? tile.reference : 0);
            key.append("|l").append(tile.level).append('/').append(tile.levelMax);
        }
        return key.toString();
    }

    /** The n-th short line of a tile, or "" when it has fewer. */
    private String fact(PhoneHome.Tile tile, int index) {
        return index < tile.facts.size() ? tile.facts.get(index) : "";
    }

    /**
     * Shows a short line; in the roomy grid an empty one keeps its room
     * (INVISIBLE, not GONE), while a 7 or 8 grid hides it to save height.
     */
    private void keepFact(TileViews views, TextView view, String text) {
        if (text.isEmpty()) {
            if (views.compact) {
                setText(view, "");
                setGone(view, true);
            } else {
                // A space keeps the line's height while it stays invisible.
                setText(view, " ");
                if (view.getVisibility() != View.INVISIBLE) {
                    view.setVisibility(View.INVISIBLE);
                }
            }
            return;
        }
        setText(view, text);
        setVisible(view, true);
    }

    /** Shows a short line, or hides it when it is empty. */
    private void setFact(TextView view, String text) {
        setText(view, text);
        setGone(view, text.isEmpty());
    }

    /** The colour of a tone; the core decides which tone a number carries. */
    private int toneColour(String tone) {
        if ("ok".equals(tone)) {
            return Palette.colour(this, R.color.jafnify_ok);
        }
        if ("wait".equals(tone)) {
            return Palette.colour(this, R.color.jafnify_wait_text);
        }
        if ("err".equals(tone)) {
            return Palette.colour(this, R.color.jafnify_err);
        }
        return Palette.colour(this, R.color.jafnify_fg);
    }

    /** How long the sharing has been up, in seconds; -1 when it is not up. */
    private long sharingUptime() {
        long since = service != null ? service.getConnectedSinceMs() : 0;
        if (since == 0) {
            return -1;
        }
        return (SystemClock.elapsedRealtime() - since) / 1000;
    }

    /** The status colour role; `wait_text` keeps the amber label legal. */
    private int statusColour(boolean warning, UiText.Status st) {
        if (warning) {
            return Palette.colour(this, R.color.jafnify_err);
        }
        if (st == UiText.Status.PAIRING || st == UiText.Status.CONNECTING
                || st == UiText.Status.RETRYING) {
            return Palette.colour(this, R.color.jafnify_wait_text);
        }
        return Palette.colour(this, R.color.jafnify_fg);
    }

    /**
     * Puts the state glyph at the start of a text view. The bounds come from
     * the view's text size (which already includes the font scale), so the
     * exclamation grows with a 200 % text setting instead of staying at a
     * fixed dp vector size. The drawable is a compound drawable, not a second
     * accessibility node; the text carries the meaning.
     */
    private void setStatusIcon(TextView view, StateArt art) {
        Drawable icon = getDrawable(art.drawableRes).mutate();
        icon.setTint(Palette.colour(this, art.colourRes));
        int size = UiText.statusGlyphSizePx(view.getTextSize());
        icon.setBounds(0, 0, size, size);
        view.setCompoundDrawablesRelative(icon, null, null, null);
    }

    private String errorSentence(String raw) {
        switch (UiText.problem(raw)) {
            case PORT_BUSY:
                return getString(R.string.err_port_busy);
            case NO_NETWORK:
                return getString(R.string.err_no_network);
            default:
                return getString(R.string.err_generic);
        }
    }

    /** Toggle the inline Details panel below the status card. */
    private void setDetailsVisible(boolean visible) {
        detailsVisible = visible;
        detailsPanel.setVisibility(visible ? View.VISIBLE : View.GONE);
        bDetails.setText(visible ? R.string.details_hide : R.string.details_show);
        if (visible) {
            boolean waiting = uiRunning && service != null && service.isWaitingForHotspot();
            renderDetails(parsed.peerName.isEmpty() ? pairedName : parsed.peerName,
                    UiText.linkLine(healthLine, uiRunning, waiting), waiting);
        }
    }

    /**
     * The technical details as a label/value table, refreshed while the panel
     * is expanded: the link (health, status, peer, last error) and the current
     * session. Views are rebuilt only when the rows change.
     */
    private void renderDetails(String peer, String linkLine, boolean waiting) {
        java.util.List<String[]> link = new java.util.ArrayList<>();
        // The handshake age is a tunnel measurement: while the hotspot is gone
        // it only speaks about the last time the laptop was heard, not now.
        if (!waiting && !healthDetail.isEmpty()) {
            link.add(StatsText.row(getString(R.string.details_label_handshake),
                    UiText.handshakeAge(healthDetail)));
        }
        // The raw native line may still say Connected while the hotspot is
        // gone: the Details status follows the same waiting sentence.
        String waitingText = getString(service != null
                && service.isWaitingForHotspotAtStart()
                ? R.string.sharer_start_hotspot : R.string.sharer_waiting_hotspot);
        String status = !uiRunning ? Native.idleStatus()
                : waiting ? waitingText
                : UiText.displayLine(parsed);
        link.add(StatsText.row(getString(R.string.details_label_status),
                UiText.withPeer(status, waiting ? "" : peer)));
        if (!parsed.peerId.isEmpty()) {
            link.add(StatsText.row(getString(R.string.details_label_peer_key),
                    UiText.keyPrefix(parsed.peerId)));
        }
        String error = lastError == null || lastError.isEmpty()
                ? getString(R.string.details_none) : lastError;
        link.add(StatsText.row(getString(R.string.details_label_last_error), error));
        SessionRecord current = service == null ? null : service.getCurrentSession();
        java.util.List<String[]> session = current == null
                ? new java.util.ArrayList<String[]>() : StatsText.sessionRows(current);
        String[] connection = UiText.connectionLines(
                Native.connectionDetails(linkLine, parsed.endpoint));
        String[] phone = phoneLines();

        StringBuilder key = new StringBuilder();
        for (String[] r : link) key.append(r[0]).append('=').append(r[1]).append('\n');
        key.append("--\n");
        for (String[] r : session) key.append(r[0]).append('=').append(r[1]).append('\n');
        key.append("--\n");
        for (String line : connection) key.append(line).append('\n');
        key.append("--\n");
        for (String line : phone) key.append(line).append('\n');
        if (key.toString().equals(lastDetailsKey)) {
            return;
        }
        lastDetailsKey = key.toString();
        detailsRows.removeAllViews();
        addDetailsSection(R.string.details_section_link, link, true, connection);
        if (phone.length > 0) {
            addDetailsSection(R.string.details_section_phone,
                    new java.util.ArrayList<String[]>(), false, phone);
        }
        if (!session.isEmpty()) {
            addDetailsSection(R.string.details_section_session, session, false, null);
        }
    }

    private String[] phoneLines() {
        if (service == null) {
            return new String[0];
        }
        // The phone data is standard now: no switch gates these lines.
        String text = Native.phoneSection(service.phoneSummaryLine(),
                switchOn("carrier_quality"),
                switchOn("network_details"),
                switchOn("battery_status"));
        if (text == null || text.isEmpty()) {
            return new String[0];
        }
        return text.split("\n");
    }

    private void addDetailsSection(int titleRes, java.util.List<String[]> rows, boolean first,
            String[] lines) {
        int xs = getResources().getDimensionPixelSize(R.dimen.jafnify_spacing_xs);
        int lg = getResources().getDimensionPixelSize(R.dimen.jafnify_spacing_lg);
        TextView title = new TextView(this);
        title.setText(getString(titleRes));
        title.setAllCaps(true);
        title.setLetterSpacing(0.08f);
        title.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_caption));
        title.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        android.widget.LinearLayout.LayoutParams tp = new android.widget.LinearLayout.LayoutParams(
                android.widget.LinearLayout.LayoutParams.MATCH_PARENT,
                android.widget.LinearLayout.LayoutParams.WRAP_CONTENT);
        tp.topMargin = first ? 0 : lg;
        tp.bottomMargin = xs;
        detailsRows.addView(title, tp);
        for (String line : lines == null ? new String[0] : lines) {
            TextView value = new TextView(this);
            value.setText(line);
            value.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                    getResources().getDimension(R.dimen.jafnify_type_body));
            value.setTextColor(Palette.colour(this, R.color.jafnify_fg));
            detailsRows.addView(value, new android.widget.LinearLayout.LayoutParams(
                    android.widget.LinearLayout.LayoutParams.MATCH_PARENT,
                    android.widget.LinearLayout.LayoutParams.WRAP_CONTENT));
        }
        for (String[] r : rows) {
            android.widget.LinearLayout line = new android.widget.LinearLayout(this);
            line.setOrientation(android.widget.LinearLayout.HORIZONTAL);
            line.setPaddingRelative(0, xs, 0, xs);
            TextView label = new TextView(this);
            label.setText(r[0]);
            label.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                    getResources().getDimension(R.dimen.jafnify_type_body));
            label.setTextColor(Palette.colour(this, R.color.jafnify_muted));
            TextView value = new TextView(this);
            value.setText(r[1]);
            value.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                    getResources().getDimension(R.dimen.jafnify_type_body));
            value.setTextColor(Palette.colour(this, R.color.jafnify_fg));
            line.addView(label, new android.widget.LinearLayout.LayoutParams(
                    0, android.widget.LinearLayout.LayoutParams.WRAP_CONTENT, 2f));
            line.addView(value, new android.widget.LinearLayout.LayoutParams(
                    0, android.widget.LinearLayout.LayoutParams.WRAP_CONTENT, 3f));
            detailsRows.addView(line);
        }
    }

    @Override
    public void onBackPressed() {
        if (UiText.backCollapsesDetails(detailsVisible)) {
            setDetailsVisible(false);
            return;
        }
        super.onBackPressed();
    }
}
