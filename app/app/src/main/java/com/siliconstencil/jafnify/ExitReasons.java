package com.siliconstencil.jafnify;

import android.annotation.TargetApi;
import android.app.ActivityManager;
import android.app.ApplicationExitInfo;
import android.content.Context;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Process;
import android.os.SystemClock;

import java.util.List;

/**
 * Why this app's previous process ended, read once when sharing starts and
 * handed to the native side. Android 11 (API 30) is the first version that
 * can tell it; below that the phone simply never sends a reason, and an older
 * laptop never sees the key.
 */
final class ExitReasons {
    /** The first Android version with getHistoricalProcessExitReasons. */
    static final int FIRST_SDK = Build.VERSION_CODES.R;

    /** The first Android version with Process.getStartElapsedRealtime. */
    static final int FIRST_START_SDK = Build.VERSION_CODES.N;

    /** A reason older than this belongs to an earlier run, not this session. */
    static final long RECENT_MS = 10 * 60 * 1000L;

    /**
     * Android's ApplicationExitInfo.REASON_PACKAGE_UPDATED. The constant
     * itself needs API 31 and never changes; a local value keeps the install
     * check usable on API 30.
     */
    static final int REASON_PACKAGE_UPDATED = 16;

    /**
     * An install this close to the process start is what started this
     * process. Two minutes covers a slow package install and the service
     * restart after it. Android does not always keep a PACKAGE_UPDATED exit
     * record for the replaced process, so the package's own install time is
     * the reliable signal.
     */
    static final long UPDATE_SLACK_MS = 2 * 60 * 1000L;

    /** One exit: {reason, timestamp millis}, or null when unknown. */
    interface Reader {
        long[] read();
    }

    /** Where the reason goes (the native bridge in production). */
    interface Sink {
        void report(int reason, long ageMs);
    }

    private ExitReasons() {
    }

    /** Whether an exit of this age still describes the session that runs now. */
    static boolean within(long ageMs) {
        return ageMs >= 0 && ageMs <= RECENT_MS;
    }

    /**
     * The wall-clock start of this process, from the wall clock and the two
     * monotonic clocks Android reports: elapsed realtime now and at the
     * process start. 0 when the clocks do not add up.
     */
    static long processStartWall(long nowMs, long elapsedRealtimeMs, long startElapsedRealtimeMs) {
        if (startElapsedRealtimeMs < 0 || elapsedRealtimeMs < startElapsedRealtimeMs) {
            return 0;
        }
        return nowMs - (elapsedRealtimeMs - startElapsedRealtimeMs);
    }

    /**
     * Whether the package was installed so close to this process's start
     * that the install is what started it. A first install also passes; the
     * laptop has no earlier outage for it, so the answer is harmless.
     */
    static boolean updatedSince(long lastUpdateTimeMs, long processStartMs) {
        return lastUpdateTimeMs > 0 && processStartMs > 0
                && lastUpdateTimeMs >= processStartMs - UPDATE_SLACK_MS;
    }

    static void reportOnce(int sdk, Reader reader, Sink sink) {
        reportOnce(sdk, reader, sink, System.currentTimeMillis());
    }

    /**
     * Report the previous exit at most once, and only where Android can tell
     * it and only when it is recent. Any failure is silence: this is
     * information, never a reason to break starting the sharer.
     */
    static void reportOnce(int sdk, Reader reader, Sink sink, long nowMs) {
        if (sdk < FIRST_SDK || reader == null || sink == null) {
            return;
        }
        long[] value;
        try {
            value = reader.read();
        } catch (RuntimeException e) {
            return;
        }
        if (value == null || value.length < 2) {
            return;
        }
        long age = nowMs - value[1];
        if (!within(age)) {
            return;
        }
        sink.report((int) value[0], age);
    }

    /**
     * The system reader. It prefers the package's own install time: a process
     * that began at an install was replaced by an update, and Android does
     * not always keep a PACKAGE_UPDATED exit record for the replaced process.
     * Otherwise it reads the historical exit reason as before.
     */
    static Reader systemReader(final Context context) {
        return new Reader() {
            @Override
            @TargetApi(Build.VERSION_CODES.R)
            public long[] read() {
                long nowMs = System.currentTimeMillis();
                long updatedAt = lastUpdateTime(context);
                if (updatedAt > 0 && Build.VERSION.SDK_INT >= FIRST_START_SDK) {
                    long startMs = processStartWall(nowMs, SystemClock.elapsedRealtime(),
                            Process.getStartElapsedRealtime());
                    if (updatedSince(updatedAt, startMs)) {
                        return new long[] {REASON_PACKAGE_UPDATED, updatedAt};
                    }
                }
                return historicalReason(context);
            }
        };
    }

    /** The package's install time, or 0 when it cannot be read. */
    private static long lastUpdateTime(Context context) {
        try {
            PackageManager manager = context.getPackageManager();
            PackageInfo info = manager.getPackageInfo(context.getPackageName(), 0);
            return info == null ? 0 : info.lastUpdateTime;
        } catch (RuntimeException e) {
            return 0;
        } catch (PackageManager.NameNotFoundException e) {
            return 0;
        }
    }

    /** The newest ApplicationExitInfo, or null when the system has none. */
    @TargetApi(Build.VERSION_CODES.R)
    private static long[] historicalReason(Context context) {
        ActivityManager manager =
                (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        if (manager == null) {
            return null;
        }
        List<ApplicationExitInfo> reasons =
                manager.getHistoricalProcessExitReasons(context.getPackageName(), 0, 1);
        if (reasons == null || reasons.isEmpty()) {
            return null;
        }
        ApplicationExitInfo info = reasons.get(0);
        return new long[] {info.getReason(), info.getTimestamp()};
    }
}
