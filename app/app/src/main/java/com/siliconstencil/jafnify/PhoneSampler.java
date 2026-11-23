package com.siliconstencil.jafnify;

/**
 * Turns one existing 1 s service tick into phone measurements. Pure Java: all
 * Android, JNI and file access sits behind the interfaces, so the cadence and
 * the OFF guarantees run in a plain JVM test.
 */
public final class PhoneSampler {
    /** The native bridge; production calls Native, tests use a fake. */
    public interface Bridge {
        /** The signal strength is Integer.MIN_VALUE when it is unknown. */
        String summaryLine(long upBps, long downBps, String network, int signal,
                int dbm, int batteryPct, int tempC10, long usedBytes, long allowBytes,
                int resetDay, int monthKey, int switches, boolean withDns,
                int thermalStatus, int thermalHeadroom);

        /** The line the paired laptop may read (null: nothing to share). */
        void setPhoneStats(String line);

        /** The same line for this phone's own main screen (null: hide). */
        default void setLocalSummary(String line) {}

        String usageStep(long atMs, long upBytes, long downBytes,
                boolean connected, String network);

        String usageCurrent();

        void usageReset();

        /** Continue the stored bucket of this hour after a restart. */
        void usageSeed(String line);

        String budgetRecord(long allowBytes, long usedBytes, int resetDay,
                int monthKey, int warnedMonth);

        String budgetWarning(String record);
    }

    /** The flags, read by id. */
    public interface FlagSource {
        boolean enabled(String id);
    }

    /** A sticky battery read: [level percent, temperature tenths C]. */
    public interface BatterySource {
        int[] read();
    }

    /** The usage file; only completed hour lines arrive here. */
    public interface UsageSink {
        void upsert(String line);

        /** The stored usage line of one hour, or null. */
        String loadUsageHour(long hourStartMs);
    }

    /** The one budget record in the same phone file. */
    public interface BudgetStore {
        String loadBudget();

        void saveBudget(String line);
    }

    /** The caller's local date: [year, month 1-12, day 1-31]. */
    public interface DateSource {
        int[] read(long nowMs);
    }

    /** One warning per month reaches this sink. */
    public interface WarningSink {
        void warn(String line);
    }

    private static final long BATTERY_MS = 60_000L;
    private static final long FLUSH_MS = 60_000L;

    private final Bridge bridge;
    private final FlagSource flags;
    private final BatterySource battery;
    private final UsageSink sink;
    private final BudgetStore budgetStore;
    private final DateSource dateSource;
    private final WarningSink warnings;

    private boolean localWasOn;
    private boolean phoneWasOn;
    private int switchesThisHour;
    private int signalLevel = -1;
    private int dbm = Integer.MIN_VALUE;
    private Long lastTickMs;
    private Long lastUpBytes;
    private Long lastDownBytes;
    private Long lastBatteryMs;
    private int batteryPct = -1;
    private int tempC10 = Integer.MIN_VALUE;
    private int thermalStatus = Integer.MIN_VALUE;
    private int thermalHeadroom = Integer.MIN_VALUE;

    private boolean usageWasOn;
    private Long lastFlushMs;

    private boolean budgetWasOn;
    private boolean budgetLoaded;
    private Budget budget = new Budget();
    private Long lastBudgetUp;
    private Long lastBudgetDown;
    private Long lastBudgetFlushMs;

    /** The core network word for a ConnectivityManager transport word. */
    public static String networkWord(String transport) {
        if ("wifi".equals(transport)) {
            return "wifi";
        }
        if ("cellular".equals(transport)) {
            return "cellular";
        }
        if ("ethernet".equals(transport)) {
            return "eth";
        }
        if ("none".equals(transport)) {
            return "none";
        }
        return "other";
    }

    public PhoneSampler(Bridge bridge, FlagSource flags, BatterySource battery,
            UsageSink sink) {
        this(bridge, flags, battery, sink, null, PhoneSampler::systemDate,
                line -> {
                });
    }

    public PhoneSampler(Bridge bridge, FlagSource flags, BatterySource battery,
            UsageSink sink, BudgetStore budgetStore) {
        this(bridge, flags, battery, sink, budgetStore, PhoneSampler::systemDate,
                line -> {
                });
    }

