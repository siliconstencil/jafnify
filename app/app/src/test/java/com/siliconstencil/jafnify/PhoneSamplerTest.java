package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** The sampler cadence and the OFF guarantees, with no Android and no JNI. */
public class PhoneSamplerTest {
    private static final class FakeBridge implements PhoneSampler.Bridge {
        final List<String> lines = new ArrayList<>();
        final List<String> set = new ArrayList<>();
        final List<Boolean> dnsFlags = new ArrayList<>();
        final List<long[]> steps = new ArrayList<>();
        int current;
        int resets;
        int switches;
        int signal;
        int dbm = Integer.MIN_VALUE;
        int thr = Integer.MIN_VALUE;
        int hr30 = Integer.MIN_VALUE;
        final List<String> seeds = new ArrayList<>();
        String warning = "";

        @Override
        public String summaryLine(long upBps, long downBps, String network, int signal,
                int dbm, int batteryPct, int tempC10, long usedBytes, long allowBytes,
                int resetDay, int monthKey, int switches, boolean withDns, int thermalStatus,
                int thermalHeadroom) {
            this.switches = switches;
            this.signal = signal;
            this.dbm = dbm;
            this.thr = thermalStatus;
            this.hr30 = thermalHeadroom;
            dnsFlags.add(withDns);
            return "summary " + upBps + " " + downBps + " " + network
                    + " " + batteryPct + " " + tempC10 + " " + usedBytes
                    + " " + allowBytes + " " + resetDay + " " + monthKey
                    + " " + withDns;
        }

        @Override
        public String budgetRecord(long allowBytes, long usedBytes, int resetDay,
                int monthKey, int warnedMonth) {
            return "JFB1 1 allow=" + allowBytes + " used=" + usedBytes
                    + " reset=" + resetDay + " month=" + monthKey
                    + " warned=" + warnedMonth;
        }

        @Override
        public String budgetWarning(String record) {
            return warning;
        }

        @Override
        public void setPhoneStats(String line) {
            set.add(line);
        }

        final List<String> local = new ArrayList<>();

        @Override
        public void setLocalSummary(String line) {
            local.add(line);
        }

        @Override
        public String usageStep(long atMs, long upBytes, long downBytes,
                boolean connected, String network) {
            steps.add(new long[] {atMs, upBytes, downBytes, connected ? 1 : 0});
            return "";
        }

        @Override
        public String usageCurrent() {
            current++;
            return "JFU1 1 hour=0";
        }

        @Override
        public void usageReset() {
            resets++;
        }

        @Override
        public void usageSeed(String line) {
            seeds.add(line);
        }
    }

    private static final class FakeFlags implements PhoneSampler.FlagSource {
        final Map<String, Boolean> values = new HashMap<>();

        @Override
        public boolean enabled(String id) {
            Boolean value = values.get(id);
            return value != null && value;
        }
    }

    private static final class FakeBattery implements PhoneSampler.BatterySource {
        int reads;

        @Override
        public int[] read() {
            reads++;
            return new int[] {80, 315};
        }
    }

    private static final class FakeSink implements PhoneSampler.UsageSink {
        final List<String> lines = new ArrayList<>();
        String storedHour = "";

        @Override
        public void upsert(String line) {
            lines.add(line);
        }

        @Override
        public String loadUsageHour(long hourStartMs) {
            return storedHour;
        }
    }

    private static final class FakeBudgetStore implements PhoneSampler.BudgetStore {
        String line;
        int loads;
        int saves;

        @Override
        public String loadBudget() {
            loads++;
            return line;
        }

        @Override
        public void saveBudget(String value) {
            saves++;
            line = value;
        }
    }

    private static final class FakeWarnings implements PhoneSampler.WarningSink {
        final List<String> lines = new ArrayList<>();

        @Override
        public void warn(String line) {
            lines.add(line);
        }
    }

    private static final class Fixture {
        final FakeBridge bridge = new FakeBridge();
        final FakeFlags flags = new FakeFlags();
        final FakeBattery battery = new FakeBattery();
        final FakeSink sink = new FakeSink();
        final FakeBudgetStore budgetStore = new FakeBudgetStore();
        final FakeWarnings warnings = new FakeWarnings();
        final int[] date = {2026, 11, 1};
        final PhoneSampler sampler;

        Fixture() {
            sampler = new PhoneSampler(bridge, flags, battery, sink,
                    budgetStore, now -> date, warnings);
        }

        void on() {
            flags.values.put("phone_stats", true);
        }

