package com.siliconstencil.jafnify;

import android.annotation.SuppressLint;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import android.net.TrafficStats;
import android.net.wifi.WifiManager;
import android.os.BatteryManager;
import android.os.Binder;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.os.SystemClock;

import java.io.File;
import java.io.IOException;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Owns the running sharer (gateway). Runs as a foreground service so it
 * survives the screen turning off and the app being backgrounded; the
 * Activity only starts/stops it (via intents) and reads status (via the
 * binder). All entry points run on the main thread.
 *
 * Declared foregroundServiceType="connectedDevice" in the manifest (the
 * documented type for relaying internet to a paired device, with no duration
 * cap); startForeground() passes the matching type on API 34+.
 */
public class SharerService extends Service {
    public static final String ACTION_START = "com.siliconstencil.jafnify.SHARER_START";
    /** Why this start happened: user, boot or system restart. */
    public static final String EXTRA_START_REASON = "start_reason";
    /** Start with a fresh pairing code even when a device is already paired. */
    public static final String ACTION_PAIR_NEW = "com.siliconstencil.jafnify.SHARER_PAIR_NEW";
    public static final String ACTION_STOP = "com.siliconstencil.jafnify.SHARER_STOP";

    private static final int NOTIFICATION_ID = 3;
    private static final int BUDGET_NOTIFICATION_ID = 4;
    private static final int HOTSPOT_NOTIFICATION_ID = 5;
    private static final String CHANNEL_ID = "jafnify_sharer";
    private static final String HOTSPOT_CHANNEL_ID = "jafnify_hotspot_off";
    /** The tether screen is not public API; HotspotSettings falls back. */
    private static final long NOTIFY_POLL_MS = 1000;
    private static final long IFACE_POLL_MS = 1000;
    private static final long RESTART_HEALTH_MS = 1000;
    /** Where the screen reads "Last connected" from (wall-clock millis). */
    static final String PREFS = "jafnify_sharer";
    static final String KEY_LAST_CONNECTED = "last_connected_ms";
    /** A user Stop wins over the resume setting, even across process death. */
    static final String KEY_USER_STOPPED = "user_stopped";
    /**
     * True from the moment sharing starts until a clean teardown. A killed
     * process never runs the teardown, so a true value on the next app open
     * means "the system closed the app while it was sharing".
     */
    static final String KEY_SHARING_LIVE = "sharing_live";

    public class LocalBinder extends Binder {
        public SharerService getService() {
            return SharerService.this;
        }
    }

    private final IBinder binder = new LocalBinder();
    private Handler handler;
    private WifiManager.MulticastLock multicastLock;
    private WifiManager.WifiLock wifiLock;
    private PowerManager.WakeLock wakeLock;

    private boolean running = false;
    /** Pairing code of this run; "" when serving the stored pair (no code). */
    private String code = "";
    private String pairedName = "";
    private String lastError = null;
    private final NotifText notifText = new NotifText();
    private final SharerLinkPolicy linkPolicy = new SharerLinkPolicy();
    private final HotspotAlert hotspotAlert = new HotspotAlert();
    private BroadcastReceiver hotspotReceiver = null;
    private final ConnectionLog connectionLog = new ConnectionLog();
    private SessionStore sessionStore;
    private List<SessionRecord> history = new ArrayList<>();
    /** The session of this sharing run; null when not sharing. */
    private SessionTracker tracker;
    /** The last parsed status, for the once-per-second phone sampler. */
    private UiText.Parsed lastParsed;
    private boolean lastConnected;
    private PhoneSampler phoneSampler;
    private NetworkSampler networkSampler;
    private PhoneNetworkMonitor networkMonitor;
    private boolean networkMonitorRunning;
    /** The last M0 line the sampler wrote; the main screen reads it. */
    private String lastPhoneSummary = "";
    /** The last native status line written to the connection log. */
    private String lastLoggedStatus = "Idle";
    /** The interface set last written to the connection log. */
    private String lastLoggedIfaces = "";
    private String lastIfaces = "";
    private String pendingIfaces = null;
    private long pendingSinceMs = 0;
    private boolean pendingIsFallback = false;
    private String rejectedIfaces = null;
    /** Unique per real sharer start, also across service instances. */
    private static long nextRunSeq = 0;
    private long runSeq = 0;
    /** elapsedRealtime when the current connection came up; 0 when not connected. */
    private long connectedSinceMs = 0;

    // --- Diagnostic journal (additive; no sharing behavior depends on it) ---
    /** Append-only flight recorder under files/diag, or null while unavailable. */
    private volatile DiagLog diag;
    /** A failed write disables the journal for the rest of this service process. */
    private boolean diagDisabled = false;
    private Long diagLastStatsMs = null;
    private Long diagLastPeerRxMs = null;
    private String diagLastIfaces = "";
    private long diagWorkerNotify = 0;
    private long diagWorkerIface = 0;
    private boolean diagWasConnected = false;
    /** The native session counter last seen; a change while Connected means
     *  the peer started a new session (see trackConnected). */
    private long lastSessionEpoch = -1;
    private ConnectivityManager.NetworkCallback diagNetworkCallback = null;
    private BroadcastReceiver diagScreenReceiver = null;
    private BroadcastReceiver diagPowerReceiver = null;
    private Thread.UncaughtExceptionHandler diagUncaughtHandler = null;
    private Thread.UncaughtExceptionHandler diagPreviousUncaughtHandler = null;

    @Override
    public void onCreate() {
        super.onCreate();
        Clock.follow(this);
        handler = new Handler(Looper.getMainLooper());
        sessionStore = new SessionStore(new File(getFilesDir(), "sessions.tsv"));
        connectionLog.attach(new File(getFilesDir(), "connection.log"));
        history = sessionStore.load();
        UsageLineStore statsStore =
                new UsageLineStore(new File(getFilesDir(), "phone-stats.tsv"));
        phoneSampler = new PhoneSampler(
                new PhoneSampler.Bridge() {
                    @Override
                    public String summaryLine(long upBps, long downBps,
                            String network, int signal, int dbm, int batteryPct,
                            int tempC10, long usedBytes, long allowBytes, int resetDay,
                            int monthKey, int switches, boolean withDns,
                            int thermalStatus, int thermalHeadroom) {
                        return Native.phoneStatsLine(upBps, downBps, network,
                                signal, dbm, batteryPct, tempC10, usedBytes, allowBytes,
                                resetDay, monthKey, switches, withDns, thermalStatus,
                                thermalHeadroom,
                                networkMonitor != null && networkMonitor.permissionGranted());
                    }

                    @Override
                    public String budgetRecord(long allowBytes, long usedBytes,
                            int resetDay, int monthKey, int warnedMonth) {
                        return Native.budgetRecord(allowBytes, usedBytes,
                                resetDay, monthKey, warnedMonth);
                    }

                    @Override
                    public String budgetWarning(String record) {
                        return Native.budgetWarning(record);
                    }

                    @Override
                    public void setPhoneStats(String line) {
                        Native.setPhoneStats(Native.instance, line);
                    }

                    @Override
                    public void setLocalSummary(String line) {
                        lastPhoneSummary = line == null ? "" : line;
                    }

                    @Override
                    public String usageStep(long atMs, long upBytes, long downBytes,
                            boolean connected, String network) {
                        return Native.usageStep(
                                atMs, upBytes, downBytes, connected, network);
                    }

                    @Override
                    public String usageCurrent() {
                        return Native.usageCurrent();
                    }

                    @Override
                    public void usageReset() {
                        Native.usageReset();
                    }

                    @Override
                    public void usageSeed(String line) {
                        Native.usageSeed(line);
                    }
                },
                new PhoneSampler.FlagSource() {
                    @Override
                    public boolean enabled(String id) {
                        return FlagValues.enabled(
                                Autostart.shared(SharerService.this), id);
                    }
                },
                new PhoneSampler.BatterySource() {
                    @Override
                    public int[] read() {
                        return readStickyBattery();
                    }
                },
                statsStore,
                statsStore,
                PhoneSampler::systemDate,
                this::notifyBudgetWarning);
        networkSampler = new NetworkSampler(
                new NetworkSampler.Bridge() {
                    @Override
                    public String generationWord(String kind) {
                        return Native.generationWord(kind);
                    }

                    @Override
                    public boolean generationSwitch(String previous, String current) {
                        return Native.generationSwitch(previous, current);
                    }

                    @Override
                    public String bucketLine(String fields) {
                        return Native.networkBucketLine(fields);
                    }

                    @Override
                    public String signalSample(long atMs, int level, long lastAtMs) {
                        return Native.signalSample(atMs, level, lastAtMs);
                    }

                    @Override
                    public long saneMah(long counter) {
                        return Native.saneMah(counter);
                    }

                    @Override
                    public long[] networkSeed(String line) {
                        return Native.networkSeed(line);
                    }
                },
                statsStore);
        networkMonitor = new PhoneNetworkMonitor(this, networkSampler);
        try {
            ensureDiag();
        } catch (RuntimeException ignored) {
            // Diagnostics are optional; the service starts regardless.
        }
        try {
            installDiagUncaughtHandler();
        } catch (RuntimeException ignored) {
            // Diagnostics are optional; the service starts regardless.
        }
    }

