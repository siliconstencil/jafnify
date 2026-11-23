package com.siliconstencil.jafnify;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

/**
 * The phone's usage buckets as core lines, one per hour. Writes are atomic and
 * owner-only; the file is local and never exported.
 */
public final class UsageLineStore
        implements PhoneSampler.UsageSink, PhoneSampler.BudgetStore,
        NetworkSampler.Store {
    private static final int MAX_LINES = 720;
    private static final int MAX_SIGNAL_LINES = 288;
    private static final String USAGE_PREFIX = "JFU1 ";
    private static final String NETWORK_PREFIX = "JFN1 ";
    private static final String SIGNAL_PREFIX = "JSG1 ";
    private static final String BUDGET_PREFIX = "JFB1 ";

    private static final Comparator<String> BY_HOUR = new Comparator<String>() {
        @Override
        public int compare(String a, String b) {
            return Long.compare(hourOf(a), hourOf(b));
        }
    };

    private final File file;

    public UsageLineStore(File file) {
        this.file = file;
    }

    /** The stored usage line of one hour, or null. */
    public synchronized String loadUsageHour(long hourStartMs) {
        return hourLine(USAGE_PREFIX, hourStartMs);
    }

    /** The stored network line of one hour, or null. */
    public synchronized String loadNetworkHour(long hourStartMs) {
        return hourLine(NETWORK_PREFIX, hourStartMs);
    }

    private String hourLine(String prefix, long hourStartMs) {
        for (String line : readAll()) {
            if (!line.startsWith(prefix)) {
                continue;
            }
            Long hour = hourOf(line);
            if (hour != null && hour.longValue() == hourStartMs) {
                return line;
            }
        }
        return null;
    }

    /** The valid usage lines, oldest first. A corrupt file is empty. */
    public synchronized List<String> load() {
        return hours(USAGE_PREFIX);
    }

    /** The network-report hours, oldest first. */
    public synchronized List<String> loadNetwork() {
        return hours(NETWORK_PREFIX);
    }

    /** The signal samples, oldest first. */
    public synchronized List<String> loadSignal() {
        List<String> lines = new ArrayList<>();
        for (String line : readAll()) {
            if (line.startsWith(SIGNAL_PREFIX)) {
                lines.add(line);
            }
        }
        return lines;
    }

    private List<String> hours(String prefix) {
        List<String> lines = new ArrayList<>();
        for (String line : readAll()) {
            if (line.startsWith(prefix) && hourOf(line) != null) {
                lines.add(line);
            }
        }
        Collections.sort(lines, BY_HOUR);
        return lines;
    }

    /** The one budget record, or null before the user sets an allowance. */
    public synchronized String loadBudget() {
        for (String line : readAll()) {
            if (line.startsWith(BUDGET_PREFIX)) {
                return line;
            }
        }
        return null;
    }

    /** Replace the one budget record, keeping every usage line. */
    @Override
    public synchronized void saveBudget(String line) {
        List<String> out = new ArrayList<>();
        boolean added = false;
        for (String old : readAll()) {
            if (old.startsWith(BUDGET_PREFIX)) {
                if (!added && line != null && !line.isEmpty()) {
                    out.add(line);
                    added = true;
                }
            } else {
                out.add(old);
            }
        }
        if (!added && line != null && !line.isEmpty()) {
            out.add(line);
        }
        write(normalize(out));
    }

    /** Add or replace one usage hour, keeping the other families, then cap. */
    @Override
    public synchronized void upsert(String line) {
        upsertHour(line, USAGE_PREFIX);
    }

    /** Add or replace one network-report hour, keeping the usage lines. */
    public synchronized void upsertNetwork(String line) {
        upsertHour(line, NETWORK_PREFIX);
    }

    private void upsertHour(String line, String prefix) {
        Long hour = line == null ? null : hourOf(line);
        if (hour == null || !line.startsWith(prefix)) {
            return;
        }
        List<String> out = new ArrayList<>();
        boolean replaced = false;
        for (String old : readAll()) {
            if (old.startsWith(prefix) && hour.equals(hourOf(old))) {
                out.add(line);
                replaced = true;
            } else {
                out.add(old);
            }
        }
        if (!replaced) {
            out.add(line);
        }
        write(normalize(out));
    }

    /** Append one signal sample; the oldest goes over the day cap. */
    public synchronized void upsertSignal(String line) {
        if (line == null || !line.startsWith(SIGNAL_PREFIX)) {
            return;
        }
        List<String> out = new ArrayList<>(readAll());
        out.add(line);
        write(normalize(out));
    }

    private List<String> readAll() {
        List<String> lines = new ArrayList<>();
        if (!file.isFile()) {
            return lines;
        }
        try (BufferedReader in = new BufferedReader(new InputStreamReader(
                new FileInputStream(file), StandardCharsets.UTF_8))) {
            String line;
            while ((line = in.readLine()) != null) {
                String trimmed = line.trim();
                if (!trimmed.isEmpty()) {
                    lines.add(trimmed);
                }
            }
        } catch (IOException e) {
            return new ArrayList<>();
        }
        return lines;
    }

    /** Keep every family apart: the budget, the usage hours, the network
     *  hours and the signal samples, each capped on its own. */
    private static List<String> normalize(List<String> lines) {
        List<String> budget = new ArrayList<>();
        List<String> others = new ArrayList<>();
        List<String> usage = new ArrayList<>();
        List<String> network = new ArrayList<>();
        List<String> signal = new ArrayList<>();
        for (String line : lines) {
            if (line.startsWith(BUDGET_PREFIX)) {
                if (!budget.contains(line)) {
                    budget.add(line);
                }
            } else if (line.startsWith(SIGNAL_PREFIX)) {
                signal.add(line);
            } else if (line.startsWith(USAGE_PREFIX) && hourOf(line) != null) {
                usage.add(line);
            } else if (line.startsWith(NETWORK_PREFIX) && hourOf(line) != null) {
                network.add(line);
            } else if (!others.contains(line)) {
                others.add(line);
            }
        }
        Collections.sort(usage, BY_HOUR);
        while (usage.size() > MAX_LINES) {
            usage.remove(0);
        }
        Collections.sort(network, BY_HOUR);
        while (network.size() > MAX_LINES) {
            network.remove(0);
        }
        while (signal.size() > MAX_SIGNAL_LINES) {
            signal.remove(0);
        }
        List<String> out = new ArrayList<>();
        out.addAll(budget);
        out.addAll(usage);
        out.addAll(network);
        out.addAll(signal);
        out.addAll(others);
        return out;
    }

    private void write(List<String> lines) {
        File dir = file.getParentFile();
        if (dir != null) {
            dir.mkdirs();
        }
        File tmp = new File(dir, "." + file.getName() + ".tmp");
        try {
            try (FileOutputStream out = new FileOutputStream(tmp)) {
                Writer writer = new OutputStreamWriter(out, StandardCharsets.UTF_8);
                for (String line : lines) {
                    writer.write(line);
                    writer.write("\n");
                }
                writer.flush();
                out.getFD().sync();
            }
            tmp.setReadable(false, false);
            tmp.setReadable(true, true);
            tmp.setWritable(false, false);
            tmp.setWritable(true, true);
            tmp.setExecutable(false, false);
            if (!tmp.renameTo(file)) {
                file.delete();
                if (!tmp.renameTo(file)) {
                    throw new IOException("could not replace the usage file");
                }
            }
        } catch (IOException e) {
            tmp.delete();
        }
    }

    private static Long hourOf(String line) {
        for (String field : line.split(" ")) {
            if (field.startsWith("hour=")) {
                try {
                    return Long.parseLong(field.substring(5));
                } catch (NumberFormatException e) {
                    return null;
                }
            }
        }
        return null;
    }
}