    public PhoneSampler(Bridge bridge, FlagSource flags, BatterySource battery,
            UsageSink sink, BudgetStore budgetStore, DateSource dateSource,
            WarningSink warnings) {
        this.bridge = bridge;
        this.flags = flags;
        this.battery = battery;
        this.sink = sink;
        this.budgetStore = budgetStore;
        this.dateSource = dateSource;
        this.warnings = warnings;
    }

    /** The switch count of the current hour, carried in the M0 summary. */
    public void setSwitches(int switches) {
        switchesThisHour = Math.max(0, switches);
    }

    /** The signal level M0 may carry; -1 when the caller gated it off. */
    public void setSignalLevel(int level) {
        signalLevel = level >= 0 && level <= 4 ? level : -1;
    }

    /** The signal strength in dBm M0 may carry; unknown when it is out of range. */
    public void setDbm(int value) {
        dbm = value >= -200 && value <= -2 ? value : Integer.MIN_VALUE;
    }

    /** Android's thermal status M0 may carry; unknown when it is out of range. */
    public void setThermalStatus(int value) {
        thermalStatus = value >= 0 && value <= 6 ? value : Integer.MIN_VALUE;
    }

    /**
     * Android's 30 s thermal headroom forecast in percent. Unknown (-1) when
     * the phone is below API 30 or Android has no reading yet.
     */
    public void setThermalHeadroom(int value) {
        thermalHeadroom = value >= 0 && value <= 200 ? value : Integer.MIN_VALUE;
    }

    /** One service tick. The caller is the existing 1 s handler. */
    public void onTick(long nowMs, long upBytes, long downBytes,
            boolean connected, String network) {
        boolean phoneOn = flags.enabled("phone_stats");
        boolean carrierOn = flags.enabled("carrier_quality");
        boolean batteryOn = flags.enabled("battery_status");
        boolean budgetOn = flags.enabled("data_budget");

        boolean reportOn = flags.enabled(FlagValues.NETWORK_DETAILS);
        // The main screen shows the phone data whenever any data switch is on;
        // the laptop only gets it while "share with the computer" is on.
        boolean localOn = phoneOn || batteryOn || budgetOn || carrierOn || reportOn;

        if (phoneOn || batteryOn) {
            readBatteryIfDue(nowMs);
        }

        if (budgetOn) {
            onBudgetTick(nowMs, upBytes, downBytes, network);
        } else if (budgetWasOn) {
            flushBudget(nowMs, true);
            budgetWasOn = false;
            lastBudgetUp = null;
            lastBudgetDown = null;
            lastBudgetFlushMs = null;
        }

        if (localOn) {
            long upRate = 0;
            long downRate = 0;
            if (lastTickMs != null && nowMs > lastTickMs
                    && upBytes >= lastUpBytes && downBytes >= lastDownBytes) {
                long elapsed = nowMs - lastTickMs;
                upRate = (upBytes - lastUpBytes) * 1000L / elapsed;
                downRate = (downBytes - lastDownBytes) * 1000L / elapsed;
            }
            long used = budgetOn ? budget.usedBytes : 0;
            long allow = budgetOn ? budget.allowBytes : 0;
            int reset = budgetOn ? budget.resetDay : 0;
            int month = budgetOn ? budget.monthKey : 0;
            // The heat travels under the same switches as the temperature.
            int thermal = (phoneOn || batteryOn) ? thermalStatus : Integer.MIN_VALUE;
            int headroom = (phoneOn || batteryOn) ? thermalHeadroom : Integer.MIN_VALUE;
            String line = bridge.summaryLine(
                    upRate, downRate, network, signalLevel, dbm, batteryPct, tempC10,
                    used, allow, reset, month, switchesThisHour, carrierOn, thermal, headroom);
            bridge.setLocalSummary(line);
            if (phoneOn) {
                bridge.setPhoneStats(line);
                phoneWasOn = true;
            } else if (phoneWasOn) {
                bridge.setPhoneStats(null);
                phoneWasOn = false;
            }
            localWasOn = true;
            lastTickMs = nowMs;
            lastUpBytes = upBytes;
            lastDownBytes = downBytes;
        } else if (localWasOn) {
            bridge.setLocalSummary(null);
            if (phoneWasOn) {
                bridge.setPhoneStats(null);
                phoneWasOn = false;
            }
            localWasOn = false;
            lastTickMs = null;
            lastUpBytes = null;
            lastDownBytes = null;
        }

        boolean usageOn = flags.enabled("usage_statistics");
        if (usageOn) {
            if (!usageWasOn) {
                long hourStart = nowMs - (nowMs % 3_600_000L);
                String stored = sink.loadUsageHour(hourStart);
                if (stored != null && !stored.isEmpty()) {
                    bridge.usageSeed(stored);
                } else {
                    bridge.usageReset();
                }
                usageWasOn = true;
                lastFlushMs = null;
            }
            String completed = bridge.usageStep(
                    nowMs, upBytes, downBytes, connected, network);
            if (completed != null && !completed.isEmpty()) {
                sink.upsert(completed);
            }
            if (lastFlushMs == null || nowMs - lastFlushMs >= FLUSH_MS) {
                String current = bridge.usageCurrent();
                if (current != null && !current.isEmpty()) {
                    sink.upsert(current);
                }
                lastFlushMs = nowMs;
            }
        } else if (usageWasOn) {
            usageWasOn = false;
            lastFlushMs = null;
        }
    }

