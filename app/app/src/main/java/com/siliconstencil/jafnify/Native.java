package com.siliconstencil.jafnify;

public class Native {
    static {
        System.loadLibrary("wgserv");
    }

    /// Current native instance handle; set by MainActivity after create().
    /// The service frees the instance on its own thread, so the handle is read
    /// from another thread: it has to be volatile.
    public static volatile long instance = 0;

    public static native long create();

    /// Stop and deallocate the instance
    /// May be called from other thread
    public static synchronized void destroy() {
        long handle = instance;
        // Zero the handle before the native free, so another thread can never
        // pass the freed value: a zero or stale handle is a native no-op.
        instance = 0;
        if (handle != 0) {
            destroy(handle);
        }
    }

    /// Native free; use destroy() so the handle is zeroed first.
    private static native void destroy(long instance);

    /// Starts the sharer (gateway): generates a pairing code, returns it
    /// (the peer must enter it), and runs the WireGuard server + router.
    /// `ifaces`: comma-separated IPv4 addresses of the interfaces discovery
    /// runs on (see NetIfaces); "" = the kernel's default route.
    public static native String sharerStart(long instance, String ifaces);

    /// Status lines: status / peer id / peer name / rx / tx / endpoint.
    public static native String sharerStatus(long instance);

    /// The core's idle status line ("Not sharing"), before any run reports.
    public static native String idleStatus();

    /// The line under the status on the first open after the system closed
    /// the app while sharing; "" when there is nothing to say. Core wording.
    public static native String stoppedLine(int reason, long ageMs);

    /// The one line while the phone shares with notifications off. Core
    /// wording: the notification's Stop action is then missing.
    public static native String notificationsOffLine();

    /// The About row and the notices screen title. Core wording.
    public static native String thirdPartyNoticesTitle();

    /// The flag table for Android, one line per row: id, bucket, default,
    /// platforms, label and help separated by TAB; lines separated by \n.
    /// "" while no row is wired.
    public static native String flagsMeta();

    /// The bucket table: key, title, collapsed (1/0) and note separated by
    /// TAB; lines separated by \n, in render order.
    public static native String flagsBucketsMeta();

    /// Build the phone summary line from the sampler's numbers. The core
    /// formatter owns the bytes. `thermalHeadroom` is the 30 s forecast in
    /// percent, or -1 when Android does not report it.
    public static native String phoneStatsLine(long upBps, long downBps,
            String network, int signal, int dbm, int batteryPct, int tempC10,
            long usedBytes, long allowBytes, int resetDay, int monthKey, int switches,
            boolean withDns, int thermalStatus, int thermalHeadroom,
            boolean networkPermission);

    /// The build the phone summary line will carry, from Android's
    /// ApplicationExitInfo: `reason` is getReason() and `ageMs` how long ago
    /// the previous process died. The core keeps it for one summary.
    public static native void setExitReason(int reason, long ageMs);

    /// Feed or clear (null/empty) the line the tunnel answers with. The same
    /// line feeds the ring of the phone's own series; an empty line forgets it.
    public static native void setPhoneStats(long instance, String line);

    /// The main-screen phone lines from the last summary line; "" without one.
    public static native String phoneSection(String line, boolean showDns,
            boolean showSwitches, boolean showBattery);

    /// The home model of the phone as one versioned text block: the header,
    /// the card, then one line per tile. The card is there even when the
    /// summary line does not parse: the phone still says what it is doing.
    /// The uptime is the time the sharing has been up, not the time the phone
    /// has been on. The state is 0 idle, 1 trying and 2 sharing; the state
    /// line is what the phone is doing while the tunnel is not up.
    /// The scopes are the words of the core ("SESSION", "TODAY", "24 H");
    /// the session is one line ("up=.. down=.. peak=.. start=..").
    public static native String phoneHome(long instance, String summaryLine,
            String usageLines, String facts, String stateLine, String notice,
            long nowMs, int offsetMinutes, long uptimeSecs, int state,
            boolean showDns, boolean showBattery, String todayScope,
            String speedScope, String session, String averageScope,
            String layout, boolean networkPermission);

    /// The always-on flag ids, one per line. The phone reads a stored choice
    /// only for the rows this list does not name.
    public static native String standardFlags();

    /// The placeable phone tiles: id TAB label, one per line.
    public static native String homeTileCatalog();

    /// The default phone home layout, as the saved text.
    public static native String homeLayoutDefault();

    /// Normalize a phone layout from the grid word, the wide word and the
    /// comma-separated ids; returns the saved text.
    public static native String homeLayoutNormalize(String grid, String wide,
            String ids);

    /// The next scope of a tile, in the words of the core. The cycle belongs
    /// to the core; the phone only stores the choice.
    public static native String nextTileScope(String tileId, String scope);

    /// The words that say a tile can be tapped to change its scope.
    public static native String tileTapHint();

    /// One budget record line; the phone stores this exact text.
    public static native String budgetRecord(long allowBytes, long usedBytes,
            int resetDay, int monthKey, int warnedMonth);

    /// The read-only budget line from a stored record; "" when it is invalid.
    public static native String budgetDisplay(String record);

    /// The fixed 80 percent warning line, or "" when it is not due.
    public static native String budgetWarning(String record);

    /// The battery lines for a level and a temperature, newline separated.
    public static native String batteryLines(int levelPct, int tempC10);

