package com.siliconstencil.jafnify;

import java.util.ArrayList;
import java.util.List;

/**
 * The best-spot session of the phone: the samples, the marks and the blobs the
 * core reads. Pure Java, so every rule runs in a plain JVM test. Nothing is
 * written to a file: the session lives in memory, and the bundle of a
 * configuration change carries it from one screen to the next.
 */
public final class BestSpot {
    /**
     * The value the modem reports for a strength it does not know:
     * android.telephony.CellInfo.UNAVAILABLE and
     * android.telephony.CellSignalStrength.UNAVAILABLE are Integer.MAX_VALUE.
     * The JNI's own unknown marker is Integer.MIN_VALUE, so the two never
     * look alike.
     */
    public static final int UNAVAILABLE = Integer.MAX_VALUE;

    private final List<String> samples = new ArrayList<>();
    private final List<String> marks = new ArrayList<>();
    /** The length of the session in milliseconds; the core owns the number. */
    private final long sessionMs;
    private boolean running;
    private long startedAtMs;

    /** A session that lasts as long as the core says. */
    public BestSpot(long sessionMs) {
        this.sessionMs = sessionMs;
    }

    /** The first valid dBm of a list of cell strengths; -1 when there is none. */
    public static int pickDbm(int[] dbms) {
        if (dbms == null) {
            return -1;
        }
        for (int dbm : dbms) {
            if (dbm != UNAVAILABLE) {
                return dbm;
            }
        }
        return -1;
    }

    /** One sample line: atMs,net,level,dbm,rttMs,wifiMs. */
    public static String sampleLine(long atMs, String net, int level, int dbm, int rttMs) {
        String word = net == null || net.isEmpty() ? "none" : net;
        int signal = level >= 0 && level <= 4 ? level : 255;
        return atMs + "," + word + "," + signal + "," + dbmField(dbm) + ","
                + rttField(rttMs) + ",";
    }

    /** One mark line: atMs,number. */
    public static String markLine(long atMs, int number) {
        return atMs + "," + number;
    }

    /**
     * The core's block split into the live line, the spot lines and the
     * verdict: the live line comes first, the verdict comes last.
     */
    public static String[] splitBlock(String text) {
        if (text == null || text.isEmpty()) {
            return new String[] {"", "", ""};
        }
        String[] lines = text.split("\n", -1);
        if (lines.length == 1) {
            return new String[] {lines[0], "", ""};
        }
        StringBuilder spots = new StringBuilder();
        for (int i = 1; i < lines.length - 1; i++) {
            if (spots.length() > 0) {
                spots.append('\n');
            }
            spots.append(lines[i]);
        }
        return new String[] {lines[0], spots.toString(), lines[lines.length - 1]};
    }

    /** The DNS answer time of the last summary line; -1 when there is none. */
    public static int rttFromSummary(String line) {
        if (line == null || line.isEmpty()) {
            return -1;
        }
        for (String token : line.trim().split("\\s+")) {
            if (!token.startsWith("rtt=")) {
                continue;
            }
            try {
                int value = Integer.parseInt(token.substring(4));
                return value > 0 ? value : -1;
            } catch (NumberFormatException e) {
                return -1;
            }
        }
        return -1;
    }

    /** Start a new session; the old samples and marks are gone. */
    public void start(long nowMs) {
        samples.clear();
        marks.clear();
        startedAtMs = nowMs;
        running = true;
    }

    /** Stop the session; the samples and the marks stay for the last lines. */
    public void stop() {
        running = false;
    }

    /** True while the session collects samples. */
    public boolean running() {
        return running;
    }

    /**
     * True when the session has run its length. The caller passes the time:
     * this class has no clock of its own.
     */
    public boolean expired(long nowMs) {
        return running && nowMs - startedAtMs >= sessionMs;
    }

    /**
     * Add one sample line; a stopped session and a session that has run its
     * length collect nothing.
     */
    public void addSample(String line, long nowMs) {
        if (!running || expired(nowMs) || line == null || line.isEmpty()) {
            return;
        }
        samples.add(line);
    }

    /**
     * Mark this spot; the number of the mark, 0 when the session is off or
     * has run its length.
     */
    public int mark(long atMs) {
        if (!running || expired(atMs)) {
            return 0;
        }
        int number = marks.size() + 1;
        marks.add(markLine(atMs, number));
        return number;
    }

    /**
     * The session state for the bundle: the samples, the marks, the running
     * flag and the start of the session. A rotation must not lose a session.
     */
    public String[] saveState() {
        return new String[] {
                samplesBlob(),
                marksBlob(),
                running ? "1" : "0",
                String.valueOf(startedAtMs),
        };
    }

    /** The session state from the bundle; a broken state leaves the session off. */
    public void restoreState(String[] state) {
        if (state == null || state.length < 4) {
            return;
        }
        samples.clear();
        samples.addAll(lines(state[0]));
        marks.clear();
        marks.addAll(lines(state[1]));
        running = "1".equals(state[2]);
        try {
            startedAtMs = Long.parseLong(state[3].trim());
        } catch (NumberFormatException e) {
            startedAtMs = 0;
            running = false;
        }
    }

    /** The samples blob for the core; one sample per line. */
    public String samplesBlob() {
        return join(samples);
    }

    /** The marks blob for the core; one mark per line. */
    public String marksBlob() {
        return join(marks);
    }

    private static String dbmField(int dbm) {
        return dbm >= -200 && dbm <= -2 ? String.valueOf(dbm) : "";
    }

    private static String rttField(int rttMs) {
        return rttMs > 0 ? String.valueOf(rttMs) : "";
    }

    private static List<String> lines(String blob) {
        List<String> out = new ArrayList<>();
        if (blob == null || blob.isEmpty()) {
            return out;
        }
        for (String line : blob.split("\n")) {
            if (!line.isEmpty()) {
                out.add(line);
            }
        }
        return out;
    }

    private static String join(List<String> lines) {
        StringBuilder text = new StringBuilder();
        for (String line : lines) {
            if (text.length() > 0) {
                text.append('\n');
            }
            text.append(line);
        }
        return text.toString();
    }
}