    /** Every stop path: flush the current hour and clear the phone line. */
    public void onStop(long nowMs) {
        if (phoneWasOn) {
            bridge.setPhoneStats(null);
            phoneWasOn = false;
        }
        if (localWasOn) {
            bridge.setLocalSummary(null);
            localWasOn = false;
        }
        if (budgetWasOn) {
            flushBudget(nowMs, true);
        }
        if (usageWasOn) {
            String current = bridge.usageCurrent();
            if (current != null && !current.isEmpty()) {
                sink.upsert(current);
            }
        }
        budgetWasOn = false;
        usageWasOn = false;
        lastTickMs = null;
        lastUpBytes = null;
        lastDownBytes = null;
        lastBudgetUp = null;
        lastBudgetDown = null;
        lastBudgetFlushMs = null;
        lastFlushMs = null;
    }

    private void onBudgetTick(long nowMs, long upBytes, long downBytes,
            String network) {
        if (!budgetLoaded) {
            loadBudget();
        }
        budgetWasOn = true;
        int[] date = dateSource.read(nowMs);
        if (date != null && date.length >= 2) {
            budget.prepare(date[0], date[1]);
        }
        if (lastBudgetUp == null || upBytes < lastBudgetUp
                || downBytes < lastBudgetDown) {
            lastBudgetUp = upBytes;
            lastBudgetDown = downBytes;
            return;
        }
        if ("cellular".equals(network)) {
            long delta = upBytes - lastBudgetUp + (downBytes - lastBudgetDown);
            if (delta > 0 && budget.usedBytes <= Long.MAX_VALUE - delta) {
                budget.usedBytes += delta;
            } else if (delta > 0) {
                budget.usedBytes = Long.MAX_VALUE;
            }
        }
        lastBudgetUp = upBytes;
        lastBudgetDown = downBytes;

        String record = budget.line(bridge);
        String warning = bridge.budgetWarning(record);
        if (warning != null && !warning.isEmpty()
                && budget.warnedMonth != budget.monthKey) {
            budget.warnedMonth = budget.monthKey;
            warnings.warn(warning);
            // Persist the once-per-month mark now, so a restart cannot warn
            // twice for the same month.
            flushBudget(nowMs, true);
        }
        if (lastBudgetFlushMs == null
                || nowMs - lastBudgetFlushMs >= FLUSH_MS) {
            flushBudget(nowMs, false);
        }
    }

    private void loadBudget() {
        budgetLoaded = true;
        if (budgetStore == null) {
            return;
        }
        String line = budgetStore.loadBudget();
        Budget loaded = Budget.parse(line);
        if (loaded != null) {
            budget = loaded;
        }
    }