    /// Feed one usage sample; returns a completed bucket line or "".
    public static native String usageStep(long atMs, long upBytes,
            long downBytes, boolean connected, String network);

    /// The current bucket line, or "" before the first sample.
    public static native String usageCurrent();

    /// Drop the in-memory usage sampler; the next ON period starts fresh.
    public static native void usageReset();

    /// Continue the stored usage bucket of this hour after a restart.
    public static native void usageSeed(String line);

    /// The six summary lines for one window (1, 7 or 30 days), newline
    /// separated, from the stored usage lines. Never null.
    public static native String usageSummary(String lines, int windowDays,
            long nowMs, int offsetMinutes);

    /// The phone's network report: this hour, the last 24 hours and the
    /// signal line, in core wording. Never null.
    public static native String networkReport(String networkLines,
            String signalLines, long nowMs, int offsetMinutes);

    /// The core wire word of a modem kind ("5g-nsa"); empty when unknown.
    public static native String generationWord(String kind);

    /// The best-spot lines of the phone screen: the live line, one line per
    /// marked spot and the verdict, newline separated. The samples blob is
    /// one sample per line ("atMs,net,level,dbm,rttMs,wifiMs") and the marks
    /// blob one mark per line ("atMs,number").
    public static native String bestSpotLines(String samples, String marks);

    /// The length of a best-spot session in milliseconds. The core owns the
    /// number, so the phone never decides a session length of its own.
    public static native long bestSpotSessionMs();

    /// The device panel lines of the phone screen, newline separated, in core
    /// wording. The facts blob is "key=value;key=value".
    public static native String devicePanel(String facts);

    /// True when the phone moved from one known generation to another.
    public static native boolean generationSwitch(String previous, String current);

    /// The canonical network-report line for the given fields; empty when the
    /// fields do not parse.
    public static native String networkBucketLine(String fields);

    /// The signal sample line when the five-minute gate allows it, empty
    /// otherwise.
    public static native String signalSample(long atMs, int level, long lastAtMs);

    /// The charge counter when it is sane, -1 otherwise.
    public static native long saneMah(long counter);

    /// The stored same-hour network bucket as counters; empty when unparsable.
    public static native long[] networkSeed(String line);

    /// The connection details of the phone, one readable line per row
    /// ("Path: ...", "MTU: 1280", "DNS: through the tunnel", ...). Never null.
    public static native String connectionDetails(String healthLine,
            String endpoint);

    /// The line under the pairing code: "" when no code is on screen,
    /// "Code expired" once its lifetime is over, else "Expires in m:ss".
    public static native String sharerCodeExpiry(long instance);

    /// Whether the code on screen has outlived its lifetime.
    public static native boolean sharerCodeExpired(long instance);

    /// The health verdict under the status: "" when there is nothing to say.
    public static native String sharerHealthLine(long instance);

    /// The handshake age for the Details view: "" when there is none.
    public static native String sharerHealthDetail(long instance);

    /// The retry line while the tunnel is retrying: "" otherwise.
    public static native String sharerRetryLine(long instance);

    /// Diagnostic snapshot for the journal: the first line is STATS with the
    /// common counters and current peer/local addresses; zero or more EVENT
    /// lines follow (session/reset/no-data/socket-rebind reasons). Values are
    /// whitespace-free; no pairing code, key or packet payload is included.
    public static native String sharerDiag(long instance);

    /// Redact an export built in Java (the connection log and the Java half of
    /// the journal) with the core rules. Pass the whole text at once, so one
    /// peer keeps one label across every line. `aliases` has one peer per
    /// line; the spellings of that peer are separated by tabs. Labels are
    /// assigned in line order (first line = peer-1).
    public static native String redactForExport(String text, String aliases);

    /// Where the paired device is kept (a file in getFilesDir()); call before
    /// any of the pair functions below.
    public static native void setStatePath(long instance, String path);

    /// Short name of the paired device; "" when none is stored.
    public static native String pairedName(long instance);

    /// Starts the sharer for the stored pair, no code. Empty string = started,
    /// otherwise the error.
    /// `ifaces` as for sharerStart.
    public static native String sharerStartKnown(long instance, String ifaces);

    /// Restart the sharer on a new interface list without recreating the
    /// native instance. It stays on the current track: the same code while
    /// pairing, or the stored pair once one is in use. Empty string =
    /// restarted, otherwise the error.
    public static native String sharerRestart(long instance, String ifaces);

    /// Stops the sharer and deletes the stored pair. Empty string = done.
    public static native String forgetPeer(long instance);

    /// Debug probe: discovery and
    /// the key exchange on `ifaces` ("name=ip/prefix,..."), no tunnel, UDP
    /// 9797 never bound. Returns the code the laptop runs `jafnify probe
    /// --code` with, or "error: ..." (starts with "error").
    public static native String probeStart(long instance, String ifaces, int seconds);

    /// The probe report so far, one line each.
    public static native String probeStatus(long instance);

    public static native boolean probeRunning(long instance);

    /// The native instance, created on first use with the state path set.
    /// Synchronized: two threads must not create two instances (one leaked).
    public static synchronized long ensure(android.content.Context context) {
        if (instance == 0) {
            instance = create();
            if (instance != 0) {
                setStatePath(instance,
                        new java.io.File(context.getFilesDir(), "state.toml").getPath());
            }
        }
        return instance;
    }
}