    @Override
    public IBinder onBind(Intent intent) {
        return binder;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        if (ACTION_START.equals(action) || ACTION_PAIR_NEW.equals(action)) {
            setUserStopped(false);
            boolean pairNew = ACTION_PAIR_NEW.equals(action);
            StartReason reason = pairNew ? StartReason.USER
                    : StartReason.parse(intent.getStringExtra(EXTRA_START_REASON));
            // Started with startForegroundService(): must reach
            // startForeground() promptly, before any slow work. Only the
            // first start pushes the start text; a repeat ACTION_START from
            // the Activity must not replace the live state (that used to
            // leave the notification stale on "Connecting...").
            if (!running) {
                String startingText = getString(
                        R.string.sharer_notif_starting_reason, reason.wire());
                notifText.shown(startingText);
                Notification starting = buildNotification(startingText);
                if (Build.VERSION.SDK_INT >= 34) {
                    startForeground(NOTIFICATION_ID, starting,
                            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
                } else {
                    startForeground(NOTIFICATION_ID, starting);
                }
            }
            if (running && pairNew) {
                // Restart on the pairing track; the stored pair stays until
                // a new pairing succeeds.
                teardown();
            }
            if (!running) {
                beginSharing(pairNew, reason);
            }
        } else if (ACTION_STOP.equals(action)) {
            setUserStopped(true);
            stopSharing();
            return START_NOT_STICKY;
        } else if (!running && !resumeAfterSystemRestart()) {
            stopSelf();
            return START_NOT_STICKY;
        }
        // Sticky only while the opt-in resume setting, a stored pair and the
        // absence of a user Stop agree; a pairing code is never resurrected.
        return restartPolicy();
    }

    /**
     * With a stored pair the sharer reconnects to it without a code, unless
     * {@code pairNew}: then it runs the code pairing, which replaces the
     * stored pair on success.
     */
    private void beginSharing(boolean pairNew, StartReason reason) {
        ensureDiag();
        lastError = null;
        if (Native.ensure(this) == 0) {
            failStart("could not create native instance");
            return;
        }
        // The previous process may have ended because of the update that
        // brought this build: say why once, on the first summary of the new
        // session. Android below API 30 cannot tell, so nothing is sent.
        ExitReasons.reportOnce(Build.VERSION.SDK_INT, ExitReasons.systemReader(this),
                new ExitReasons.Sink() {
                    @Override
                    public void report(int exitReason, long ageMs) {
                        Native.setExitReason(exitReason, ageMs);
                    }
                });
        String name = Native.pairedName(Native.instance);
        boolean known = !pairNew && !name.isEmpty();
        // The hotspot is a LAN the default network does not lead to: discovery
        // runs on every non-cellular interface, not on the default route.
        // With no usable LAN (or with the system saying the AP is off) there
        // is nothing to discover on: the native side starts with no interface
        // and the session waits for the hotspot instead of appearing to try.
        String ifaces = NetIfaces.discover(this);
        boolean hotspotReady = SharerLinkPolicy.hotspotReady(
                SharerLinkPolicy.usableSet(ifaces), hotspotApHint());
        String discovery = hotspotReady ? ifaces : "";
        getSharedPreferences(PREFS, MODE_PRIVATE)
                .edit().putBoolean(KEY_SHARING_LIVE, true).apply();
        String ret = known ? Native.sharerStartKnown(Native.instance, discovery)
                : Native.sharerStart(Native.instance, discovery);
        if (ret == null || !(known ? ret.isEmpty() : ret.matches("[0-9]{4}"))) {
            Native.destroy();
            failStart(ret == null || ret.isEmpty() ? "" : ret);
            return;
        }

        WifiManager wifi = (WifiManager) getApplicationContext()
                .getSystemService(Context.WIFI_SERVICE);
        multicastLock = wifi.createMulticastLock("jafnify:sharer");
        multicastLock.setReferenceCounted(false);
        multicastLock.acquire();
        // The reference products hold this for the whole serving period; a
        // LOW_LATENCY lock keeps the radio out of deep power save so the
        // phone stays reachable on the hotspot. Released with the session.
        wifiLock = wifi.createWifiLock(RadioLock.mode(Build.VERSION.SDK_INT),
                "jafnify:sharer");
        wifiLock.setReferenceCounted(false);
        acquireRadioLock();
        PowerManager pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "jafnify:sharer");
        wakeLock.setReferenceCounted(false);
        acquireSessionWakeLock();

        code = known ? "" : ret;
        pairedName = name;
        // A new sharing run starts a new off period for the alert.
        hotspotAlert.clear();
        running = true;
        runSeq = ++nextRunSeq;
        tracker = new SessionTracker("sharer", System.currentTimeMillis());
        tracker.setStartReason(reason.wire());
        lastLoggedStatus = "Idle";
        lastLoggedIfaces = IfaceRefresh.normalize(discovery);
        // Earlier runs stay above this one (kept in connection.log).
        addLog("session", ConnectionLog.sessionStartedLine("sharer"));
        addLog("session", ConnectionLog.startedByLine(reason.wire()));
        lastIfaces = discovery;
        linkPolicy.start(discovery, !code.isEmpty());
        if (linkPolicy.isWaiting()) {
            releaseSessionWakeLock();
            releaseRadioLock();
            addLog("link", "waiting for the hotspot");
        }
        updateNotification();
        diagLastStatsMs = null;
        diagLastPeerRxMs = null;
        diagWasConnected = false;
        diagLastIfaces = IfaceRefresh.normalize(discovery);
        try {
            startDiagCallbacks();
            diagEvent(System.currentTimeMillis(), "session_start",
                    "mode", known ? "known" : "code",
                    "started_by", reason.wire(),
                    "ifaces", diagLastIfaces.isEmpty() ? "none" : diagLastIfaces,
                    "hotspot_ready", hotspotReady ? "yes" : "no",
                    "candidates", safeCandidates());
            if (linkPolicy.isWaiting()) {
                diagEvent(System.currentTimeMillis(), "waiting_hotspot",
                        "reason", "no_hotspot_at_start");
            }
        } catch (RuntimeException ignored) {
            // Diagnostics must never affect starting the sharer.
            unregisterDiagCallbacks();
        }
        startHotspotReceiver();
        handler.postDelayed(notifyPoll, NOTIFY_POLL_MS);
        handler.postDelayed(ifacePoll, IFACE_POLL_MS);
    }

    // Held for the whole sharing session by design. teardown() releases it on
    // every stop path (Stop, forget, restart failure, pair-new, onDestroy);
    // failStart() returns before this lock is acquired.
    @SuppressLint("WakelockTimeout")
    private void acquireSessionWakeLock() {
        wakeLock.acquire();
    }

    /** Drop the partial wake lock while waiting; the poll may sleep. */
    private void releaseSessionWakeLock() {
        if (wakeLock != null && wakeLock.isHeld()) {
            wakeLock.release();
        }
    }

    /** Re-acquire the partial wake lock after a restore. */
    private void ensureSessionWakeLock() {
        if (wakeLock != null && !wakeLock.isHeld()) {
            acquireSessionWakeLock();
        }
    }

    /** Re-arm multicast after the interface came back. */
    private void ensureMulticastLock() {
        if (multicastLock != null && !multicastLock.isHeld()) {
            multicastLock.acquire();
        }
    }

    /** Hold the Wi-Fi radio awake only while there is a live link to serve. */
    private void acquireRadioLock() {
        if (wifiLock != null) {
            wifiLock.acquire();
        }
    }

    /** Drop the Wi-Fi lock while waiting for the hotspot. */
    private void releaseRadioLock() {
        if (wifiLock != null && wifiLock.isHeld()) {
            wifiLock.release();
        }
    }

    /** Re-acquire the Wi-Fi lock after a restore. */
    private void ensureRadioLock() {
        if (wifiLock != null && !wifiLock.isHeld()) {
            acquireRadioLock();
        }
    }

    private void failStart(String error) {
        lastError = error;
        stopForeground(true);
        stopSelf();
    }