    private void flushBudget(long nowMs, boolean force) {
        if (budgetStore == null) {
            return;
        }
        if (!budgetLoaded) {
            loadBudget();
        }
        if (lastBudgetFlushMs != null && nowMs - lastBudgetFlushMs < FLUSH_MS
                && !force) {
            return;
        }
        // Adopt a new allowance or reset day the settings screen wrote since
        // the last flush. The counter and the warning stay ours.
        Budget persisted = Budget.parse(budgetStore.loadBudget());
        if (persisted != null) {
            budget.allowBytes = persisted.allowBytes;
            budget.resetDay = persisted.resetDay;
        }
        budgetStore.saveBudget(budget.line(bridge));
        lastBudgetFlushMs = nowMs;
    }

    private void readBatteryIfDue(long nowMs) {
        if (lastBatteryMs != null && nowMs - lastBatteryMs < BATTERY_MS) {
            return;
        }
        int[] value = battery.read();
        if (value != null && value.length >= 2) {
            batteryPct = value[0];
            tempC10 = value[1];
        }
        lastBatteryMs = nowMs;
    }

    /** The device-local date for a timestamp; the caller supplies the time. */
    public static int[] systemDate(long nowMs) {
        java.util.Calendar calendar = java.util.Calendar.getInstance();
        calendar.setTimeInMillis(nowMs);
        return new int[] {
                calendar.get(java.util.Calendar.YEAR),
                calendar.get(java.util.Calendar.MONTH) + 1,
                calendar.get(java.util.Calendar.DAY_OF_MONTH),
        };
    }

    /** The budget values; the line itself is built by the core through JNI. */
    private static final class Budget {
        long allowBytes;
        long usedBytes;
        int resetDay;
        int monthKey;
        int warnedMonth;

        void prepare(int year, int month) {
            int key = year * 12 + month;
            if (monthKey != key) {
                usedBytes = 0;
                warnedMonth = 0;
                monthKey = key;
            }
            int last = daysInMonth(year, month);
            if (resetDay < 1) {
                resetDay = 1;
            } else if (resetDay > last) {
                resetDay = last;
            }
        }

        String line(Bridge bridge) {
            return bridge.budgetRecord(
                    allowBytes, usedBytes, resetDay, monthKey, warnedMonth);
        }

        static Budget parse(String line) {
            if (line == null || line.isEmpty()) {
                return null;
            }
            String[] parts = line.trim().split(" ");
            if (parts.length < 2 || !"JFB1".equals(parts[0])
                    || !"1".equals(parts[1])) {
                return null;
            }
            Budget budget = new Budget();
            boolean allow = false;
            boolean used = false;
            boolean reset = false;
            boolean month = false;
            for (int i = 2; i < parts.length; i++) {
                String[] pair = parts[i].split("=", 2);
                if (pair.length != 2) {
                    return null;
                }
                try {
                    if ("allow".equals(pair[0])) {
                        budget.allowBytes = Long.parseLong(pair[1]);
                        allow = true;
                    } else if ("used".equals(pair[0])) {
                        budget.usedBytes = Long.parseLong(pair[1]);
                        used = true;
                    } else if ("reset".equals(pair[0])) {
                        budget.resetDay = Integer.parseInt(pair[1]);
                        reset = true;
                    } else if ("month".equals(pair[0])) {
                        budget.monthKey = Integer.parseInt(pair[1]);
                        month = true;
                    } else if ("warned".equals(pair[0])) {
                        budget.warnedMonth = Integer.parseInt(pair[1]);
                    }
                } catch (NumberFormatException e) {
                    return null;
                }
            }
            if (!allow || !used || !reset || !month) {
                return null;
            }
            return budget;
        }

        private static int daysInMonth(int year, int month) {
            switch (month) {
                case 1:
                case 3:
                case 5:
                case 7:
                case 8:
                case 10:
                case 12:
                    return 31;
                case 4:
                case 6:
                case 9:
                case 11:
                    return 30;
                case 2:
                    boolean leap = year % 4 == 0
                            && (year % 100 != 0 || year % 400 == 0);
                    return leap ? 29 : 28;
                default:
                    return 30;
            }
        }
    }
}