        void off() {
            flags.values.put("phone_stats", false);
        }

        void usageOn() {
            flags.values.put("usage_statistics", true);
        }

        void carrierOn() {
            flags.values.put("carrier_quality", true);
        }

        void budgetOn() {
            flags.values.put("data_budget", true);
        }

        void batteryOn() {
            flags.values.put("battery_status", true);
        }
    }

    @Test
    public void transport_words_map_to_core_network_words() {
        assertEquals("wifi", PhoneSampler.networkWord("wifi"));
        assertEquals("cellular", PhoneSampler.networkWord("cellular"));
        assertEquals("eth", PhoneSampler.networkWord("ethernet"));
        assertEquals("none", PhoneSampler.networkWord("none"));
        assertEquals("other", PhoneSampler.networkWord("vpn"));
        assertEquals("other", PhoneSampler.networkWord("bluetooth"));
    }

    @Test
    public void the_signal_strength_travels_to_the_summary_line() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setDbm(-95);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("the dBm of the sample reaches the summary", -95, f.bridge.dbm);
    }

    @Test
    public void an_unknown_signal_strength_stays_unknown() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setDbm(-1);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("a gated dBm never reaches the summary",
                Integer.MIN_VALUE, f.bridge.dbm);
    }

    // The modem's unknown value (CellInfo.UNAVAILABLE) is Integer.MAX_VALUE.
    // It must never travel as a number: the JNI's own unknown marker
    // (Integer.MIN_VALUE) is what goes into the summary line.
    @Test
    public void the_modem_unknown_value_never_travels_as_a_number() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setDbm(Integer.MAX_VALUE);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("the unavailable value becomes the JNI's unknown marker",
                Integer.MIN_VALUE, f.bridge.dbm);
    }

    @Test
    public void the_thermal_status_travels_to_the_summary_line() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setThermalStatus(3);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("the thermal status reaches the summary", 3, f.bridge.thr);
    }

    @Test
    public void an_unknown_thermal_status_stays_unknown() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setThermalStatus(-1);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("a gated thermal status never reaches the summary",
                Integer.MIN_VALUE, f.bridge.thr);
    }

    @Test
    public void the_battery_switch_carries_the_heat_too() {
        Fixture f = new Fixture();
        f.batteryOn();
        f.sampler.setThermalStatus(2);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("the heat travels under the same switches as the temperature",
                2, f.bridge.thr);
    }

    @Test
    public void the_thermal_headroom_travels_to_the_summary_line() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setThermalHeadroom(105);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("the headroom forecast reaches the summary", 105, f.bridge.hr30);
    }

    @Test
    public void an_unknown_thermal_headroom_stays_unknown() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setThermalHeadroom(-1);
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals("a gated headroom forecast never reaches the summary",
                Integer.MIN_VALUE, f.bridge.hr30);
    }

    @Test
    public void a_restart_mid_hour_seeds_the_usage_bucket() {
        Fixture f = new Fixture();
        f.sink.storedHour = "JFU1 1 hour=0 up=1000 down=2000 ups=0 mins=0 samples=20";
        f.usageOn();
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals(1, f.bridge.seeds.size());
        assertEquals(f.sink.storedHour, f.bridge.seeds.get(0));
        assertEquals(0, f.bridge.resets);
    }

    @Test
    public void a_fresh_hour_keeps_the_usage_reset() {
        Fixture f = new Fixture();
        f.usageOn();
        f.sampler.onTick(1_800_000, 0, 0, true, "wifi");
        assertEquals(0, f.bridge.seeds.size());
        assertEquals(1, f.bridge.resets);
    }

    @Test
    public void the_switch_count_reaches_the_summary_line() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.setSwitches(4);
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(4, f.bridge.switches);
    }

    @Test
    public void carrier_quality_off_does_not_ask_for_dns() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(1, f.bridge.dnsFlags.size());
        assertEquals(Boolean.FALSE, f.bridge.dnsFlags.get(0));
    }

    @Test
    public void carrier_quality_on_asks_for_dns_only_while_phone_stats_is_on() {
        Fixture f = new Fixture();
        f.on();
        f.carrierOn();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(1, f.bridge.dnsFlags.size());
        assertEquals(Boolean.TRUE, f.bridge.dnsFlags.get(0));

        f.sampler.onTick(2_000, 0, 0, true, "wifi");
        assertEquals(2, f.bridge.dnsFlags.size());
        assertEquals(Boolean.TRUE, f.bridge.dnsFlags.get(1));

        f.off();
        f.sampler.onTick(3_000, 0, 0, true, "wifi");
        // DNS is still on: the main screen keeps its line, the laptop gets none.
        assertEquals(3, f.bridge.dnsFlags.size());
        assertNull("phone_stats OFF clears the laptop line",
                f.bridge.set.get(f.bridge.set.size() - 1));

        f.flags.values.put("carrier_quality", false);
        f.sampler.onTick(4_000, 0, 0, true, "wifi");
        assertEquals("every data switch OFF stops the summary", 3, f.bridge.dnsFlags.size());
    }

    @Test
    public void off_makes_no_battery_read_and_no_native_call() {
        Fixture f = new Fixture();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(0, f.battery.reads);
        assertEquals(0, f.bridge.lines.size());
        assertEquals(0, f.bridge.set.size());
        assertEquals(0, f.bridge.steps.size());
        assertEquals(0, f.sink.lines.size());
    }

    @Test
    public void on_sends_one_line_per_tick_and_starts_from_a_baseline() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(1_000, 500, 700, true, "wifi");
        assertEquals(1, f.bridge.set.size());
        assertEquals("summary 0 0 wifi 80 315 0 0 0 0 false", f.bridge.set.get(0));
        assertEquals(1, f.battery.reads);

        f.sampler.onTick(2_000, 1_500, 2_700, true, "wifi");
        assertEquals(2, f.bridge.set.size());
        assertEquals("summary 1000 2000 wifi 80 315 0 0 0 0 false", f.bridge.set.get(1));
        assertEquals("battery is cached inside the minute", 1, f.battery.reads);
    }

    @Test
    public void battery_is_read_at_most_once_per_minute() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(0, 0, 0, true, "wifi");
        f.sampler.onTick(59_000, 1, 1, true, "wifi");
        assertEquals(1, f.battery.reads);
        f.sampler.onTick(60_000, 2, 2, true, "wifi");
        assertEquals(2, f.battery.reads);
    }

    @Test
    public void off_after_on_clears_the_provider_once() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        f.off();
        f.sampler.onTick(2_000, 0, 0, true, "wifi");
        assertEquals(2, f.bridge.set.size());
        assertNull(f.bridge.set.get(1));
        f.sampler.onTick(3_000, 0, 0, true, "wifi");
        assertEquals("steady OFF does not clear again", 2, f.bridge.set.size());
    }

    @Test
    public void a_counter_reset_starts_a_new_baseline_without_a_huge_rate() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(0, 10_000, 20_000, true, "wifi");
        f.sampler.onTick(1_000, 11_000, 22_000, true, "wifi");
        f.sampler.onTick(2_000, 50, 60, true, "wifi");
        assertEquals("summary 0 0 wifi 80 315 0 0 0 0 false", f.bridge.set.get(2));
        f.sampler.onTick(3_000, 1_050, 1_060, true, "wifi");
        assertEquals("summary 1000 1000 wifi 80 315 0 0 0 0 false", f.bridge.set.get(3));
    }

    @Test
    public void usage_off_never_steps_or_writes() {
        Fixture f = new Fixture();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(0, f.bridge.steps.size());
        assertEquals(0, f.bridge.current);
        assertEquals(0, f.bridge.resets);
        assertEquals(0, f.sink.lines.size());
    }

    @Test
    public void usage_on_steps_every_tick_and_flushes_every_minute() {
        Fixture f = new Fixture();
        f.usageOn();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(1, f.bridge.resets);
        assertEquals(1, f.bridge.steps.size());
        assertEquals("the first ON tick flushes", 1, f.bridge.current);
        assertEquals(1, f.sink.lines.size());

        f.sampler.onTick(2_000, 1, 1, true, "wifi");
        assertEquals(2, f.bridge.steps.size());
        assertEquals(1, f.bridge.current);

        f.sampler.onTick(61_000, 2, 2, true, "wifi");
        assertEquals(3, f.bridge.steps.size());
        assertEquals(2, f.bridge.current);
        assertEquals(2, f.sink.lines.size());
    }

    @Test
    public void budget_off_never_loads_or_saves_the_store() {
        Fixture f = new Fixture();
        f.on();
        f.sampler.onTick(1_000, 0, 0, true, "cellular");
        assertEquals(0, f.budgetStore.loads);
        assertEquals(0, f.budgetStore.saves);
    }

    @Test
    public void budget_counts_only_the_cellular_uplink() {
        Fixture f = new Fixture();
        f.on();
        f.budgetOn();
        f.sampler.onTick(1_000, 1_000, 2_000, true, "cellular");
        f.sampler.onTick(2_000, 2_000, 4_000, true, "cellular");
        assertEquals("summary 1000 2000 cellular 80 315 3000 0 1 24323 false",
                f.bridge.set.get(1));
        f.sampler.onTick(3_000, 5_000, 9_000, true, "wifi");
        f.sampler.onTick(4_000, 6_000, 11_000, true, "cellular");
        assertEquals("Wi-Fi bytes are not budgeted",
                "summary 1000 2000 cellular 80 315 6000 0 1 24323 false",
                f.bridge.set.get(3));
    }

    @Test
    public void a_new_month_resets_the_budget_counter() {
        Fixture f = new Fixture();
        f.on();
        f.budgetOn();
        f.sampler.onTick(1_000, 1_000, 2_000, true, "cellular");
        f.sampler.onTick(2_000, 2_000, 4_000, true, "cellular");
        f.date[1] = 12;
        f.sampler.onTick(3_000, 2_000, 4_000, true, "cellular");
        assertEquals("summary 0 0 cellular 80 315 0 0 1 24324 false",
                f.bridge.set.get(2));
    }

    @Test
    public void the_budget_warning_is_sent_once_and_saved() {
        Fixture f = new Fixture();
        f.on();
        f.budgetOn();
        f.budgetStore.line = "JFB1 1 allow=10000 used=0 reset=1 month=24323 warned=0";
        f.bridge.warning = "80% of your mobile data allowance used.";
        f.sampler.onTick(1_000, 1_000, 2_000, true, "cellular");
        f.sampler.onTick(2_000, 2_000, 4_000, true, "cellular");
        assertEquals(1, f.warnings.lines.size());
        assertTrue(f.budgetStore.saves >= 1);
        f.sampler.onTick(3_000, 3_000, 6_000, true, "cellular");
        assertEquals("the warned month blocks a second warning", 1,
                f.warnings.lines.size());
    }

    @Test
    public void the_budget_is_flushed_every_minute_and_on_stop() {
        Fixture f = new Fixture();
        f.budgetOn();
        f.sampler.onTick(1_000, 0, 0, true, "cellular");
        assertEquals("the first budget tick is a baseline", 0,
                f.budgetStore.saves);
        f.sampler.onTick(2_000, 1, 1, true, "cellular");
        assertEquals(1, f.budgetStore.saves);
        f.sampler.onTick(62_000, 2, 2, true, "cellular");
        assertEquals(2, f.budgetStore.saves);
        f.sampler.onStop(63_000);
        assertEquals(3, f.budgetStore.saves);
    }

    @Test
    public void battery_status_can_read_the_battery_without_phone_stats() {
        Fixture f = new Fixture();
        f.batteryOn();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        assertEquals(1, f.battery.reads);
        assertEquals(0, f.bridge.set.size());
    }

    @Test
    public void stop_flushes_current_and_clears_the_phone_line() {
        Fixture f = new Fixture();
        f.on();
        f.usageOn();
        f.sampler.onTick(1_000, 0, 0, true, "wifi");
        f.sampler.onStop(2_000);
        assertEquals("the ON tick line and the clear", 2, f.bridge.set.size());
        assertNull(f.bridge.set.get(1));
        assertTrue(f.bridge.current >= 2);
        assertTrue(f.sink.lines.size() >= 2);
    }

    @Test
    public void the_main_screen_line_does_not_need_the_laptop_switch() {
        Fixture f = new Fixture();
        f.batteryOn();
        f.sampler.onTick(1_000, 0, 0, true, "cellular");
        // The phone's own screen gets the line; the laptop gets nothing.
        assertEquals(1, f.bridge.local.size());
        assertTrue(f.bridge.local.get(0) != null);
        assertTrue(f.bridge.set.isEmpty());

        f.on();
        f.sampler.onTick(2_000, 0, 0, true, "cellular");
        assertEquals(1, f.bridge.set.size());
        assertTrue(f.bridge.set.get(0) != null);

        f.off();
        f.flags.values.put("battery_status", false);
        f.sampler.onTick(3_000, 0, 0, true, "cellular");
        // Both switched off: the laptop line and the screen line are cleared.
        assertEquals(null, f.bridge.set.get(f.bridge.set.size() - 1));
        assertEquals(null, f.bridge.local.get(f.bridge.local.size() - 1));
    }
}