    private final Runnable notifyPoll = new Runnable() {
        @Override
        public void run() {
            diagWorkerNotify++;
            if (!running) {
                return;
            }
            updateNotification();
            sampleDiag(System.currentTimeMillis());
            if (phoneSampler != null && lastParsed != null) {
                String net = PhoneSampler.networkWord(currentTransport());
                // One merged row now owns both the network report and the
                // signal history; an old install keeps either old switch.
                boolean detailsOn = FlagValues.networkDetails(
                        Autostart.shared(SharerService.this));
                boolean reportOn = detailsOn;
                boolean signalOn = detailsOn;
                // The best-spot switch needs the phone's own signal level and
                // strength too: the laptop's best-spot block reads them from
                // this summary line.
                boolean bestSpotOn = Autostart.shared(SharerService.this)
                        .getBoolean("best_spot", false);
                boolean granted = networkMonitor != null
                        && networkMonitor.permissionGranted();
                // A permission taken away while running: store the truth so
                // Settings shows the switch off, and stop the listener.
                if (PhonePermission.shouldTurnOff(granted, detailsOn, false)) {
                    Autostart.shared(SharerService.this)
                            .putBoolean(FlagValues.NETWORK_DETAILS, false);
                    reportOn = false;
                    signalOn = false;
                    detailsOn = false;
                }
                if (networkSampler != null) {
                    networkSampler.setFlags(reportOn, signalOn);
                }
                if (networkMonitor != null) {
                    if (granted && (reportOn || signalOn || bestSpotOn)) {
                        if (!networkMonitorRunning) {
                            networkMonitor.start();
                            networkMonitorRunning = true;
                        }
                    } else if (networkMonitorRunning) {
                        networkMonitor.stop();
                        networkMonitorRunning = false;
                    }
                    // The generation is read only while the user's switch is
                    // on and the listener runs; off means the generic word.
                    if (networkMonitorRunning && "cellular".equals(net)) {
                        String generation = networkMonitor.generationWord();
                        if (!generation.isEmpty()) {
                            net = generation;
                        }
                    }
                    // Battery reads (a sticky intent) only while the report is on.
                    if (networkSampler != null && reportOn) {
                        networkSampler.onTick(System.currentTimeMillis(),
                                lastParsed.rx, lastParsed.tx, lastConnected,
                                networkMonitor.batteryPercent(),
                                networkMonitor.chargeCounterMah(),
                                networkMonitor.charging());
                    }
                }
                int signal = PhonePermission.gatedSignal(granted, reportOn, signalOn,
                        bestSpotOn, networkMonitorRunning && networkMonitor != null
                                ? networkMonitor.signalLevel() : -1);
                phoneSampler.setSignalLevel(signal);
                int dbm = PhonePermission.gatedDbm(granted, reportOn, signalOn,
                        bestSpotOn, networkMonitorRunning && networkMonitor != null
                                ? networkMonitor.dbm() : -1);
                phoneSampler.setDbm(dbm);
                phoneSampler.setThermalStatus(thermalStatus());
                phoneSampler.setThermalHeadroom(thermalHeadroomPercent());
                if (networkSampler != null) {
                    phoneSampler.setSwitches(networkSampler.switchesThisHour());
                }
                phoneSampler.onTick(System.currentTimeMillis(), lastParsed.rx,
                        lastParsed.tx, lastConnected, net);
            }
            handler.postDelayed(this, NOTIFY_POLL_MS);
        }
    };

    private final Runnable ifacePoll = new Runnable() {
        @Override
        public void run() {
            diagWorkerIface++;
            if (!running) {
                return;
            }
            long now = SystemClock.elapsedRealtime();
            if (pendingIfaces != null) {
                boolean wasFallback = pendingIsFallback;
                if (hasNativeStatus()) {
                    if (!wasFallback) {
                        lastIfaces = pendingIfaces;
                        rejectedIfaces = null;
                        linkPolicy.restartSucceeded();
                    }
                    pendingIfaces = null;
                    pendingIsFallback = false;
                } else if (now - pendingSinceMs >= RESTART_HEALTH_MS) {
                    if (wasFallback) {
                        failRestart("could not restore discovery");
                        return;
                    }
                    if (linkPolicy.isRestoring()) {
                        // The set that vanished must not be used as a fallback.
                        rejectedIfaces = IfaceRefresh.normalize(pendingIfaces);
                        pendingIfaces = null;
                        pendingIsFallback = false;
                        linkPolicy.restartFailed("could not restore discovery", lastIfaces);
                        enterWaiting();
                    } else {
                        startFallbackRestart(pendingIfaces, null);
                    }
                    if (!running) {
                        return;
                    }
                    handler.postDelayed(this, IFACE_POLL_MS);
                    return;
                }
            }
            String current = NetIfaces.discover(SharerService.this);
            observeLinkChange(current);
            if (IfaceRefresh.normalize(current).equals(rejectedIfaces)) {
                handler.postDelayed(this, IFACE_POLL_MS);
                return;
            }
            rejectedIfaces = null;
            SharerLinkPolicy.Action action = linkPolicy.tick(current, now, isConnected());
            if (action == SharerLinkPolicy.Action.WAIT) {
                enterWaiting();
            } else if (action == SharerLinkPolicy.Action.RESTART) {
                restartForIfaces(linkPolicy.restartSet(), now);
                if (!running) {
                    return;
                }
            }
            handler.postDelayed(this, IFACE_POLL_MS);
        }
    };

    /** The usable set vanished: keep the session, drop the wake lock. */
    private void enterWaiting() {
        releaseSessionWakeLock();
        releaseRadioLock();
        addLog("link", "waiting for the hotspot");
        diagEvent(System.currentTimeMillis(), "waiting_hotspot");
        updateNotification();
    }

    /** The candidate list as one journal value; "" when unreadable. */
    private String safeCandidates() {
        try {
            return NetIfaces.describe(this);
        } catch (RuntimeException e) {
            return "";
        }
    }

    private String safeDiscover() {
        try {
            return NetIfaces.discover(this);
        } catch (RuntimeException e) {
            return "";
        }
    }

    /**
     * The system's own hotspot signal, independent of the interface poll.
     * Android does not guarantee the broadcast on every vendor; when it does
     * not arrive, the address rules in NetIfaces still detect the loss.
     */
    private void startHotspotReceiver() {
        if (hotspotReceiver != null) {
            return;
        }
        BroadcastReceiver receiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                if (intent == null) {
                    return;
                }
                int state = intent.getIntExtra(WifiManager.EXTRA_WIFI_STATE, -1);
                HotspotWatch.Signal signal = HotspotWatch.signal(state);
                String candidates = safeCandidates();
                if (signal == HotspotWatch.Signal.OFF) {
                    boolean waitNow = linkPolicy.hotspotDisabled(
                            SystemClock.elapsedRealtime(), safeDiscover());
                    diagEvent(System.currentTimeMillis(), "hotspot_state",
                            "state", "off",
                            "wifi_state", Integer.toString(state),
                            "candidates", candidates);
                    if (waitNow) {
                        enterWaiting();
                    }
                } else if (signal == HotspotWatch.Signal.ON) {
                    linkPolicy.hotspotEnabled();
                    diagEvent(System.currentTimeMillis(), "hotspot_state",
                            "state", "on",
                            "wifi_state", Integer.toString(state),
                            "candidates", candidates);
                }
            }
        };
        IntentFilter filter = new IntentFilter(HotspotWatch.ACTION);
        if (registerSystemReceiver(receiver, filter)) {
            hotspotReceiver = receiver;
        }
    }

    /**
     * The system's last AP state, when it can be read: the sticky
     * WIFI_AP_STATE_CHANGED intent (AOSP sends that as a normal, non-sticky
     * broadcast, so usually there is none) and the hidden
     * WifiManager.isWifiApEnabled (blocked on some builds). Only an OFF veto
     * for {@link SharerLinkPolicy#hotspotReady(boolean, Boolean)}; the usable
     * interface set remains the gate.
     */
    @SuppressLint("UnspecifiedRegisterReceiverFlag")
    private Boolean hotspotApHint() {
        try {
            Intent sticky = registerReceiver(null, new IntentFilter(HotspotWatch.ACTION));
            if (sticky != null) {
                Boolean on = HotspotWatch.apOn(sticky.getIntExtra(
                        WifiManager.EXTRA_WIFI_STATE, -1));
                if (on != null) {
                    return on;
                }
            }
        } catch (RuntimeException ignored) {
            // Reading the sticky intent is best effort.
        }
        try {
            WifiManager wifi = (WifiManager) getApplicationContext()
                    .getSystemService(Context.WIFI_SERVICE);
            if (wifi == null) {
                return null;
            }
            Method enabled = WifiManager.class.getMethod("isWifiApEnabled");
            Object value = enabled.invoke(wifi);
            if (value instanceof Boolean) {
                return (Boolean) value;
            }
        } catch (Exception | LinkageError ignored) {
            // Hidden API unavailable: the interface set decides alone.
        }
        return null;
    }

    private void unregisterHotspotReceiver() {
        BroadcastReceiver receiver = hotspotReceiver;
        hotspotReceiver = null;
        if (receiver != null) {
            try {
                unregisterReceiver(receiver);
            } catch (RuntimeException ignored) {
                // Already unregistered.
            }
        }
    }

    /**
     * One alerting notification per off period. The foreground notification
     * stays low importance; this one is default importance and is cancelled
     * as soon as the hotspot is back or the user stops sharing.
     */
    private void updateHotspotAlert(boolean waiting) {
        if (waiting) {
            if (hotspotAlert.off()) {
                notifyHotspotOff();
            }
        } else if (hotspotAlert.isAlerted()) {
            hotspotAlert.clear();
            cancelHotspotAlert();
        }
    }

    private void notifyHotspotOff() {
        try {
            NotificationManager nm =
                    (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            if (nm == null) {
                return;
            }
            Notification.Builder b;
            if (Build.VERSION.SDK_INT >= 26) {
                NotificationChannel channel = new NotificationChannel(
                        HOTSPOT_CHANNEL_ID,
                        getString(R.string.hotspot_off_channel_name),
                        NotificationManager.IMPORTANCE_DEFAULT);
                channel.setDescription(getString(R.string.hotspot_off_channel_desc));
                nm.createNotificationChannel(channel);
                b = new Notification.Builder(this, HOTSPOT_CHANNEL_ID);
            } else {
                b = new Notification.Builder(this);
            }
            int piFlags = PendingIntent.FLAG_UPDATE_CURRENT;
            if (Build.VERSION.SDK_INT >= 23) {
                piFlags |= PendingIntent.FLAG_IMMUTABLE;
            }
            PendingIntent open = PendingIntent.getActivity(this, 2,
                    HotspotSettings.intent(this), piFlags);
            b.setContentTitle(getString(R.string.hotspot_off_notification_title))
                    .setContentText(waitingHotspotText())
                    .setSmallIcon(R.drawable.ic_notification)
                    .setContentIntent(open)
                    .setAutoCancel(true)
                    .setOnlyAlertOnce(true);
            nm.notify(HOTSPOT_NOTIFICATION_ID, b.build());
        } catch (RuntimeException ignored) {
            // A notification failure must not affect the session.
        }
    }

    /**
     * The sentence both the card and the notifications use while waiting. A
     * start with no hotspot says "to share"; a hotspot lost mid-share says
     * "to keep sharing".
     */
    private String waitingHotspotText() {
        return getString(linkPolicy.waitingFromStart()
                ? R.string.sharer_start_hotspot
                : R.string.sharer_waiting_hotspot);
    }

    private void cancelHotspotAlert() {
        try {
            NotificationManager nm =
                    (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            if (nm != null) {
                nm.cancel(HOTSPOT_NOTIFICATION_ID);
            }
        } catch (RuntimeException ignored) {
            // Cancelling is best effort.
        }
    }

    /**
     * Observational uplink report on normalized interface sets: any real
     * change is one immediate log line, while only empty-to-non-empty is a
     * link restore and non-empty-to-empty is a link loss. A non-empty swap is
     * logged but counts as neither. It never tears the tunnel down or changes
     * the restart/drop rules.
     */
    private void observeLinkChange(String ifaces) {
        String old = IfaceRefresh.normalize(lastLoggedIfaces);
        String next = IfaceRefresh.normalize(ifaces);
        IfaceRefresh.LinkChange change = IfaceRefresh.linkChange(old, next);
        if (change == IfaceRefresh.LinkChange.NONE) {
            lastLoggedIfaces = next;
            observeDiagIfaces(next);
            return;
        }
        addLog("link", ConnectionLog.linkChangedLine(old, next));
        diagEvent(System.currentTimeMillis(), "link_change",
                "change", change.name(),
                "old", old.isEmpty() ? "none" : old,
                "new", next.isEmpty() ? "none" : next);
        if (change == IfaceRefresh.LinkChange.RESTORE) {
            if (tracker != null) {
                tracker.linkUp();
            }
        } else if (change == IfaceRefresh.LinkChange.LOSS) {
            if (tracker != null) {
                tracker.linkDown();
            }
        }
        lastLoggedIfaces = next;
        observeDiagIfaces(next);
    }

    /** Journal the inferred hotspot/discovery interface set when it changes. */
    private void observeDiagIfaces(String normalized) {
        String next = normalized == null ? "" : normalized;
        if (next.equals(diagLastIfaces)) {
            return;
        }
        String old = diagLastIfaces;
        diagLastIfaces = next;
        // Each usable-set change carries the whole candidate list, so a real
        // phone run shows what still looked usable when the hotspot went off.
        diagEvent(System.currentTimeMillis(), "hotspot_iface",
                "old", old.isEmpty() ? "none" : old,
                "new", next.isEmpty() ? "none" : next,
                "candidates", safeCandidates());
    }

    /**
     * Feed the session tracker one sample and log the events it derives. The
     * tracker compares before/after itself, so only a new drop or a reconnect
     * that just got its first activity produces a line.
     */
    private void sampleSession(UiText.Parsed p) {
        if (tracker == null) {
            return;
        }
        long now = System.currentTimeMillis();
        SessionRecord before = tracker.current();
        tracker.sample(now, p.line, p.rx, p.tx, p.endpoint, UiText.retryReason(p.line));
        SessionRecord after = tracker.current();

        if (p.line != null && !p.line.isEmpty() && !p.line.equals(lastLoggedStatus)) {
            String from = lastLoggedStatus.isEmpty() ? "Idle" : lastLoggedStatus;
            addLog("state", ConnectionLog.stateLine(from, p.line));
            lastLoggedStatus = p.line;
        }

        if (after.drops > before.drops) {
            SessionRecord.Reconnect dropped = lastReconnect(after);
            if (dropped != null) {
                addLog("drop", ConnectionLog.dropLine(
                        dropped.reason, dropped.disconnectedAtMs, dropped.detectedAtMs));
            }
        }

        SessionRecord.Reconnect previous = lastReconnect(before);
        SessionRecord.Reconnect reconnected = lastReconnect(after);
        if (reconnected != null && reconnected.connectedAtMs > 0
                && (previous == null || previous.connectedAtMs != reconnected.connectedAtMs)) {
            String endpoint = after.peerEndpoint == null || after.peerEndpoint.isEmpty()
                    ? "unknown"
                    : after.peerEndpoint;
            addLog("reconnect", ConnectionLog.reconnectLine(
                    reconnected.connectedAtMs, reconnected.durationMs, endpoint));
        }
    }

    private static SessionRecord.Reconnect lastReconnect(SessionRecord record) {
        if (record == null || record.reconnects == null || record.reconnects.isEmpty()) {
            return null;
        }
        return record.reconnects.get(record.reconnects.size() - 1);
    }

    /** A live tunnel must not be torn down just because an address appeared. */
    private boolean isConnected() {
        String raw = getStatusRaw();
        return raw != null
                && UiText.status(UiText.parse(raw).line) == UiText.Status.CONNECTED;
    }

    private boolean hasNativeStatus() {
        if (!running || Native.instance == 0) {
            return false;
        }
        String raw = Native.sharerStatus(Native.instance);
        return raw != null && !raw.isEmpty() && raw.charAt(0) != '\n';
    }

    /**
     * Restart discovery on a changed interface set without touching the
     * code, the stored pair, the run id, the locks or the notification.
     * Every leg must report a live status within one poll, otherwise the
     * previous list is restored; a dead fallback tears down.
     */
    private void restartForIfaces(String ifaces, long now) {
        if (IfaceRefresh.normalize(ifaces).isEmpty()) {
            // The policy only asks for a restart on a non-empty set.
            return;
        }
        String ret = Native.sharerRestart(Native.instance, ifaces);
        if (ret == null || !ret.isEmpty()) {
            SharerLinkPolicy.Recovery recovery = linkPolicy.restartFailed(ret, lastIfaces);
            if (recovery == SharerLinkPolicy.Recovery.FALLBACK) {
                startFallbackRestart(ifaces, ret);
            } else if (recovery == SharerLinkPolicy.Recovery.WAIT) {
                rejectedIfaces = IfaceRefresh.normalize(ifaces);
                pendingIfaces = null;
                pendingIsFallback = false;
                enterWaiting();
            } else {
                failRestart(linkPolicy.stopReason());
            }
            return;
        }
        pendingIfaces = ifaces;
        pendingSinceMs = now;
        pendingIsFallback = false;
        ensureSessionWakeLock();
        ensureMulticastLock();
        ensureRadioLock();
    }

    private void startFallbackRestart(String failedIfaces, String error) {
        String fallback = IfaceRefresh.normalize(lastIfaces);
        if (fallback.isEmpty()) {
            linkPolicy.fallbackFailed(error);
            failRestart(linkPolicy.stopReason());
            return;
        }
        String ret = Native.sharerRestart(Native.instance, fallback);
        rejectedIfaces = IfaceRefresh.normalize(failedIfaces);
        if (ret == null || !ret.isEmpty()) {
            linkPolicy.fallbackFailed(error == null || error.isEmpty()
                    ? (ret == null ? "could not restore discovery" : ret)
                    : error);
            failRestart(linkPolicy.stopReason());
            return;
        }
        linkPolicy.fallbackSucceeded(fallback);
        pendingIfaces = fallback;
        pendingSinceMs = SystemClock.elapsedRealtime();
        pendingIsFallback = true;
        ensureSessionWakeLock();
        ensureMulticastLock();
        ensureRadioLock();
    }

    private void failRestart(String error) {
        lastError = error;
        teardown(error);
        stopForeground(true);
        stopSelf();
    }

    /** Also keeps the connected clock and the stored "last connected" time. */
    private void updateNotification() {
        UiText.Parsed p = UiText.parse(getStatusRaw());
        sampleSession(p);
        UiText.Status status = UiText.displayStatus(p);
        lastParsed = p;
        lastConnected = status == UiText.Status.CONNECTED;
        updateHotspotAlert(linkPolicy.isWaiting());
        trackConnected(status == UiText.Status.CONNECTED, p.session, p.endpoint,
                p.connectedMs);
        // The live name is the pair's display name; it is known from the
        // moment the gateway starts. Never a code or a key here.
        String name = p.peerName.isEmpty() ? pairedName : p.peerName;
        // The native line is the core's; only the connected form adds the
        // peer's name in front of the core word ("Laptop - Connected"). A
        // waiting phone says why and never a link value, even while the
        // native side still reports Connected.
        boolean namedConnected = status == UiText.Status.CONNECTED && !name.isEmpty();
        String text = UiText.notificationText(linkPolicy.isWaiting(),
                waitingHotspotText(),
                namedConnected,
                namedConnected ? getString(R.string.sharer_notif_connected, name) : "",
                UiText.displayLine(p), getString(R.string.sharer_notif_starting));
        String next = notifText.next(text);
        if (next == null) {
            return;
        }
        NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        nm.notify(NOTIFICATION_ID, buildNotification(next));
    }

    private void trackConnected(boolean connected, long session, String endpoint,
            long nativeConnectedMs) {
        // The phone's native clock is the authority; the laptop adopts the same
        // value. A neutral gap clears it natively, so the display restarts from
        // the same first exchange instead of drifting.
        long alignedSince = UiText.connectedSince(SystemClock.elapsedRealtime(),
                nativeConnectedMs, connectedSinceMs);
        boolean newPeerSession = connected && connectedSinceMs != 0
                && lastSessionEpoch >= 0 && session != lastSessionEpoch;
        lastSessionEpoch = session;
        if (newPeerSession) {
            // The peer stopped and came back (or restarted) while data kept
            // flowing: the tracker records it, but the phone's own clock is
            // the authority and does not rewind for the peer's restart.
            long now = System.currentTimeMillis();
            connectedSinceMs = alignedSince;
            saveLastConnected();
            if (tracker != null) {
                tracker.peerRestart(now, "Peer restarted");
            }
            addLog("drop", ConnectionLog.dropLine("Peer restarted", now, now));
            addLog("reconnect", ConnectionLog.reconnectLine(now, 0,
                    endpoint == null || endpoint.isEmpty() ? "unknown" : endpoint));
            diagEvent(now, "peer_restart");
            return;
        }
        if (connected && connectedSinceMs == 0) {
            connectedSinceMs = alignedSince;
            saveLastConnected();
            diagWasConnected = true;
            diagEvent(System.currentTimeMillis(), "connected");
        } else if (connected) {
            // Self-correcting: the native value absorbs the 1 s poll lag.
            connectedSinceMs = alignedSince;
        } else if (!connected) {
            connectedSinceMs = 0;
            if (diagWasConnected) {
                diagWasConnected = false;
                diagEvent(System.currentTimeMillis(), "disconnected");
            }
        }
    }

    private void saveLastConnected() {
        getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                .putLong(KEY_LAST_CONNECTED, System.currentTimeMillis()).apply();
    }

    @SuppressWarnings("deprecation")
    private Notification buildNotification(String text) {
        int piFlags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (Build.VERSION.SDK_INT >= 23) {
            piFlags |= PendingIntent.FLAG_IMMUTABLE;
        }
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class), piFlags);
        Intent stop = new Intent(this, SharerService.class);
        stop.setAction(ACTION_STOP);
        PendingIntent stopPi = PendingIntent.getService(this, 1, stop, piFlags);

        Notification.Builder b;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel channel = new NotificationChannel(CHANNEL_ID,
                    getString(R.string.sharer_channel_name),
                    NotificationManager.IMPORTANCE_LOW);
            channel.setDescription(getString(R.string.sharer_channel_desc));
            ((NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE))
                    .createNotificationChannel(channel);
            b = new Notification.Builder(this, CHANNEL_ID);
        } else {
            b = new Notification.Builder(this);
        }
        b.setContentTitle(getString(R.string.sharer_notif_title))
                .setContentText(text)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentIntent(open)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .addAction(R.drawable.ic_notification, getString(R.string.sharer_notif_stop), stopPi);
        return b.build();
    }

    /**
     * One warning per month on the existing low-importance channel. It is
     * informational: no cutoff, no throttle, and no change to the session.
     */
    private void notifyBudgetWarning(String line) {
        if (line == null || line.isEmpty()) {
            return;
        }
        try {
            NotificationManager nm = (NotificationManager) getSystemService(
                    Context.NOTIFICATION_SERVICE);
            if (nm == null) {
                return;
            }
            Notification.Builder b;
            if (Build.VERSION.SDK_INT >= 26) {
                NotificationChannel channel = new NotificationChannel(CHANNEL_ID,
                        getString(R.string.sharer_channel_name),
                        NotificationManager.IMPORTANCE_LOW);
                channel.setDescription(getString(R.string.sharer_channel_desc));
                nm.createNotificationChannel(channel);
                b = new Notification.Builder(this, CHANNEL_ID);
            } else {
                b = new Notification.Builder(this);
            }
            b.setContentTitle(getString(R.string.budget_notification_title))
                    .setContentText(line)
                    .setSmallIcon(R.drawable.ic_notification)
                    .setAutoCancel(true);
            nm.notify(BUDGET_NOTIFICATION_ID, b.build());
        } catch (RuntimeException ignored) {
            // A notification failure must not affect the session.
        }
    }

    /** Native teardown + locks. Idempotent; safe from onDestroy. */
    private void teardown() {
        teardown("stopped");
    }

    /**
     * Native teardown + locks. The live session is finished and stored once;
     * a second call with no live tracker only releases whatever is left.
     */
    private void teardown(String reason) {
        String stopReason = reason == null || reason.isEmpty() ? "stopped" : reason;
        getSharedPreferences(PREFS, MODE_PRIVATE)
                .edit().putBoolean(KEY_SHARING_LIVE, false).apply();
        // A Stop (or any other teardown) also ends the off period: the alert
        // must not stay behind a session that no longer runs.
        cancelHotspotAlert();
        hotspotAlert.clear();
        linkPolicy.userStop();
        if (running || tracker != null) {
            diagEvent(System.currentTimeMillis(), "session_stop", "reason", stopReason);
        }
        unregisterDiagCallbacks();
        // Persistence is best effort: finishSession catches its own storage
        // failures, and this outer guard keeps a surprise RuntimeException
        // from skipping the native teardown, lock release and running=false.
        try {
            finishSession(stopReason);
        } catch (RuntimeException e) {
            try {
                addLog("history", "history not saved");
            } catch (RuntimeException ignored) {
                // Logging is best effort; cleanup below must still run.
            }
        }
        if (phoneSampler != null) {
            try {
                phoneSampler.onStop(System.currentTimeMillis());
            } catch (RuntimeException ignored) {
                // A store failure must not skip the rest of teardown.
            }
        }
        if (networkMonitor != null && networkMonitorRunning) {
            networkMonitor.stop();
            networkMonitorRunning = false;
        }
        if (networkSampler != null) {
            try {
                networkSampler.onStop(System.currentTimeMillis());
            } catch (RuntimeException ignored) {
                // A store failure must not skip the rest of teardown.
            }
        }
        handler.removeCallbacks(notifyPoll);
        handler.removeCallbacks(ifacePoll);
        lastIfaces = "";
        lastLoggedIfaces = "";
        pendingIfaces = null;
        pendingIsFallback = false;
        rejectedIfaces = null;
        if (connectedSinceMs != 0) {
            saveLastConnected(); // the connection ended now
            connectedSinceMs = 0;
        }
        if (running && Native.instance != 0) {
            Native.destroy();
        }
        running = false;
        code = "";
        pairedName = "";
        if (multicastLock != null && multicastLock.isHeld()) {
            multicastLock.release();
        }
        multicastLock = null;
        if (wifiLock != null && wifiLock.isHeld()) {
            wifiLock.release();
        }
        wifiLock = null;
        if (wakeLock != null && wakeLock.isHeld()) {
            wakeLock.release();
        }
        wakeLock = null;
    }

    /**
     * Close and persist the live session. Storage problems (including an
     * unchecked SecurityException from a file helper) never escape: the
     * completed record stays in the in-memory history and is logged as
     * "history not saved".
     */
    private void finishSession(String reason) {
        SessionTracker finished = tracker;
        if (finished == null) {
            return;
        }
        tracker = null;
        SessionRecord record = finished.finish(System.currentTimeMillis(), reason);
        addLog("session", ConnectionLog.sessionEndedLine(reason));
        boolean saved = false;
        try {
            saved = sessionStore != null && sessionStore.append(record);
            if (saved) {
                history = sessionStore.load();
            }
        } catch (RuntimeException e) {
            saved = false;
        }
        if (!saved) {
            addLog("history", "history not saved");
            keepInMemory(record);
        }
    }

    /** Newest-first in-memory fallback when the session file cannot be used. */
    private void keepInMemory(SessionRecord record) {
        if (history == null) {
            history = new ArrayList<>();
        }
        history.add(0, record);
        if (history.size() > SessionStore.MAX_SESSIONS) {
            history.subList(SessionStore.MAX_SESSIONS, history.size()).clear();
        }
    }

    private void stopSharing() {
        teardown();
        stopForeground(true);
        stopSelf();
    }

    /**
     * The system restarted the service after killing the process. Resume the
     * stored pair only when the opt-in setting allows it; a code is never
     * resurrected and a user Stop always wins.
     */
    private boolean resumeAfterSystemRestart() {
        // ensure() returns the instance handle; 0 means it could not be made.
        if (Native.ensure(this) == 0) {
            return false;
        }
        boolean hasPair = !Native.pairedName(Native.instance).isEmpty();
        boolean resume = Autostart.loadResume(Autostart.shared(this));
        if (!StartPolicy.shouldStart(StartPolicy.Trigger.SERVICE_RESTART,
                resume, hasPair, termsAccepted(), isUserStopped())) {
            Native.destroy();
            return false;
        }
        String startingText = getString(R.string.sharer_notif_starting_reason,
                StartReason.SYSTEM_RESTART.wire());
        notifText.shown(startingText);
        Notification starting = buildNotification(startingText);
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFICATION_ID, starting,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
        } else {
            startForeground(NOTIFICATION_ID, starting);
        }
        beginSharing(false, StartReason.SYSTEM_RESTART);
        return running;
    }

    /** START_STICKY only while a session that may be resumed is live. */
    private int restartPolicy() {
        // A pairing code is ephemeral: never ask for a restart of a code run.
        if (!running || !code.isEmpty()) {
            return START_NOT_STICKY;
        }
        boolean resume = Autostart.loadResume(Autostart.shared(this));
        boolean hasPair = !Native.pairedName(Native.instance).isEmpty();
        return StartPolicy.shouldStart(StartPolicy.Trigger.SERVICE_RESTART,
                resume, hasPair, termsAccepted(), isUserStopped())
                ? START_STICKY
                : START_NOT_STICKY;
    }

    private boolean termsAccepted() {
        return Terms.accepted(getSharedPreferences(PREFS, MODE_PRIVATE)
                .getInt(Terms.PREF_KEY, 0));
    }

    /** Persisted so a Stop still wins if the process dies during the stop. */
    private boolean isUserStopped() {
        return getSharedPreferences(PREFS, MODE_PRIVATE)
                .getBoolean(KEY_USER_STOPPED, false);
    }

    private void setUserStopped(boolean stopped) {
        getSharedPreferences(PREFS, MODE_PRIVATE)
                .edit().putBoolean(KEY_USER_STOPPED, stopped).apply();
    }

    @Override
    public void onDestroy() {
        teardown();
        restoreDiagUncaughtHandler();
        closeDiag();
        super.onDestroy();
    }

    // --- Diagnostic journal (additive; sharing never depends on it) ---

    /** Create the journal on first use; a failure just leaves it unavailable. */
    private synchronized void ensureDiag() {
        if (diag != null || diagDisabled) {
            return;
        }
        try {
            diag = new DiagLog(new File(getFilesDir(), "diag"));
        } catch (RuntimeException e) {
            diag = null;
        }
    }

    private synchronized void closeDiag() {
        DiagLog journal = diag;
        diag = null;
        if (journal != null) {
            try {
                journal.close();
            } catch (RuntimeException ignored) {
                // Closing is best effort; the service is going away.
            }
        }
    }

    /** A write failed: stop using the journal, but never affect sharing. */
    private void disableDiag() {
        diagDisabled = true;
        DiagLog journal = diag;
        diag = null;
        if (journal != null) {
            try {
                journal.close();
            } catch (RuntimeException ignored) {
                // Nothing left to flush.
            }
        }
    }

    private synchronized void journalStats(long nowMs, Map<String, String> fields) {
        DiagLog journal = diag;
        if (journal == null) {
            return;
        }
        try {
            journal.stats(nowMs, fields);
        } catch (RuntimeException e) {
            disableDiag();
        }
    }

    private synchronized void journalEvent(long nowMs, String name, Map<String, String> fields) {
        DiagLog journal = diag;
        if (journal == null) {
            return;
        }
        try {
            journal.event(nowMs, name, fields);
        } catch (RuntimeException e) {
            disableDiag();
        }
    }

    private void diagEvent(long nowMs, String name, String... keysAndValues) {
        LinkedHashMap<String, String> fields = new LinkedHashMap<>();
        if (keysAndValues != null) {
            for (int i = 0; i + 1 < keysAndValues.length; i += 2) {
                fields.put(keysAndValues[i], keysAndValues[i + 1]);
            }
        }
        journalEvent(nowMs, name, fields);
    }

    /**
     * One journal sample, driven by the existing 1 s notify poll: drain native
     * events, then (when the cadence says so) write a STATS record. Never
     * throws into the poll; a collection failure disables the journal only.
     */
    private void sampleDiag(long nowMs) {
        try {
            collectDiag(nowMs);
        } catch (RuntimeException e) {
            diagEvent(nowMs, "diag_error", "stage", "collect");
            disableDiag();
        }
    }

    private void collectDiag(long nowMs) {
        if (!running || diag == null || diagDisabled) {
            return;
        }
        long instance = Native.instance;
        if (instance == 0) {
            return;
        }
        String raw;
        try {
            raw = Native.sharerDiag(instance);
        } catch (RuntimeException e) {
            diagEvent(nowMs, "diag_error", "stage", "native");
            return;
        }
        if (raw == null || raw.isEmpty()) {
            return;
        }

        LinkedHashMap<String, String> stats = null;
        for (String line : raw.split("\n")) {
            if (stats == null && line.startsWith("STATS")) {
                stats = DiagExport.parseStatsLine(line);
            } else if (line.startsWith("EVENT ")) {
                journalNativeEvent(line);
            }
        }

        updateDiagPeerRx(stats);
        if (stats != null && DiagSampler.statsDue(nowMs, diagLastStatsMs, diagLastPeerRxMs)) {
            addJavaStats(stats);
            journalStats(nowMs, stats);
            diagLastStatsMs = Long.valueOf(nowMs);
        }
    }

    /** Native events already carry their own UTC epoch-ms timestamp and name. */
    private void journalNativeEvent(String line) {
        long ts = System.currentTimeMillis();
        String name = "native";
        LinkedHashMap<String, String> fields = new LinkedHashMap<>();
        String body = line.substring("EVENT".length()).trim();
        for (String token : body.split(" ")) {
            int eq = token.indexOf('=');
            if (eq <= 0) {
                continue;
            }
            String key = token.substring(0, eq);
            String value = token.substring(eq + 1);
            if ("ts".equals(key)) {
                try {
                    ts = Long.parseLong(value);
                } catch (NumberFormatException ignored) {
                    // Keep the local time when the native clock is unreadable.
                }
            } else if ("event".equals(key)) {
                if (!value.isEmpty()) {
                    name = value;
                }
            } else {
                fields.put(key, value);
            }
        }
        journalEvent(ts, name, fields);
    }

    /** Native last_rx_ms is absolute epoch ms; -1 or absent means unknown. */
    private void updateDiagPeerRx(Map<String, String> stats) {
        if (stats == null) {
            diagLastPeerRxMs = null;
            return;
        }
        String raw = stats.get("last_rx_ms");
        if (raw == null) {
            diagLastPeerRxMs = null;
            return;
        }
        try {
            long value = Long.parseLong(raw);
            diagLastPeerRxMs = value < 0 ? null : Long.valueOf(value);
        } catch (NumberFormatException e) {
            diagLastPeerRxMs = null;
        }
    }

    /** Java-side context appended after the native fields, in fixed order. */
    private void addJavaStats(LinkedHashMap<String, String> stats) {
        PowerManager pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
        // Mobile counters are the uplink this phone shares; -1 = unsupported.
        stats.put("uplink_rx_bytes", Long.toString(TrafficStats.getMobileRxBytes()));
        stats.put("uplink_tx_bytes", Long.toString(TrafficStats.getMobileTxBytes()));
        stats.put("wakelock", wakeLock != null && wakeLock.isHeld() ? "yes" : "no");
        // The Wi-Fi lock is held for the served part of the session (RE: both
        // reference products keep one for the whole serving period).
        stats.put("wifilock", wifiLock != null && wifiLock.isHeld() ? "yes" : "no");
        stats.put("interactive", pm == null ? "unknown" : (pm.isInteractive() ? "yes" : "no"));
        stats.put("device_idle", deviceIdle(pm));
        stats.put("power_save", pm == null ? "unknown" : (pm.isPowerSaveMode() ? "yes" : "no"));
        addNetworkStats(stats);
        stats.put("worker_notify", Long.toString(diagWorkerNotify));
        stats.put("worker_iface", Long.toString(diagWorkerIface));
    }

    private static String deviceIdle(PowerManager pm) {
        if (pm == null || Build.VERSION.SDK_INT < 23) {
            return "unknown";
        }
        return pm.isDeviceIdleMode() ? "yes" : "no";
    }

    /** The current transport word, from the existing detection helpers. */
    @SuppressWarnings("deprecation")
    private String currentTransport() {
        ConnectivityManager cm =
                (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            return "none";
        }
        if (Build.VERSION.SDK_INT >= 23) {
            Network active = cm.getActiveNetwork();
            if (active == null) {
                return "none";
            }
            return transportOf(cm.getNetworkCapabilities(active));
        }
        NetworkInfo info = cm.getActiveNetworkInfo();
        return info == null ? "none" : legacyTransport(info);
    }

    /**
     * Android's thermal status, 0 to 6. It needs API 29; an older phone has
     * no value and the line leaves the key out.
     */
    private int thermalStatus() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return -1;
        }
        PowerManager power = (PowerManager) getSystemService(Context.POWER_SERVICE);
        if (power == null) {
            return -1;
        }
        return power.getCurrentThermalStatus();
    }

    /** The last thermal headroom reading and when it was taken, in ms. */
    private boolean headroomRead;
    private long headroomAtMs;
    private int headroomPct = -1;

    /** How often the headroom forecast is asked for, in ms. */
    private static final long HEADROOM_MS = 10_000L;

    /**
     * Android's 30 s thermal headroom forecast in percent, 100 at severe
     * throttling. It needs API 30; below that, or before Android has a
     * reading (NaN), the value is -1 and the line leaves the key out. The
     * call is cached for 10 s: it is not free.
     */
    private int thermalHeadroomPercent() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return -1;
        }
        long now = SystemClock.elapsedRealtime();
        if (headroomRead && now - headroomAtMs < HEADROOM_MS) {
            return headroomPct;
        }
        PowerManager power = (PowerManager) getSystemService(Context.POWER_SERVICE);
        if (power == null) {
            return -1;
        }
        headroomRead = true;
        headroomAtMs = now;
        float headroom = power.getThermalHeadroom(30);
        if (Float.isNaN(headroom)) {
            headroomPct = -1;
        } else {
            long pct = Math.round(headroom * 100.0f);
            headroomPct = (int) Math.max(0L, Math.min(200L, pct));
        }
        return headroomPct;
    }

    /**
     * The sticky battery broadcast; a null receiver costs no wakeup and needs
     * no permission. Returns null when the system has no value.
     */
    private int[] readStickyBattery() {
        try {
            Intent battery = registerReceiver(
                    null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
            if (battery == null) {
                return null;
            }
            int level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
            int scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
            int temp = battery.getIntExtra(
                    BatteryManager.EXTRA_TEMPERATURE, Integer.MIN_VALUE);
            int pct = level >= 0 && scale > 0 ? level * 100 / scale : -1;
            return new int[] {pct, temp};
        } catch (RuntimeException e) {
            return null;
        }
    }

    @SuppressWarnings("deprecation")
    private void addNetworkStats(LinkedHashMap<String, String> stats) {
        ConnectivityManager cm =
                (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            stats.put("transport", "none");
            stats.put("validated", "unknown");
            return;
        }
        if (Build.VERSION.SDK_INT >= 23) {
            Network active = cm.getActiveNetwork();
            if (active == null) {
                stats.put("transport", "none");
                stats.put("validated", "no");
                return;
            }
            NetworkCapabilities caps = cm.getNetworkCapabilities(active);
            stats.put("transport", transportOf(caps));
            stats.put("validated", caps != null
                    && caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
                    ? "yes" : "no");
        } else {
            NetworkInfo info = cm.getActiveNetworkInfo();
            stats.put("transport", legacyTransport(info));
            stats.put("validated", info != null && info.isConnected() ? "yes" : "no");
        }
    }

    private static String transportOf(NetworkCapabilities caps) {
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
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
            return "vpn";
        }
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH)) {
            return "bluetooth";
        }
        return "other";
    }

    @SuppressWarnings("deprecation")
    private static String legacyTransport(NetworkInfo info) {
        if (info == null) {
            return "none";
        }
        int type = info.getType();
        if (type == ConnectivityManager.TYPE_WIFI) {
            return "wifi";
        }
        if (type == ConnectivityManager.TYPE_MOBILE) {
            return "cellular";
        }
        if (type == ConnectivityManager.TYPE_ETHERNET) {
            return "ethernet";
        }
        return "other";
    }

    /** Network changes are journaled only while this sharer run is active. */
    private void startDiagCallbacks() {
        startDiagNetworkCallback();
        if (diagScreenReceiver == null) {
            BroadcastReceiver screen = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    String action = intent == null ? null : intent.getAction();
                    if (Intent.ACTION_SCREEN_ON.equals(action)) {
                        diagEvent(System.currentTimeMillis(), "screen", "state", "on");
                    } else if (Intent.ACTION_SCREEN_OFF.equals(action)) {
                        diagEvent(System.currentTimeMillis(), "screen", "state", "off");
                    }
                }
            };
            IntentFilter filter = new IntentFilter();
            filter.addAction(Intent.ACTION_SCREEN_ON);
            filter.addAction(Intent.ACTION_SCREEN_OFF);
            if (registerSystemReceiver(screen, filter)) {
                diagScreenReceiver = screen;
            }
        }
        if (diagPowerReceiver == null) {
            BroadcastReceiver power = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    String action = intent == null ? null : intent.getAction();
                    PowerManager pm =
                            (PowerManager) context.getSystemService(Context.POWER_SERVICE);
                    long now = System.currentTimeMillis();
                    if (PowerManager.ACTION_POWER_SAVE_MODE_CHANGED.equals(action)) {
                        boolean on = pm != null && pm.isPowerSaveMode();
                        diagEvent(now, "power_save", "enabled", on ? "yes" : "no");
                    } else if (Build.VERSION.SDK_INT >= 23
                            && PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED.equals(action)) {
                        boolean idle = pm != null && pm.isDeviceIdleMode();
                        diagEvent(now, "device_idle", "enabled", idle ? "yes" : "no");
                    }
                }
            };
            IntentFilter filter = new IntentFilter();
            filter.addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED);
            if (Build.VERSION.SDK_INT >= 23) {
                filter.addAction(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED);
            }
            if (registerSystemReceiver(power, filter)) {
                diagPowerReceiver = power;
            }
        }
    }

    private void startDiagNetworkCallback() {
        if (diagNetworkCallback != null || Build.VERSION.SDK_INT < 24) {
            return;
        }
        ConnectivityManager cm =
                (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            return;
        }
        ConnectivityManager.NetworkCallback callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                diagEvent(System.currentTimeMillis(), "network", "change", "available");
            }

            @Override
            public void onLost(Network network) {
                diagEvent(System.currentTimeMillis(), "network", "change", "lost");
            }

            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities caps) {
                diagEvent(System.currentTimeMillis(), "network",
                        "change", "capabilities",
                        "transport", transportOf(caps),
                        "validated", caps != null
                                && caps.hasCapability(
                                        NetworkCapabilities.NET_CAPABILITY_VALIDATED)
                                ? "yes" : "no");
            }

            @Override
            public void onLinkPropertiesChanged(Network network, LinkProperties lp) {
                String iface = lp == null ? null : lp.getInterfaceName();
                diagEvent(System.currentTimeMillis(), "network",
                        "change", "link_properties",
                        "iface", iface == null || iface.isEmpty() ? "unknown" : iface);
            }
        };
        try {
            cm.registerDefaultNetworkCallback(callback);
            diagNetworkCallback = callback;
        } catch (RuntimeException e) {
            diagNetworkCallback = null;
        }
    }

    private boolean registerSystemReceiver(BroadcastReceiver receiver, IntentFilter filter) {
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                // System broadcasts are documented as wanting RECEIVER_EXPORTED;
                // these actions are protected, so another app cannot spoof them.
                // This is the shared helper, so its hotspot receiver
                // registration keeps this one-word change on merge.
                registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED);
            } else {
                registerSystemReceiverLegacy(receiver, filter);
            }
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    // RECEIVER_EXPORTED/RECEIVER_NOT_EXPORTED exist from API 33 only; below it
    // the two-argument form is the only registration API. The filters are
    // system broadcasts and the 33+ branch above restricts them.
    @SuppressLint("UnspecifiedRegisterReceiverFlag")
    private void registerSystemReceiverLegacy(BroadcastReceiver receiver, IntentFilter filter) {
        registerReceiver(receiver, filter);
    }

    private void unregisterDiagCallbacks() {
        ConnectivityManager.NetworkCallback callback = diagNetworkCallback;
        diagNetworkCallback = null;
        if (callback != null) {
            try {
                ConnectivityManager cm =
                        (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
                if (cm != null) {
                    cm.unregisterNetworkCallback(callback);
                }
            } catch (RuntimeException ignored) {
                // Already unregistered or the manager is gone.
            }
        }
        BroadcastReceiver screen = diagScreenReceiver;
        diagScreenReceiver = null;
        if (screen != null) {
            try {
                unregisterReceiver(screen);
            } catch (RuntimeException ignored) {
                // Already unregistered.
            }
        }
        BroadcastReceiver power = diagPowerReceiver;
        diagPowerReceiver = null;
        if (power != null) {
            try {
                unregisterReceiver(power);
            } catch (RuntimeException ignored) {
                // Already unregistered.
            }
        }
    }

    /**
     * A crash inside the service is journaled before the previous handler runs;
     * the exception is never swallowed.
     */
    private void installDiagUncaughtHandler() {
        if (diagUncaughtHandler != null) {
            return;
        }
        diagPreviousUncaughtHandler = Thread.getDefaultUncaughtExceptionHandler();
        diagUncaughtHandler = new Thread.UncaughtExceptionHandler() {
            @Override
            public void uncaughtException(Thread thread, Throwable error) {
                try {
                    diagEvent(System.currentTimeMillis(), "uncaught",
                            "thread", thread == null ? "unknown" : thread.getName(),
                            "type", error == null ? "unknown" : error.getClass().getName());
                } catch (RuntimeException ignored) {
                    // A journal problem must not replace the original crash.
                }
                Thread.UncaughtExceptionHandler previous = diagPreviousUncaughtHandler;
                if (previous != null) {
                    previous.uncaughtException(thread, error);
                    return;
                }
                if (error instanceof RuntimeException) {
                    throw (RuntimeException) error;
                }
                if (error instanceof Error) {
                    throw (Error) error;
                }
                throw new RuntimeException(error);
            }
        };
        Thread.setDefaultUncaughtExceptionHandler(diagUncaughtHandler);
    }

    private void restoreDiagUncaughtHandler() {
        Thread.UncaughtExceptionHandler installed = diagUncaughtHandler;
        if (installed != null
                && Thread.getDefaultUncaughtExceptionHandler() == installed) {
            Thread.setDefaultUncaughtExceptionHandler(diagPreviousUncaughtHandler);
        }
        diagUncaughtHandler = null;
        diagPreviousUncaughtHandler = null;
    }

    /**
     * Zip the journal into {@code cache/diag.zip} for the Save diagnostics
     * action. Returns null when the archive cannot be built; the journal
     * sources are never deleted.
     */
    public File exportDiagZip() {
        return exportDiagZip("");
    }

    /**
     * The same archive with the peer aliases the app knows: one peer per line,
     * its spellings separated by tabs. The alias list is used for every entry,
     * so one peer keeps one label across the whole zip.
     */
    public File exportDiagZip(String aliases) {
        try {
            File diagDir = new File(getFilesDir(), "diag");
            File out = new File(getCacheDir(), DiagExport.ZIP_NAME);
            return DiagExport.zip(diagDir, out, aliases);
        } catch (IOException | RuntimeException e) {
            return null;
        }
    }

    // --- Status API for the bound Activity (main thread) ---

    public boolean isRunning() {
        return running;
    }

    /** True while the session is alive but no usable LAN interface exists. */
    public boolean isWaitingForHotspot() {
        return running && linkPolicy.isWaiting();
    }

    /** True when Share started with the hotspot off and it has not appeared. */
    public boolean isWaitingForHotspotAtStart() {
        return running && linkPolicy.waitingFromStart();
    }

    /** Current pairing code, or "" when not sharing or serving the stored pair. */
    public String getCode() {
        return code;
    }

    /** Identifies this sharer run; the Activity logs the run only once. */
    public long getRunSeq() {
        return runSeq;
    }

    /** Redraw the notification from the native state (the Activity's poll). */
    public void refreshNotification() {
        if (running) {
            updateNotification();
        }
    }

    /** Raw native sharer status (six lines, endpoint last), or null when not sharing. */
    public String getStatusRaw() {
        if (!running || Native.instance == 0) {
            return null;
        }
        return Native.sharerStatus(Native.instance);
    }

    /** Code expiry line ("Expires in m:ss" / "Code expired"), or "" when none. */
    public String getCodeExpiry() {
        if (!running || Native.instance == 0) {
            return "";
        }
        return nonNull(Native.sharerCodeExpiry(Native.instance));
    }

    /** Whether the code on screen has outlived its lifetime. */
    public boolean getCodeExpired() {
        return running && Native.instance != 0
                && Native.sharerCodeExpired(Native.instance);
    }

    /** Health verdict under the status, or "" when there is nothing to say. */
    public String getHealthLine() {
        if (!running || Native.instance == 0) {
            return "";
        }
        return nonNull(Native.sharerHealthLine(Native.instance));
    }

    /** Handshake age for the Details view, or "" when there is none. */
    public String getHealthDetail() {
        if (!running || Native.instance == 0) {
            return "";
        }
        return nonNull(Native.sharerHealthDetail(Native.instance));
    }

    /** Retry line while the tunnel is retrying, or "" otherwise. */
    public String getRetryLine() {
        if (!running || Native.instance == 0) {
            return "";
        }
        return nonNull(Native.sharerRetryLine(Native.instance));
    }

    private static String nonNull(String text) {
        return text == null ? "" : text;
    }

    /** elapsedRealtime when the current connection came up; 0 when not connected. */
    public long getConnectedSinceMs() {
        return connectedSinceMs;
    }

    /** The last phone summary line (M0) the sampler wrote; "" when none. */
    public String phoneSummaryLine() {
        return lastPhoneSummary;
    }

    /** The live session, or null when no sharer run is active. */
    public SessionRecord getCurrentSession() {
        return tracker == null ? null : tracker.current();
    }

    /** Completed sessions, newest first (at most 50). */
    public List<SessionRecord> getSessionHistory() {
        return new ArrayList<>(history);
    }

    /** The whole bounded connection log, no trailing newline. */
    public String getLogText() {
        return connectionLog.text();
    }

    /** The last {@code n} connection-log lines, oldest first. */
    public List<String> getLogTail(int n) {
        return connectionLog.tail(n);
    }

    /** Add one timestamped event to the connection log (never a key or code). */
    public void addLog(String kind, String text) {
        connectionLog.push(kind, text, System.currentTimeMillis());
    }

    /**
     * Forget the paired device, also while sharing: stop the sharer first,
     * then delete the record and its key. Runs here, in order, on the main
     * thread (a stop sent as an intent would arrive after the delete).
     * Returns "" when done, otherwise the error text.
     */
    public String forgetPair() {
        teardown();
        stopForeground(true);
        stopSelf();
        long inst = Native.ensure(this);
        String error = inst == 0 ? "could not create native instance" : Native.forgetPeer(inst);
        getSharedPreferences(PREFS, MODE_PRIVATE).edit().remove(KEY_LAST_CONNECTED).apply();
        return error == null ? "" : error;
    }

    /** Returns the last start error once (null if none; may be empty), then clears it. */
    public String consumeError() {
        String e = lastError;
        lastError = null;
        return e;
    }
}
