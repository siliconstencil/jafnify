//! The windowless tray daemon: owns the tunnel, discovery, reconnect, kill
//! switch, tray icon and log. The window talks to it over `control`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jafnify_core::best_spot::Sample;
use jafnify_core::good_hours::Sample as HourSample;
use jafnify_core::notification::{connection_transition, Notification, NotificationPolicy};
use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::state::Role;
use jafnify_core::ConnectionState;

use crate::app::{App, AppBuilder, UiEvent, FRESH_PHONE_SECS};
use crate::applog;
use crate::children::Children;
use crate::control::{
    bind_socket, lock_path, socket_path, InstanceLock, Request, Response, Snapshot, WindowPresence,
    WINDOW_PRESENCE_TIMEOUT,
};
use crate::export;
use crate::firewall::FirewallKind;
use crate::theme::ThemeMode;
use crate::tray::{tray_should_be_visible, TrayCommand, TrayManager, TrayVisual};
use crate::ui_text::{key_prefix, retry_reason};
use crate::APP_DIR;

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

/// How many drops the daemon sends to the window and the CLI. The window
/// shows the newest five of them.
pub const DROP_ROWS: usize = 20;

/// The settings a window asks to change. Every field is what the request
/// carried, so the window and the daemon cannot drift apart.
struct SettingsPatch<'a> {
    role: &'a str,
    autostart: bool,
    kill_switch: bool,
    tray_only_when_closed: bool,
    theme: Option<&'a str>,
    flags: &'a BTreeMap<String, bool>,
    nickname: Option<&'a str>,
}

struct Runtime {
    app: App,
    config_dir: PathBuf,
    flags: crate::flags::FlagStore,
    /// The local device name (`device-name.toml`); never sent to the peer.
    nickname: String,
    /// The phone summary cache and its gate bits; the worker owns the socket.
    phone_stats: std::sync::Arc<crate::phone_stats::PhoneStatsShared>,
    /// True while a link test runs on its worker thread; one at a time.
    link_test_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tray: TrayManager,
    presence: WindowPresence,
    children: Children,
    last_state: ConnectionState,
    /// The desktop notifier; none until a line is really sent.
    notifier: Option<Box<dyn crate::notification::Notifier>>,
    /// A test keeps its own notifier even while the flags are off.
    notifier_pinned: bool,
    /// The rate model and the last line that went out.
    notifications: NotificationPolicy,
    /// The last phone network kind the laptop saw.
    last_phone_net: Option<NetworkKind>,
    /// The heat state of the phone, with the hysteresis.
    heat: jafnify_core::phone_heat::HeatTracker,
    /// The phone summary generation whose exit reason was already handled.
    exit_generation: u64,
    /// The phone summary generation whose connected time was already adopted.
    conn_sync_generation: u64,
    /// The good-time episodes of the link.
    good_time: jafnify_core::good_time::GoodTimeTracker,
    /// The read-only host probe; its worker runs the optional tools.
    probe: crate::host_probe::HostProbe,
    /// The phone summary the best-spot session already has as a sample.
    best_spot_generation: u64,
    /// The phone summary the good-hours learning already has as a sample.
    good_hours_generation: u64,
    /// The trend rings of the tiles. They live in memory only.
    trends: jafnify_core::tiles::Trends,
    /// The phone summary generation the trends were last fed with.
    trend_generation: u64,
    no_peer: bool,
    event_error: Option<String>,
    /// The page a tray entry asked the window to open; taken and cleared by
    /// the window on its next poll.
    pending_page: Option<String>,
    quit: bool,
}

/// The notifier of the desktop. A unit test never reaches the real session
/// bus: a test that forgot its fake would otherwise put a line on the
/// desktop of whoever runs the suite.
#[cfg(not(test))]
fn desktop_notifier() -> Box<dyn crate::notification::Notifier> {
    Box::new(crate::notification::DesktopNotifier::start())
}

#[cfg(test)]
fn desktop_notifier() -> Box<dyn crate::notification::Notifier> {
    Box::new(crate::notification::DesktopNotifier::silent())
}

/// The notifications whose popup is withdrawn when the phone cools down.
fn is_heat(notification: Notification) -> bool {
    matches!(
        notification,
        Notification::PhoneHot { .. } | Notification::PhoneThrottling
    )
}

impl Runtime {
    fn new(app: App, config_dir: PathBuf) -> Self {
        let mut tray = TrayManager::new();
        tray.set_visible(true);
        let flags = crate::flags::FlagStore::load(&config_dir.join(crate::flags::FILE_NAME));
        let nickname = crate::local_name::load(&config_dir.join(crate::local_name::FILE_NAME));
        Self {
            last_state: app.state(),
            app,
            config_dir,
            flags,
            nickname,
            phone_stats: std::sync::Arc::new(crate::phone_stats::PhoneStatsShared::new()),
            link_test_busy: Default::default(),
            tray,
            presence: WindowPresence::new(WINDOW_PRESENCE_TIMEOUT),
            children: Children::default(),
            notifier: None,
            notifier_pinned: false,
            notifications: NotificationPolicy::new(),
            last_phone_net: None,
            heat: Default::default(),
            exit_generation: 0,
            conn_sync_generation: 0,
            good_time: Default::default(),
            probe: crate::host_probe::HostProbe::new(std::sync::Arc::new(
                crate::host_probe::ProcessRunner,
            )),
            best_spot_generation: 0,
            good_hours_generation: 0,
            trends: Default::default(),
            trend_generation: 0,
            no_peer: false,
            event_error: None,
            pending_page: None,
            quit: false,
        }
    }

    fn start_known_if_possible(&mut self) {
        if jafnify_core::terms::is_accepted(&self.config_dir) && self.app.is_paired() {
            if let Err(e) = self.app.start_known() {
                self.event_error = Some(e.to_string());
            }
        }
    }

    fn tick(&mut self) {
        self.children.reap();
        let window_open = self.presence.is_open(Instant::now());
        self.app.set_viewer_open(window_open);
        self.app
            .set_usage_enabled(self.flags.get("usage_statistics"));
        self.app
            .set_drop_history_enabled(self.flags.get("drop_history"));
        self.app.set_phone_context(self.phone_context());
        match self.app.tick() {
            Ok(Some(UiEvent::NoPeerFound)) => self.no_peer = true,
            Ok(Some(UiEvent::Connected)) => {
                self.no_peer = false;
                self.event_error = None;
            }
            Ok(Some(UiEvent::RootError(e))) => self.event_error = Some(e),
            Ok(Some(UiEvent::PermissionDenied)) => {
                self.event_error = Some("Permission was denied - try again".to_string())
            }
            Ok(_) => {}
            Err(e) => self.event_error = Some(e.to_string()),
        }
        let state = self.app.state();
        let previous_state = self.last_state;
        if state != previous_state {
            if state != ConnectionState::Searching {
                self.no_peer = false;
            }
            self.last_state = state;
        }
        let now = Instant::now();
        self.notify_connection(previous_state, state, now);
        // The phone summary feeds the phone block, the heat warning and the
        // good-time notice, so any of the three asks for it. The best-spot
        // session and the good-hours learning need the same summary: no new
        // request, no new traffic.
        // While this laptop joins a phone, the summary is also the clock
        // carrier: the phone's own connected time rides on it, so it runs even
        // with every phone tile switched off. A Linux sharer keeps the old
        // gating; its peer is not this clock's authority.
        let joiner_connected = state == ConnectionState::Connected
            && self.app.role() == jafnify_core::state::Role::Joiner;
        let phone_on = self.flags.get("phone_stats")
            || self.flags.get("phone_heat_warning")
            || self.flags.get("good_time_notice")
            || self.flags.get("best_spot")
            || self.flags.get("good_hours")
            || self.flags.get("wifi_range")
            || joiner_connected;
        self.phone_stats
            .set(phone_on, state == ConnectionState::Connected, window_open);
        // The phone shows how far the laptop is: the laptop's link report
        // rides on the summary request while the range switch is on.
        self.phone_stats.set_range(if self.flags.get("wifi_range") {
            self.app.range_report(now)
        } else {
            None
        });
        let (phone_stats, phone_age_secs, _) = self.phone_stats.snapshot(now);
        // Adopt the phone's connected time once per summary: both sides then
        // count from the phone's own first authenticated exchange.
        let (sync_stats, sync_at, sync_generation) = self.phone_stats.summary_for_clock();
        if sync_generation != self.conn_sync_generation {
            self.conn_sync_generation = sync_generation;
            if let (Some(stats), Some(at)) = (sync_stats.as_ref(), sync_at) {
                self.app.observe_phone_connected_ms(stats.sconn_ms, at);
            }
        }
        let phone_net = phone_stats.as_ref().map(|stats| stats.net);
        self.notify_phone_network(state, phone_net, now);
        self.update_phone_exit(state, phone_age_secs, now);
        self.update_phone_heat(state, phone_stats.as_ref(), phone_age_secs, now);
        self.update_good_time(state, phone_stats.as_ref(), phone_age_secs, now);
        self.update_best_spot(window_open);
        self.update_good_hours(state, phone_age_secs);
        self.update_trends(state, phone_age_secs);
        if window_open && self.app.details_expanded() {
            self.probe
                .request(self.app.uplink_iface().map(str::to_string));
        } else {
            self.probe.clear();
        }
        self.release_notifier();
    }

    /// The phone state of this moment, for the context of a drop: the last
    /// summary and the Wi-Fi leg. The daemon owns the phone measurements, so
    /// it hands them to the app before every tick.
    fn phone_context(&self) -> Option<crate::app::PhoneContextSample> {
        let (stats, age_secs, _) = self.phone_stats.snapshot(Instant::now());
        let stats = stats?;
        let age_secs = age_secs?;
        Some(crate::app::PhoneContextSample {
            context: jafnify_core::drop_history::PhoneContext {
                net: Some(stats.net),
                signal: stats.signal,
                temp_c_x10: stats.temp_c_x10,
                battery: stats.battery,
                rtt_ms: stats.rtt_ms,
                thr: stats.thr,
            },
            age_secs,
            at: Instant::now(),
            wifi: self.phone_stats.wifi_leg(),
            wg_loss_pct: stats.wg_loss_pct,
            wg_rtt_ms: stats.wg_rtt_ms,
        })
    }

    /// The desktop notifier, created only when a line is really sent.
    fn send_notification(&mut self, notification: Notification, now: Instant) -> Option<String> {
        self.send_notification_with(notification, now, None)
    }

    /// The line for a transition, with the reading behind it when the line
    /// speaks about one. Every line that goes out is written to the
    /// connection log, so a popup can always be traced to its numbers.
    fn send_notification_with(
        &mut self,
        notification: Notification,
        now: Instant,
        reading: Option<String>,
    ) -> Option<String> {
        let line = self.notifications.decide(notification, now)?;
        let body = notification.body().unwrap_or("");
        if self.notifier.is_none() {
            self.notifier = Some(desktop_notifier());
        }
        if let Some(notifier) = self.notifier.as_mut() {
            if is_heat(notification) {
                notifier.notify_heat(&line, body);
            } else if matches!(notification, Notification::Lost) {
                // Keep its id: an app update reclassifies the outage and the
                // popup must not stay claiming a lost connection.
                notifier.notify_lost(&line, body);
            } else {
                notifier.notify(&line, body);
            }
        }
        let mut text = format!("notified: {line}");
        if let Some(reading) = reading {
            text.push_str(", ");
            text.push_str(&reading);
        }
        self.app.log_notification(&text, now);
        Some(line)
    }

    /// The line for a connection change, while the flag is on.
    fn notify_connection(
        &mut self,
        previous: ConnectionState,
        current: ConnectionState,
        now: Instant,
    ) -> Option<String> {
        if !self.flags.get("notify_connection_change") {
            return None;
        }
        // The phone cannot be reached: the laptop waits for it and comes back
        // on its own. An alarm about that is noise, so the line is dropped.
        if self.app.phone_unreachable() && current != ConnectionState::Connected {
            return None;
        }
        // The return of an absence is quiet: the samplers resume silently.
        if current == ConnectionState::Connected && self.app.take_away_quiet() {
            return None;
        }
        let notification = connection_transition(previous, current)?;
        self.send_notification(notification, now)
    }

    /// The line for a phone network change, while the flag is on. The tunnel
    /// has to be up: a dropped tunnel sends only the connection line.
    fn notify_phone_network(
        &mut self,
        state: ConnectionState,
        current: Option<NetworkKind>,
        now: Instant,
    ) -> Option<String> {
        if !self.flags.get("phone_network_warning") {
            return None;
        }
        if state != ConnectionState::Connected {
            self.last_phone_net = None;
            return None;
        }
        let current = current?;
        let previous = self.last_phone_net.unwrap_or(NetworkKind::Unknown);
        self.last_phone_net = Some(current);
        let warning = jafnify_core::phone_network_warning::warning(previous, current)?;
        self.send_notification(warning.notification(), now)
    }

    /// A test keeps its own notifier.
    #[cfg(test)]
    fn set_notifier(&mut self, notifier: Box<dyn crate::notification::Notifier>) {
        self.notifier = Some(notifier);
        self.notifier_pinned = true;
    }

    /// The heat warning: one line when the phone gets hot, none while it
    /// stays hot, and again only after it cooled down. The reading has to be
    /// fresh: an old summary says nothing about the phone now. A tunnel that
    /// went down keeps the state: the phone did not cool down.
    fn update_phone_heat(
        &mut self,
        state: ConnectionState,
        stats: Option<&jafnify_core::phone_stats::PhoneStats>,
        age_secs: Option<u64>,
        now: Instant,
    ) {
        if !self.flags.get("phone_heat_warning") {
            self.heat.reset();
            return;
        }
        if state != ConnectionState::Connected {
            return;
        }
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        let temp = if fresh {
            stats.and_then(|stats| stats.temp_c_x10)
        } else {
            None
        };
        // The phone's own word about the heat. A phone that does not report
        // it is warned about by its temperature instead.
        let thr = if fresh {
            stats.and_then(|stats| stats.thr)
        } else {
            None
        };
        let Some(change) = self.heat.update(temp, thr) else {
            return;
        };
        // The reading behind a heat line, so the log can say what the popup
        // knew: one line with the thermal word and the age of the summary.
        let reading = format!(
            "thr {}, age {} s",
            thr.map(|status| status.to_string())
                .unwrap_or_else(|| "none".to_string()),
            age_secs
                .map(|age| age.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
        match change.warning {
            Some(jafnify_core::phone_heat::Warning::Throttling) => {
                self.send_notification_with(Notification::PhoneThrottling, now, Some(reading));
            }
            Some(jafnify_core::phone_heat::Warning::Hot { temp_c_x10 }) => {
                self.send_notification_with(
                    Notification::PhoneHot { temp_c_x10 },
                    now,
                    Some(reading),
                );
            }
            None => {}
        }
        // The phone cooled down or stopped limiting itself: a stale popup on
        // the desktop must not sit there claiming the phone is still hot.
        if change.cooled || change.stopped_throttling {
            if let Some(notifier) = self.notifier.as_ref() {
                notifier.withdraw_heat();
            }
        }
    }

    /// The exit reason the phone sent in the first summary of the new
    /// session: an update or a user stop makes the outage neutral, a crash
    /// keeps it a drop with the phone's own hint. One summary, one action.
    fn update_phone_exit(&mut self, state: ConnectionState, age_secs: Option<u64>, now: Instant) {
        if state != ConnectionState::Connected {
            return;
        }
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        if !fresh {
            return;
        }
        let Some((reason, generation)) = self.phone_stats.exit() else {
            return;
        };
        if generation == self.exit_generation {
            return;
        }
        self.exit_generation = generation;
        match self.app.note_phone_exit(reason, now) {
            // The outage stayed neutral while the peer was away: an app
            // update is not a lost connection. Withdraw a Lost line that a
            // long install may already have sent.
            crate::app::NoteExit::Updated => {
                if let Some(notifier) = self.notifier.as_ref() {
                    notifier.withdraw_lost();
                }
            }
            // A quick reset turned out to be a crash: the drop is only now
            // being written, so this is the moment the desktop must hear it.
            crate::app::NoteExit::FaultNow => {
                self.send_notification(Notification::Lost, now);
            }
            _ => {}
        }
    }

    /// One sample of the phone for the trend rings of the tiles. The cadence
    /// is the one the phone summary already has: one sample per summary, no
    /// new request and no new traffic. The rings are cleared when the tunnel
    /// goes down: the numbers of the old session say nothing about the new
    /// one.
    fn update_trends(&mut self, state: ConnectionState, age_secs: Option<u64>) {
        if !self.flags.get("tile_home") {
            self.trends.clear();
            return;
        }
        if state != ConnectionState::Connected {
            self.trends.clear();
            return;
        }
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        if !fresh {
            return;
        }
        let (stats, generation) = self.phone_stats.summary_with_generation();
        if generation == self.trend_generation {
            return;
        }
        self.trend_generation = generation;
        let Some(stats) = stats else {
            return;
        };
        let wifi_ms = self.phone_stats.wifi_leg().map(|leg| leg.median_ms);
        self.trends.push(self.now_ms(), &stats, wifi_ms);
    }

    /// The good-time notice: the conditions have to hold for a while, and they
    /// have to have been bad before that.
    fn update_good_time(
        &mut self,
        state: ConnectionState,
        stats: Option<&jafnify_core::phone_stats::PhoneStats>,
        age_secs: Option<u64>,
        now: Instant,
    ) {
        if !self.flags.get("good_time_notice") {
            return;
        }
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        let wifi = self.phone_stats.wifi_leg();
        let good = state == ConnectionState::Connected
            && fresh
            && stats
                .map(|stats| {
                    jafnify_core::good_time::is_good(
                        stats.net,
                        stats.signal,
                        stats.rtt_ms,
                        wifi.as_ref(),
                    )
                })
                .unwrap_or(false);
        let net = stats.map(|stats| stats.net.display()).unwrap_or("Unknown");
        let signal = stats.and_then(|stats| stats.signal).unwrap_or(0);
        if let Some(notice) = self.good_time.update(good, net, signal, now) {
            self.send_notification(
                Notification::GoodTime {
                    net: notice.net,
                    signal: notice.signal,
                },
                now,
            );
        }
    }

    /// A test keeps its own probe.
    #[cfg(test)]
    fn set_probe(&mut self, probe: crate::host_probe::HostProbe) {
        self.probe = probe;
    }

    /// One sample of the phone for the best-spot session, with the Wi-Fi leg.
    /// The cadence is the one the phone summary already has: one sample per
    /// summary, no new request and no new traffic. The session is a window
    /// activity, so the samples land while the window is open.
    fn update_best_spot(&mut self, window_open: bool) {
        if !self.flags.get("best_spot") || !window_open {
            return;
        }
        // The summary and the generation that counts it are read together: a
        // summary that arrives between two reads is sampled, not lost, and
        // the sample is never an older summary than the generation.
        let (stats, generation) = self.phone_stats.summary_with_generation();
        if generation == self.best_spot_generation {
            return;
        }
        self.best_spot_generation = generation;
        let Some(stats) = stats else {
            return;
        };
        let wifi_ms = self.phone_stats.wifi_leg().map(|leg| leg.median_ms);
        self.app.best_spot_sample(Sample {
            at_ms: self.now_ms(),
            net: stats.net,
            // 255 is the core's word for an unknown signal level.
            level: stats.signal.unwrap_or(255),
            dbm: stats.dbm,
            rtt_ms: stats.rtt_ms,
            wifi_ms,
        });
    }

    /// One sample of the phone for the good-hours learning. The cadence is
    /// the one the phone summary already has: one sample per summary, no new
    /// request and no new traffic. The samples are taken only while the
    /// switch is on, while the link is up and while the summary is fresh.
    fn update_good_hours(&mut self, state: ConnectionState, age_secs: Option<u64>) {
        self.app.set_good_hours_gate(
            self.flags.get("good_hours"),
            state == ConnectionState::Connected,
            age_secs,
        );
        // The summary and the generation that counts it are read together: a
        // summary that arrives between two reads is sampled, not lost, and
        // the sample is never an older summary than the generation.
        let (stats, generation) = self.phone_stats.summary_with_generation();
        if generation == self.good_hours_generation {
            return;
        }
        self.good_hours_generation = generation;
        let Some(stats) = stats else {
            return;
        };
        self.app.good_hours_sample(HourSample {
            at_ms: self.now_ms(),
            net: stats.net,
            signal: stats.signal,
            dbm: stats.dbm,
            dns_ms: stats.rtt_ms,
            temp_c_x10: stats.temp_c_x10,
            thr: stats.thr,
        });
    }

    /// The lines of the good-hours block, in the core wording. The block adds
    /// nothing while its switch is off.
    fn good_hours_lines(&self) -> Vec<String> {
        if !self.flags.get("good_hours") {
            return Vec::new();
        }
        let now_ms = self.now_ms();
        let offset = crate::applog::local_offset_minutes(now_ms);
        let mut lines = self.app.good_hours_lines(now_ms, offset);
        if let Some(evidence) = self.hot_hour_evidence(now_ms, offset) {
            lines.push(evidence);
        }
        crate::flags::good_hours_lines(self.flags.get("good_hours"), lines)
    }

    /// What the hot hours taught us, when the data is enough to say it. The
    /// line says nothing when the phone was never hot enough, or never
    /// measured.
    fn hot_hour_evidence(&self, now_ms: i64, offset: i32) -> Option<String> {
        if !self.flags.get("good_hours") {
            return None;
        }
        let records = self.app.good_hours_records();
        jafnify_core::good_hours::hot_hour_evidence(&records, now_ms, offset)
    }

    /// The home screen of the window: the status card, the tiles and the hour
    /// strip. The block adds nothing while its switch is off.
    fn home(&self) -> Option<jafnify_core::tiles::HomeModel> {
        if !self.flags.get("tile_home") {
            return None;
        }
        let now_ms = self.now_ms();
        let offset = crate::applog::local_offset_minutes(now_ms);
        let records = self.app.good_hours_records();
        let summary = jafnify_core::good_hours::Summary::build(&records, now_ms, offset);
        // The badge of the DNS tile needs the overall median of the good
        // hours: without the learning there is nothing to compare with.
        let baseline = if self.flags.get("good_hours") && summary.baseline_ms > 0 {
            Some(summary.baseline_ms)
        } else {
            None
        };
        let evidence = self.hot_hour_evidence(now_ms, offset);
        let tiles = self.tiles(now_ms, offset, baseline, evidence.as_deref());
        let hours = self
            .flags
            .get("good_hours")
            .then(|| jafnify_core::tiles::hours_strip(&summary));
        // The card's time is the live session's connected time, the field the
        // Details page shows. `connected_secs` is the current tunnel clock and
        // can restart on a phone restart that does not end the session.
        let session = self.app.current_session();
        let uptime_secs = if session.started_at > 0 {
            Some(session.connected_ms / 1_000)
        } else {
            self.app.connected_secs()
        };
        let status = jafnify_core::tiles::status_card(&jafnify_core::tiles::StatusInput {
            connected: self.app.display_connected(),
            name: self.app.paired_name().as_deref(),
            path: self.path_text().as_deref(),
            uptime_secs,
            notice: self.good_time_notice(self.app.state()).as_deref(),
        });
        Some(jafnify_core::tiles::home_model(status, tiles, hours))
    }

    /// The tiles of the home screen. A tile is only there when its own switch
    /// is on and it has data.
    /// The range tile: the Wi-Fi level the laptop hears from the hotspot,
    /// while the flag is on and the uplink is Wi-Fi.
    fn range_tile(&self) -> Option<jafnify_core::tiles::Tile> {
        if !self.flags.get("wifi_range") {
            return None;
        }
        let report = self.app.range_report(Instant::now())?;
        Some(jafnify_core::wifi_range::range_tile(
            &report,
            self.app.range_trend(),
        ))
    }

    fn tiles(
        &self,
        now_ms: i64,
        offset: i32,
        baseline_ms: Option<u32>,
        evidence: Option<&str>,
    ) -> Vec<jafnify_core::tiles::Tile> {
        let mut tiles = Vec::new();
        let (stats, age_secs, _) = self.phone_stats.snapshot(Instant::now());
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        // The phone tiles are about the phone now: a stale reading says
        // nothing, so the tiles wait for a fresh summary.
        if self.flags.get("phone_stats") && fresh {
            if let Some(stats) = stats.as_ref() {
                if let Some(tile) =
                    jafnify_core::tiles::dns_tile(stats.rtt_ms, self.trends.dns(), baseline_ms)
                {
                    tiles.push(tile);
                }
                if let Some(tile) =
                    jafnify_core::tiles::signal_tile(stats, self.trends.signal())
                {
                    // An older phone may have no `caps=net` but still send a
                    // real reading: never hide it behind the hint.
                    tiles.push(tile);
                } else if !stats.caps_net {
                    tiles.push(jafnify_core::tiles::laptop_signal_permission_tile());
                }
                if let Some(tile) = jafnify_core::tiles::phone_tile(
                    stats.temp_c_x10,
                    stats.thr,
                    stats.hr30,
                    stats.battery,
                    self.trends.phone(),
                    evidence,
                ) {
                    tiles.push(tile);
                }
            }
            let phone_loss = stats.as_ref().and_then(|stats| stats.wg_loss_pct);
            if let Some(leg) = self.phone_stats.wifi_leg() {
                if let Some(tile) =
                    jafnify_core::tiles::wifi_tile(&leg, self.trends.wifi(), phone_loss)
                {
                    tiles.push(tile);
                }
            }
        }
        if let Some(tile) = self.range_tile() {
            tiles.push(tile);
        }
        if self.flags.get("usage_statistics") {
            let [today, _week, _month] = self.app.usage_summaries(Instant::now(), offset);
            let measured = jafnify_core::scope::ScopeTotals {
                up_bytes: today.up_bytes,
                down_bytes: today.down_bytes,
                peak_down_bps: today.peak_down_bps,
            };
            let session = self.app.scope_session();
            let asked = self.app.tile_scope(jafnify_core::tiles::ID_TODAY);
            // The core owns the rule: the numbers and the label of a tile
            // always belong to the same scope.
            let (scope, totals) = jafnify_core::scope::totals(
                asked,
                jafnify_core::tiles::ID_TODAY,
                measured,
                session.as_ref(),
            );
            if let Some(tile) = jafnify_core::tiles::today_tile(
                &totals,
                scope,
                jafnify_core::tiles::Measure::Laptop,
            ) {
                tiles.push(tile);
            }
            let average_scope = jafnify_core::scope::effective(
                self.app.tile_scope(jafnify_core::tiles::ID_AVERAGE),
                jafnify_core::tiles::ID_AVERAGE,
                session.as_ref(),
            );
            if let Some(tile) =
                jafnify_core::tiles::average_tile(&today, session.as_ref(), now_ms, average_scope)
            {
                tiles.push(tile);
            }
        }
        if self.flags.get("drop_history") {
            let scope = self.app.tile_scope(jafnify_core::tiles::ID_DROPS);
            let session = self.app.scope_session();
            if let Some(tile) = jafnify_core::tiles::drops_tile(
                &self.app.drop_events(),
                now_ms,
                offset,
                scope,
                session.as_ref(),
            ) {
                tiles.push(tile);
            }
        }
        // The layout owns the slots: one tile per slot, in the order the user
        // chose, with the muted "--" tile for a slot whose data is missing.
        jafnify_core::home_layout::arrange(
            &self.app.home_layout(),
            jafnify_core::home_layout::Platform::Linux,
            tiles,
        )
    }

    /// The way the peer is reached, in words.
    fn path_text(&self) -> Option<String> {
        jafnify_core::connection_details::path_text(self.app.health().path)
    }

    /// The good-time notice of this moment, as the chip of the status card.
    /// It is only about a running tunnel: a card that says "not connected"
    /// must not carry a chip about the big downloads.
    fn good_time_notice(&self, state: ConnectionState) -> Option<String> {
        if !self.flags.get("good_time_notice") || state != ConnectionState::Connected {
            return None;
        }
        let (stats, age_secs, _) = self.phone_stats.snapshot(Instant::now());
        let fresh = age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
        let good = fresh
            && stats.as_ref().map(|stats| {
                jafnify_core::good_time::is_good(
                    stats.net,
                    stats.signal,
                    stats.rtt_ms,
                    self.phone_stats.wifi_leg().as_ref(),
                )
            }) == Some(true);
        good.then(|| jafnify_core::good_time::NOTICE_PREFIX.to_string())
    }

    /// The lines of the best-spot block, in the core wording. There is
    /// nothing to show before the first session starts.
    fn best_spot_lines(&self, now_ms: i64) -> Vec<String> {
        let state = self.app.best_spot_state();
        if state.started_at.is_none() {
            return Vec::new();
        }
        crate::flags::best_spot_lines(
            Vec::new(),
            self.flags.get("best_spot"),
            &state.samples,
            &state.marks,
            now_ms,
        )
    }

    /// The answer to a best-spot command: whether the session runs, whether
    /// it shows its marks and the lines to show.
    fn best_spot_reply(&self) -> Response {
        let now_ms = self.now_ms();
        Response::BestSpot {
            running: self.app.best_spot_running(now_ms),
            active: self.app.best_spot_showing(),
            lines: self.best_spot_lines(now_ms),
        }
    }

    /// Close the notifier while all the notification switches are off.
    fn release_notifier(&mut self) {
        if self.notifier_pinned {
            return;
        }
        if !self.flags.get("notify_connection_change")
            && !self.flags.get("phone_network_warning")
            && !self.flags.get("phone_heat_warning")
            && !self.flags.get("good_time_notice")
        {
            self.notifier = None;
        }
    }

    fn sync_tray(&mut self) {
        let window_open = self.presence.is_open(Instant::now());
        let visible = tray_should_be_visible(self.app.tray_only_when_closed(), window_open);
        self.tray.set_visible(visible);
        let error = self.event_error.is_some() || self.app.last_error().is_some();
        self.tray
            .set(TrayVisual::from_state_and_error(self.app.state(), error));
    }

    fn spawn_window(&mut self) {
        match crate::control::spawn_window() {
            Ok(child) => self.children.track(child),
            Err(e) => self.event_error = Some(e.to_string()),
        }
    }

    fn handle_tray(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::Open => {
                self.spawn_window();
            }
            TrayCommand::OpenPage(page) => {
                // Leave the hint for the window; a closed window is started
                // and takes it on its first poll, an open one on its next.
                self.pending_page = Some(page.key().to_string());
                self.spawn_window();
            }
            TrayCommand::Connect => {
                if jafnify_core::terms::is_accepted(&self.config_dir) && self.app.is_paired() {
                    if let Err(e) = self.app.start_known() {
                        self.event_error = Some(e.to_string());
                    }
                } else {
                    self.spawn_window();
                }
            }
            TrayCommand::Disconnect => {
                if let Err(e) = self.app.stop() {
                    self.event_error = Some(e.to_string());
                }
            }
            TrayCommand::CopyDiagnostics => {
                if let Some(request) = command.request() {
                    match self.handle(request) {
                        Response::Diagnostics { text } => {
                            if let Err(e) = crate::clipboard::copy(&text) {
                                self.event_error = Some(format!("Could not copy diagnostics: {e}"));
                            }
                        }
                        _ => {
                            self.event_error =
                                Some("The daemon did not return the diagnostics".to_string())
                        }
                    }
                }
            }
            TrayCommand::Exit => match self.app.stop() {
                Ok(()) => self.quit = true,
                Err(e) => {
                    self.event_error = Some(e.to_string());
                    self.spawn_window();
                }
            },
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::GetState => {
                self.presence.seen(Instant::now());
                Response::State(Box::new(self.snapshot()))
            }
            // A status poll is read-only: it must not count as an open window.
            Request::Status => Response::State(Box::new(self.snapshot())),
            Request::WindowClosed => {
                self.presence.closed();
                Response::Ok { note: None }
            }
            Request::TakeOpenPage => Response::Page {
                page: self.pending_page.take(),
            },
            Request::Connect => {
                let out = self.app.start_known().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::TryAgain => {
                let out = self.app.try_again().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::Disconnect => {
                let out = self.app.stop().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::Pair { code } => {
                let out = self.app.start(&code).map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::Forget => {
                let out = self.app.forget().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::AllowFirewall => match self.app.allow_firewall() {
                Ok(iface) => {
                    self.event_error = None;
                    Response::Ok {
                        note: Some(format!("Rule added for {iface}. Searching again.")),
                    }
                }
                Err(e) => Response::Err {
                    message: e.to_string(),
                },
            },
            Request::AcceptTerms => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                match jafnify_core::terms::record_acceptance(&self.config_dir, now) {
                    Ok(()) => Response::Ok { note: None },
                    Err(e) => Response::Err {
                        message: format!("Could not save the acceptance: {e}"),
                    },
                }
            }
            Request::SetSettings {
                role,
                autostart,
                kill_switch,
                tray_only_when_closed,
                theme,
                flags,
                nickname,
            } => {
                let out = self.apply_settings(SettingsPatch {
                    role: &role,
                    autostart,
                    kill_switch,
                    tray_only_when_closed,
                    theme: theme.as_deref(),
                    flags: &flags,
                    nickname: nickname.as_deref(),
                });
                self.finish(out, None)
            }
            Request::SetTheme { theme } => {
                let out = match ThemeMode::parse(&theme) {
                    Some(mode) => {
                        self.app.set_theme(mode);
                        self.app.save().map_err(|e| e.to_string())
                    }
                    None => Err(format!("unknown theme: {theme}")),
                };
                self.finish(out, None)
            }
            Request::SetFlag { id, value } => {
                let out = self.set_flag(&id, value);
                self.finish(out, None)
            }
            Request::SetTileScope { id, scope } => {
                let out = self.set_tile_scope(&id, &scope);
                self.finish(out, None)
            }
            Request::SetHomeLayout { layout } => {
                let out = self.app.set_home_layout_text(&layout);
                self.finish(out, None)
            }
            // `serve` hands LinkTest to `start_link_test` (worker thread);
            // this arm never runs the probe, so the loop cannot block here.
            Request::LinkTest => Response::Err {
                message: "link test must be started through the control stream".to_string(),
            },
            Request::SetViewState {
                details_expanded,
                advanced_expanded,
            } => {
                self.app.set_view_state(details_expanded, advanced_expanded);
                let out = self.app.save().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::SetWindowGeometry { window } => {
                self.app.set_window_geometry(window);
                let out = self.app.save().map_err(|e| e.to_string());
                self.finish(out, None)
            }
            Request::Diagnostics => Response::Diagnostics {
                text: self.diagnostics(),
            },
            Request::GetDrops => Response::Drops {
                summary: self.drop_summary(),
                rows: self.drop_rows(),
            },
            // The daemon owns the file: the window must not delete it behind
            // the daemon's back.
            Request::ClearDrops => {
                self.app.clear_drop_history();
                Response::Ok {
                    note: Some("Drop history cleared.".to_string()),
                }
            }
            Request::BestSpotStart => {
                self.app.best_spot_start(Instant::now());
                self.best_spot_reply()
            }
            Request::BestSpotMark => {
                self.app.best_spot_mark(Instant::now());
                self.best_spot_reply()
            }
            Request::BestSpotStop => {
                self.app.best_spot_stop();
                self.best_spot_reply()
            }
            Request::GetGoodHours => Response::GoodHours {
                lines: self.good_hours_lines(),
            },
            // The daemon owns the file: the window must not delete it behind
            // the daemon's back.
            Request::ClearGoodHours => {
                self.app.clear_good_hours();
                Response::Ok {
                    note: Some("Good hours cleared.".to_string()),
                }
            }
        }
    }

    /// The summary of the drop history of the last day.
    fn drop_summary(&self) -> String {
        let now_ms = self.now_ms();
        let records = self.app.drop_records();
        jafnify_core::drop_history::summary(&records, now_ms)
    }

    /// The newest drops, newest first, in the core wording.
    fn drop_rows(&self) -> Vec<String> {
        let offset = crate::applog::local_offset_minutes(self.now_ms());
        let records = self.app.drop_records();
        records
            .iter()
            .rev()
            .take(DROP_ROWS)
            .map(|record| record.detail(offset))
            .collect()
    }

    /// The wall-clock time in epoch milliseconds.
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
    }

    /// The redacted diagnostics export: the connection log, the journal tail
    /// and the application log tail. The daemon owns the files and the
    /// in-memory log, so the window asks for the text instead of reading the
    /// files itself.
    fn diagnostics(&self) -> String {
        self.diagnostics_from(export::journal_tail(), export::app_log_tail())
    }

    /// The export, from the pieces the daemon already has. Split out so a
    /// test can build it without touching the operator's log files.
    fn diagnostics_from(&self, journal: Vec<String>, app_log: Vec<String>) -> String {
        let aliases = self.peer_aliases();
        let alias_refs: Vec<&str> = aliases.iter().map(String::as_str).collect();
        export::bundle(
            &self.app.connection_log(),
            &journal,
            &app_log,
            &export::link_log_tail_in(&self.config_dir),
            &alias_refs,
            &export::hostname(),
        )
    }

    /// Every spelling of the paired peer's identity that is safe to register:
    /// the human name, the key prefix and the pair id. An endpoint is never an
    /// alias: a private peer address must survive the export unchanged.
    fn peer_aliases(&self) -> Vec<String> {
        let mut aliases = Vec::new();
        if let Some(name) = self.app.paired_name() {
            aliases.push(name);
        }
        if let Some(peer) = jafnify_core::known::stored_pair(self.app.peers()) {
            if !aliases.contains(&peer.name) {
                aliases.push(peer.name.clone());
            }
            aliases.push(key_prefix(&peer.public_key));
            aliases.push(peer.id.clone());
        }
        aliases
    }

    fn finish(&mut self, result: Result<(), String>, note: Option<String>) -> Response {
        match result {
            Ok(()) => {
                self.event_error = None;
                match note {
                    Some(note) => Response::Ok { note: Some(note) },
                    // State-changing calls publish the fresh snapshot so the
                    // window can see an attempt note before the next tick.
                    None => Response::State(Box::new(self.snapshot())),
                }
            }
            Err(message) => Response::Err { message },
        }
    }

    fn apply_settings(&mut self, patch: SettingsPatch<'_>) -> Result<(), String> {
        let SettingsPatch {
            role,
            autostart,
            kill_switch,
            tray_only_when_closed,
            theme,
            flags,
            nickname,
        } = patch;
        let role = match role {
            "sharer" => Role::Sharer,
            "joiner" => Role::Joiner,
            other => return Err(format!("unknown role: {other}")),
        };
        // An old window sends no theme: keep the saved one. Only an explicit
        // value changes it.
        let theme = match theme {
            Some(value) => {
                Some(ThemeMode::parse(value).ok_or_else(|| format!("unknown theme: {value}"))?)
            }
            None => None,
        };
        self.app.set_role(role);
        self.app.set_tray_only_when_closed(tray_only_when_closed);
        if let Some(name) = nickname {
            let path = self.config_dir.join(crate::local_name::FILE_NAME);
            self.nickname = crate::local_name::save(&path, name).map_err(|e| e.to_string())?;
        }
        if let Some(theme) = theme {
            self.app.set_theme(theme);
        }
        // `set_kill_switch` saves the state file. Keep the old autostart value
        // there until enabling the unit succeeds, so a masked unit never leaves
        // a checked box behind.
        self.app
            .set_kill_switch(kill_switch)
            .map_err(|e| e.to_string())?;
        // Rewrite the unit even when the checkbox did not change: an upgrade
        // from the old `start` unit must become `tray`.
        self.set_autostart(autostart)?;
        self.app.set_autostart(autostart);
        self.app.save().map_err(|e| e.to_string())?;
        self.apply_flags(flags)
    }

    /// Store one flag switch. Writes `flags.toml` only, never `state.toml`,
    /// autostart, the kill switch, the role, the theme or the helper.
    fn set_flag(&mut self, id: &str, value: bool) -> Result<(), String> {
        if self.store_flag(id, value) {
            self.save_flags()?;
        }
        Ok(())
    }

    /// Store the scope of one tile. The core owns the words and the order:
    /// a word the core does not know is refused, and a tile that does not
    /// support the scope keeps its default.
    fn set_tile_scope(&mut self, id: &str, scope: &str) -> Result<(), String> {
        let word = jafnify_core::scope::TileScope::parse(scope)
            .ok_or_else(|| format!("unknown scope: {scope}"))?;
        if !jafnify_core::scope::TileScope::is_scopeable(id) {
            return Err(format!("the {id} tile has no scope to change"));
        }
        if !word.supports(id) {
            return Err(format!("the {id} tile has no {scope} scope"));
        }
        self.app.set_tile_scope(id, scope);
        self.app.save_tile_scopes().map_err(|e| e.to_string())
    }

    /// Store the flag values a `SetSettings` request may change. Unknown,
    /// unwired and unsupported ids are ignored with a log line and are never
    /// written, so a released window cannot create a key no code reads.
    fn apply_flags(&mut self, incoming: &BTreeMap<String, bool>) -> Result<(), String> {
        let mut accepted = false;
        for (id, value) in incoming {
            accepted |= self.store_flag(id, *value);
        }
        if accepted {
            self.save_flags()?;
        }
        Ok(())
    }

    /// The shared refusal rule; returns whether the id was accepted.
    fn store_flag(&mut self, id: &str, value: bool) -> bool {
        match crate::flags::writable(id, jafnify_core::flags::platform::LINUX) {
            Ok(_) => {
                self.flags.set(id, value);
                true
            }
            Err(refusal) => {
                eprintln!("ignoring flag {id}: {}", refusal.reason());
                false
            }
        }
    }

    fn save_flags(&self) -> Result<(), String> {
        self.flags
            .save(&self.config_dir.join(crate::flags::FILE_NAME))
            .map_err(|e| e.to_string())
    }

    fn set_autostart(&self, enabled: bool) -> Result<(), String> {
        let unit = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("systemd/user")
            .join(crate::flavor::UNIT);
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let exec = format!("{} tray", exe.display());
            crate::autostart::enable(&unit, &exec, &PathBuf::from("systemctl")).map_err(|e| match e
            {
                crate::autostart::AutostartError::Masked => e.to_string(),
                other => format!("Autostart error: {other}"),
            })
        } else {
            let _ = crate::autostart::disable(&unit, &PathBuf::from("systemctl"));
            Ok(())
        }
    }

    fn snapshot(&self) -> Snapshot {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let state = self.app.state();
        let reason = retry_reason(&self.app.status_text()).to_string();
        let peer = self.app.peers().first();
        let health = self.app.health();
        let health_line = health.verdict();
        let health_detail = health.handshake_detail();
        let (phone_stats, phone_stats_age_secs, phone_stats_unavailable) =
            self.phone_stats.snapshot(Instant::now());
        let (wifi_ms, wifi_loss_pct, wifi_n) = match self.phone_stats.wifi_leg() {
            Some(leg) => (
                Some(leg.median_ms),
                Some(leg.loss_pct),
                Some(leg.samples as u64),
            ),
            None => (None, None, None),
        };
        // The heat line needs a fresh reading: a phone that was hot minutes
        // ago says nothing about the phone now. The temperature is the one
        // that made it hot.
        let phone_hot_temp_c_x10 = if state == ConnectionState::Connected
            && phone_stats_age_secs
                .map(|age| age <= FRESH_PHONE_SECS)
                .unwrap_or(false)
        {
            self.heat.hot_temp_c_x10()
        } else {
            None
        };
        let phone_good_time = state == ConnectionState::Connected
            && phone_stats_age_secs
                .map(|age| age <= FRESH_PHONE_SECS)
                .unwrap_or(false)
            && phone_stats
                .as_ref()
                .map(|stats| {
                    jafnify_core::good_time::is_good(
                        stats.net,
                        stats.signal,
                        stats.rtt_ms,
                        self.phone_stats.wifi_leg().as_ref(),
                    )
                })
                .unwrap_or(false);
        let usage = if self.flags.get("usage_statistics") {
            let [today, week, month] = self.app.usage_summaries(
                Instant::now(),
                crate::applog::local_offset_minutes(now as i64 * 1_000),
            );
            Some(crate::control::UsageView {
                today: today.lines(),
                week: week.lines(),
                month: month.lines(),
            })
        } else {
            None
        };
        // The session rows and the session tile read the same drop events:
        // the tracker's own counter also sees a stop of the phone as a state
        // change, which is not a drop.
        let drop_events = self.app.drop_events();
        let current_session = self.app.current_session();
        let session_drops = self
            .app
            .scope_session()
            .map(|scope| {
                jafnify_core::tiles::session_drop_count(
                    &drop_events,
                    &scope,
                    now as i64 * 1_000,
                )
            })
            .unwrap_or(0);
        let session_history = self
            .app
            .session_history()
            .into_iter()
            .map(|mut record| {
                let scope = jafnify_core::scope::ScopeSession {
                    started_at_ms: record.started_at,
                    up_bytes: record.up_bytes,
                    down_bytes: record.down_bytes,
                    peak_down_bps: record.peak_down_bps,
                };
                let ended = record.ended_at.max(record.started_at);
                record.drops = jafnify_core::tiles::session_drop_count(&drop_events, &scope, ended);
                record
            })
            .collect();
        Snapshot {
            state: state_name(state).to_string(),
            status_text: self.app.status_text(),
            paired_name: self.app.paired_name(),
            paired: self.app.is_paired(),
            denied: self.app.denied(),
            retry_reason: (state == ConnectionState::Retry && !reason.is_empty()).then_some(reason),
            no_peer: self.no_peer,
            connected_secs: self.app.connected_secs(),
            traffic: self.app.traffic(),
            last_connected_ago: self
                .app
                .last_connected()
                .filter(|&t| t > 0)
                .map(|t| now.saturating_sub(t)),
            retry_secs_left: self.app.retry_secs_left(),
            health_line,
            health_detail,
            path: jafnify_core::connection_details::path_text(health.path),
            nickname: self.nickname.clone(),
            dns_through_tunnel: self.app.dns_through_tunnel(),
            peer_endpoint: self.app.peer_endpoint(),
            foreign_tunnel: self.app.foreign_tunnel(),
            code_expired: self.app.code_expired(),
            firewall: self.app.firewall_blocked().map(|kind| match kind {
                FirewallKind::Ufw => "ufw".to_string(),
                FirewallKind::Other => "other".to_string(),
            }),
            firewall_iface: self.app.firewall_iface().map(str::to_string),
            dns_override: self.app.host_dns_override().map(str::to_string),
            last_error: self
                .app
                .last_error()
                .map(str::to_string)
                .or_else(|| self.event_error.clone()),
            role: match self.app.role() {
                Role::Sharer => "sharer".to_string(),
                Role::Joiner => "joiner".to_string(),
            },
            autostart: self.app.autostart(),
            kill_switch: self.app.kill_switch(),
            tray_only_when_closed: self.app.tray_only_when_closed(),
            details_expanded: self.app.details_expanded(),
            advanced_expanded: self.app.advanced_expanded(),
            terms_ok: jafnify_core::terms::is_accepted(&self.config_dir),
            ufw_active: self.app.ufw_active(),
            pair_id: peer.map(|p| p.id.clone()),
            peer_key_prefix: peer.map(|p| key_prefix(&p.public_key)),
            current_session,
            session_history,
            session_drops,
            connection_log: self.app.connection_log(),
            window: self.app.window_geometry(),
            polkit_note: self.app.polkit_prompt_expected(),
            link_lost: self.app.link_lost(),
            no_network: self.app.no_network(),
            theme: self.app.theme().as_str().to_string(),
            flags: self.flags.effective(),
            tile_scopes: self.app.tile_scopes(),
            home_layout: self.app.home_layout().to_text(),
            phone_stats,
            phone_stats_age_secs,
            phone_stats_unavailable,
            usage,
            host_hints: self.host_hint_rows(),
            wifi_ms,
            wifi_loss_pct,
            wifi_n,
            phone_hot_temp_c_x10,
            phone_good_time,
            best_spot_running: self.app.best_spot_running(self.now_ms()),
            best_spot_active: self.app.best_spot_showing(),
            best_spot_lines: self.best_spot_lines(self.now_ms()),
            good_hours_lines: self.good_hours_lines(),
            home: self.home(),
        }
    }

    /// The hints the read-only probe found, plus the firewall hint the app
    /// already knows. The text never carries a network name or an address.
    fn host_hint_rows(&self) -> Vec<crate::control::HostHintRow> {
        let mut hints = self.probe.hints(Instant::now());
        if let Some(kind) = self.app.firewall_blocked() {
            hints.push(crate::host_hints::firewall_hint(
                kind,
                self.app.firewall_iface(),
            ));
        }
        hints
            .into_iter()
            .map(|hint| crate::control::HostHintRow {
                title: hint.title(),
                help: hint.help(),
            })
            .collect()
    }

    fn serve(&mut self, mut stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let reader = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::LinkTest) => match self.start_link_test(stream) {
                Ok(()) => return,
                Err((back, response)) => {
                    stream = back;
                    response
                }
            },
            Ok(request) => self.handle(request),
            Err(e) => Response::Err {
                message: format!("bad request: {e}"),
            },
        };
        let _ = serde_json::to_writer(&mut stream, &response);
        let _ = stream.write_all(b"\n");
    }
}

impl Runtime {
    /// A connected link test takes up to ~10 s. It runs on its own thread and
    /// answers on the request's stream, so the daemon loop (liveness,
    /// reconnect, tray) never waits for it. Refused while another test runs:
    /// the action is always available now, with no switch to gate it.
    #[allow(clippy::result_large_err)]
    fn start_link_test(&mut self, stream: UnixStream) -> Result<(), (UnixStream, Response)> {
        use std::sync::atomic::Ordering;
        let connected = self.app.state() == ConnectionState::Connected;
        if !connected {
            let result = crate::link_test::run_default(false);
            return Err((stream, link_test_reply(result)));
        }
        if self.link_test_busy.swap(true, Ordering::SeqCst) {
            return Err((
                stream,
                Response::Err {
                    message: "A link test is already running".to_string(),
                },
            ));
        }
        let busy = self.link_test_busy.clone();
        std::thread::spawn(move || {
            // The flag clears even if the worker panics, so "already running"
            // is never permanent after one bad test.
            let _busy = LinkTestBusy(busy);
            let mut stream = stream;
            let result = crate::link_test::run_default(true);
            let _ = serde_json::to_writer(&mut stream, &link_test_reply(result));
            let _ = stream.write_all(b"\n");
        });
        Ok(())
    }
}

/// Clears the link-test busy flag when the worker ends, including on a panic.
struct LinkTestBusy(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for LinkTestBusy {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn link_test_reply(result: jafnify_core::link_test::LinkTestResult) -> Response {
    Response::LinkTest {
        line: result.line(),
        sent: result.sent,
        avg_ms: result.avg_ms,
        loss_pct: result.loss_pct,
    }
}

fn state_name(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Idle => "idle",
        ConnectionState::Searching => "searching",
        ConnectionState::Pairing => "pairing",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Retry => "retry",
    }
}

/// A second `jafnify tray`: print the core's one line and, when the GUI is
/// installed, open the one window. The open is best effort: a missing GUI
/// leaves the line as the only result, and the exit code stays 0.
pub fn second_instance(say: impl FnOnce(&str), open_window: impl FnOnce() -> std::io::Result<()>) {
    say(jafnify_core::share_notice::ALREADY_IN_TRAY);
    let _ = open_window();
}

pub fn run() -> Result<(), String> {
    let lock_path = lock_path().map_err(|e| e.to_string())?;
    let Some(_lock) = InstanceLock::acquire(&lock_path).map_err(|e| e.to_string())? else {
        second_instance(
            |line| eprintln!("{line}"),
            || crate::control::spawn_window().map(|_child| ()),
        );
        return Ok(());
    };
    crate::stop::install();
    let socket_path = socket_path().map_err(|e| e.to_string())?;
    let listener = bind_socket(&socket_path).map_err(|e| e.to_string())?;
    let config_dir = config_dir();
    // The lock is ours, so this is the one place that may close the previous
    // process's unfinished live session and append it to the history. A CLI
    // App build stays read-only.
    if let Err(e) =
        crate::session::SessionHistory::recover_unfinished(&config_dir.join("sessions.toml"))
    {
        eprintln!("warning: could not recover the unfinished session: {e}");
    }
    let app = AppBuilder::new(config_dir.clone())
        .logger(applog::open_default_unless_test())
        .diag_path(applog::diag_path_unless_test())
        .session_history_owner(true)
        .build();
    let mut runtime = Runtime::new(app, config_dir);
    let mut worker_log = applog::open_default_unless_test();
    crate::phone_stats::spawn_worker(
        runtime.phone_stats.clone(),
        Box::new(crate::phone_stats::UdpTransport::new()),
        move |level, message| {
            if let Some(log) = worker_log.as_mut() {
                log.event(level, message);
            }
        },
    );
    if let Err(e) = runtime.app.cleanup_stale_state() {
        runtime.event_error = Some(e.to_string());
    }
    runtime.app.clear_stale_routing();
    runtime.start_known_if_possible();

    loop {
        if crate::stop::requested() {
            // The service is stopping: tell the peer, then remove the dead
            // tunnel's routing before the process goes away, so the laptop
            // stays online.
            runtime.app.say_bye();
            runtime.app.fall_back_routing();
            break;
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => runtime.serve(stream),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("control socket: {e}")),
            }
        }
        while let Some(command) = runtime.tray.try_recv() {
            runtime.handle_tray(command);
        }
        runtime.tick();
        runtime.sync_tray();
        if runtime.quit {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    runtime.tray.shutdown();
    // The hour in the making is written before the daemon goes away: a clean
    // stop does not lose it.
    runtime.app.flush_good_hours();
    // The live session is closed with the last figures, so a clean exit does
    // not wait for the next start to write it.
    runtime.app.close_session_on_exit();
    drop(listener);
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{load_state, save_state, LinuxStateFile, Settings};
    use crate::theme::ThemeMode;
    use jafnify_core::known::pair_record;
    use jafnify_core::phone_stats::NetworkKind;
    use jafnify_core::KeyPair;

    fn stored_settings(runtime: &Runtime) -> Settings {
        load_state(&runtime.config_dir.join("state.toml"))
            .unwrap()
            .settings
    }

    fn runtime_with_pair(role: Role) -> (tempfile::TempDir, Runtime) {
        runtime_with_pair_endpoint(role, None)
    }

    fn runtime_with_pair_endpoint(
        role: Role,
        last_endpoint: Option<String>,
    ) -> (tempfile::TempDir, Runtime) {
        runtime_with_pair_endpoint_and_drops(role, last_endpoint, None)
    }

    fn runtime_with_pair_endpoint_and_drops(
        role: Role,
        last_endpoint: Option<String>,
        drops: Option<&str>,
    ) -> (tempfile::TempDir, Runtime) {
        runtime_with_pair_endpoint_drops_and_usage(role, last_endpoint, drops, None)
    }

    /// A runtime with a pair, a drop file and a usage file. The files are in
    /// place before the app is built: the stores read them at the start.
    fn runtime_with_pair_endpoint_drops_and_usage(
        role: Role,
        last_endpoint: Option<String>,
        drops: Option<&str>,
        usage: Option<&str>,
    ) -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("cfg");
        let rec = pair_record(
            &KeyPair::generate(),
            &KeyPair::generate().public_key_b64(),
            role,
            last_endpoint,
            1,
        )
        .unwrap();
        save_state(
            &config_dir.join("state.toml"),
            &LinuxStateFile {
                peers: vec![rec],
                settings: Settings {
                    role,
                    ..Settings::default()
                },
            },
        )
        .unwrap();
        if let Some(text) = drops {
            std::fs::write(config_dir.join(crate::drop_store::FILE_NAME), text).unwrap();
        }
        if let Some(text) = usage {
            std::fs::write(config_dir.join("usage.tsv"), text).unwrap();
        }
        let app = AppBuilder::new(config_dir.clone())
            .iface_pin(std::net::Ipv4Addr::LOCALHOST)
            // Hermetic: never look at the host's real jafn0 counters.
            .rx_bytes_path(dir.path().join("rx"))
            .tx_bytes_path(dir.path().join("tx"))
            .session_history_owner(true)
            .build();
        let last_state = app.state();
        let flags = crate::flags::FlagStore::load(&config_dir.join(crate::flags::FILE_NAME));
        let nickname = crate::local_name::load(&config_dir.join(crate::local_name::FILE_NAME));
        let runtime = Runtime {
            app,
            config_dir,
            flags,
            nickname,
            phone_stats: std::sync::Arc::new(crate::phone_stats::PhoneStatsShared::new()),
            link_test_busy: Default::default(),
            tray: TrayManager::default(),
            presence: WindowPresence::new(WINDOW_PRESENCE_TIMEOUT),
            children: Children::default(),
            last_state,
            notifier: None,
            notifier_pinned: false,
            notifications: NotificationPolicy::new(),
            last_phone_net: None,
            heat: Default::default(),
            exit_generation: 0,
            conn_sync_generation: 0,
            good_time: Default::default(),
            probe: crate::host_probe::HostProbe::new(std::sync::Arc::new(
                crate::host_probe::ProcessRunner,
            )),
            best_spot_generation: 0,
            good_hours_generation: 0,
            trends: Default::default(),
            trend_generation: 0,
            no_peer: false,
            event_error: None,
            pending_page: None,
            quit: false,
        };
        (dir, runtime)
    }

    #[test]
    fn the_desktop_notifier_stays_away_while_the_flags_are_off() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        assert!(runtime
            .notify_connection(ConnectionState::Searching, ConnectionState::Connected, now)
            .is_none());
        assert!(runtime
            .notify_phone_network(ConnectionState::Connected, Some(NetworkKind::Mobile), now)
            .is_none());
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn a_connection_change_sends_the_fixed_line() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("notify_connection_change", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        assert_eq!(
            runtime.notify_connection(ConnectionState::Searching, ConnectionState::Connected, now),
            Some("Jafnify connected.".to_string())
        );
        assert_eq!(
            runtime.notify_connection(
                ConnectionState::Connected,
                ConnectionState::Searching,
                now + Duration::from_secs(1)
            ),
            Some("Jafnify lost the connection.".to_string())
        );
        assert_eq!(
            *lines.lock().unwrap(),
            vec![
                "Jafnify connected.".to_string(),
                "Jafnify lost the connection.".to_string()
            ]
        );
    }

    #[test]
    fn the_phone_network_warning_uses_the_same_rate_model() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_network_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        assert!(runtime
            .notify_phone_network(ConnectionState::Connected, Some(NetworkKind::Mobile), now)
            .is_none());
        assert_eq!(
            runtime.notify_phone_network(
                ConnectionState::Connected,
                Some(NetworkKind::None),
                now + Duration::from_secs(60)
            ),
            Some("Phone lost mobile data.".to_string())
        );
        // The same line again inside the five-minute dedupe stays silent.
        assert!(runtime
            .notify_phone_network(
                ConnectionState::Connected,
                Some(NetworkKind::None),
                now + Duration::from_secs(120)
            )
            .is_none());
        assert_eq!(
            *lines.lock().unwrap(),
            vec!["Phone lost mobile data.".to_string()]
        );
    }

    /// A phone summary of a hot phone on a good network.
    fn hot_stats() -> jafnify_core::phone_stats::PhoneStats {
        jafnify_core::phone_stats::PhoneStats {
            net: NetworkKind::Mobile5gAdv,
            signal: Some(4),
            rtt_ms: Some(45),
            temp_c_x10: Some(430),
            ..Default::default()
        }
    }

    /// A phone summary of a phone on a slow network.
    fn slow_stats() -> jafnify_core::phone_stats::PhoneStats {
        jafnify_core::phone_stats::PhoneStats {
            net: NetworkKind::Mobile3g,
            signal: Some(2),
            rtt_ms: Some(300),
            ..Default::default()
        }
    }

    #[test]
    fn the_hot_phone_gets_one_notification() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert!(
            lines.lock().unwrap().is_empty(),
            "one reading is never enough"
        );
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
        assert_eq!(lines.lock().unwrap()[0], "Phone is hot (43 C)");
        // The phone stays hot: no second line.
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_hot_phone_notification_carries_the_help() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let bodies = fake.bodies_shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(
            *bodies.lock().unwrap(),
            vec![
                "Hot phones can slow mobile data down; this one does not report throttling."
                    .to_string()
            ]
        );
    }

    #[test]
    fn no_hot_phone_notification_while_the_switch_is_off() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn no_hot_phone_notification_while_the_tunnel_is_down() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Retry, Some(&stats), Some(1), now);
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn no_hot_phone_notification_when_the_summary_is_stale() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(31), now);
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn the_hot_phone_says_it_again_after_it_cooled_down() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let mut stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
        // The phone cools down.
        stats.temp_c_x10 = Some(380);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
        // And it gets hot again: that is news, after the second reading.
        stats.temp_c_x10 = Some(430);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_heat_line_and_its_reading_reach_the_connection_log() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        let log = runtime.app.connection_log().join("\n");
        assert!(
            log.contains("notified: Phone is hot (43 C), thr none, age 1 s"),
            "{log}"
        );
    }

    #[test]
    fn a_phone_that_cools_down_withdraws_the_heat_popup() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        let withdrawn = fake.withdrawals_shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let mut stats = hot_stats();
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
        assert_eq!(withdrawn.load(std::sync::atomic::Ordering::SeqCst), 0);

        stats.temp_c_x10 = Some(380);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(
            withdrawn.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the popup of the cooled phone is withdrawn"
        );
    }

    #[test]
    fn a_phone_that_stops_throttling_withdraws_the_popup() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let withdrawn = fake.withdrawals_shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let mut stats = hot_stats();
        stats.thr = Some(3);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        stats.thr = Some(1);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(withdrawn.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn a_unit_test_never_reaches_the_session_bus() {
        assert!(
            desktop_notifier().is_silent(),
            "the test notifier must never create a session bus"
        );
    }

    #[test]
    fn the_heat_line_keeps_the_temperature_that_made_the_phone_hot() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let now = Instant::now();
        let mut stats = hot_stats();
        stats.temp_c_x10 = Some(431);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(runtime.heat.hot_temp_c_x10(), Some(431));
        // The phone cools a little, but it is still hot: the line keeps the
        // number that made it hot.
        stats.temp_c_x10 = Some(400);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(runtime.heat.hot_temp_c_x10(), Some(431));
    }

    #[test]
    fn the_old_notifications_keep_an_empty_body() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let fake = crate::notification::FakeNotifier::new();
        let bodies = fake.bodies_shared();
        runtime.set_notifier(Box::new(fake));

        runtime.send_notification(Notification::Connected, Instant::now());
        assert_eq!(*bodies.lock().unwrap(), vec![String::new()]);
    }

    #[test]
    fn the_good_time_notice_comes_after_the_conditions_hold() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("good_time_notice", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let bad = slow_stats();
        let good = hot_stats();
        // The conditions were bad for ten minutes.
        runtime.update_good_time(ConnectionState::Connected, Some(&bad), Some(1), now);
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&bad),
            Some(1),
            now + Duration::from_secs(600),
        );
        // Now they are good, but the hold is not over.
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&good),
            Some(1),
            now + Duration::from_secs(601),
        );
        assert!(lines.lock().unwrap().is_empty());
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&good),
            Some(1),
            now + Duration::from_secs(721),
        );
        assert_eq!(
            *lines.lock().unwrap(),
            vec!["Good time for big downloads: 5G (advanced), signal 4/4".to_string()]
        );
    }

    #[test]
    fn no_good_time_notice_while_the_switch_is_off() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let bad = slow_stats();
        let good = hot_stats();
        runtime.update_good_time(ConnectionState::Connected, Some(&bad), Some(1), now);
        runtime.update_good_time(ConnectionState::Connected, Some(&good), Some(1), now);
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn no_good_time_notice_while_the_tunnel_is_down() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("good_time_notice", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let bad = slow_stats();
        let good = hot_stats();
        runtime.update_good_time(ConnectionState::Connected, Some(&bad), Some(1), now);
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&bad),
            Some(1),
            now + Duration::from_secs(600),
        );
        // The tunnel is down: the phone's numbers say nothing.
        runtime.update_good_time(
            ConnectionState::Retry,
            Some(&good),
            Some(1),
            now + Duration::from_secs(601),
        );
        runtime.update_good_time(
            ConnectionState::Retry,
            Some(&good),
            Some(1),
            now + Duration::from_secs(721),
        );
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn no_good_time_notice_when_the_summary_is_stale() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("good_time_notice", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let bad = slow_stats();
        let good = hot_stats();
        runtime.update_good_time(ConnectionState::Connected, Some(&bad), Some(1), now);
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&bad),
            Some(1),
            now + Duration::from_secs(600),
        );
        // The summary is too old: the phone's numbers say nothing.
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&good),
            Some(31),
            now + Duration::from_secs(601),
        );
        runtime.update_good_time(
            ConnectionState::Connected,
            Some(&good),
            Some(31),
            now + Duration::from_secs(721),
        );
        assert!(lines.lock().unwrap().is_empty());
    }

    /// Counts the tool calls and answers the power save question.
    struct CountingRunner {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::host_probe::CommandRunner for CountingRunner {
        fn run(&self, program: &str, args: &[&str], _timeout: Duration) -> Option<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if program == "nmcli" && args.contains(&"DEVICE,TYPE") {
                return Some("wlan0:802-11-wireless\n".to_string());
            }
            if program == "iw" && args.contains(&"power_save") {
                return Some("Power save: on\n".to_string());
            }
            None
        }
    }

    #[test]
    fn the_host_tools_run_only_while_the_details_are_open() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runner = CountingRunner {
            calls: calls.clone(),
        };
        runtime.set_probe(crate::host_probe::HostProbe::new(std::sync::Arc::new(
            runner,
        )));

        runtime.tick();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a closed window runs no tool"
        );

        runtime.presence.seen(Instant::now());
        runtime.app.set_view_state(true, false);
        runtime.tick();

        let mut hints = Vec::new();
        for _ in 0..200 {
            hints = runtime.snapshot().host_hints;
            if !hints.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "the tools run while the details are open"
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].title, "Wi-Fi power saving is on for wlan0.");
        assert!(hints[0].help.starts_with("This can add short stalls."));
    }

    #[test]
    fn the_host_hints_never_reach_the_diagnostics_export() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runner = CountingRunner {
            calls: calls.clone(),
        };
        runtime.set_probe(crate::host_probe::HostProbe::new(std::sync::Arc::new(
            runner,
        )));
        runtime.presence.seen(Instant::now());
        runtime.app.set_view_state(true, false);
        runtime.tick();

        let mut hints = Vec::new();
        for _ in 0..200 {
            hints = runtime.snapshot().host_hints;
            if !hints.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!hints.is_empty(), "the hint was found");

        let text = runtime.diagnostics_from(Vec::new(), Vec::new());
        assert!(!text.contains("power saving"), "{text}");
        assert!(!text.contains("access points"), "{text}");
        assert!(!text.contains("This computer"), "{text}");
    }

    #[test]
    fn the_daemon_answers_the_drop_history_and_clears_it() {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 60_000;
        let file = format!(
            "JFD1 1 drop at={at} reason=no-data net=5g-adv sig=1 temp=395 batt=80 rtt=120\n\
             JFD1 1 back at={} down=12000\n",
            at + 12_000
        );
        let (_dir, mut runtime) =
            runtime_with_pair_endpoint_and_drops(Role::Sharer, None, Some(&file));
        let path = runtime.config_dir.join(crate::drop_store::FILE_NAME);
        assert!(path.exists());

        let response = runtime.handle(Request::GetDrops);
        let Response::Drops { summary, rows } = response else {
            panic!("the daemon answers the drop history");
        };
        assert_eq!(summary, "Drops in the last 24 h: 1 (longest 12 s)");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].contains("down 12 s"), "{rows:?}");
        assert!(rows[0].contains("5G (advanced)"), "{rows:?}");
        assert!(rows[0].contains("likely weak signal"), "{rows:?}");

        let response = runtime.handle(Request::ClearDrops);
        assert!(matches!(response, Response::Ok { .. }));
        assert!(!path.exists(), "the daemon owns the file: it deletes it");
        let response = runtime.handle(Request::GetDrops);
        let Response::Drops { rows, .. } = response else {
            panic!("the daemon answers the drop history");
        };
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[test]
    fn a_private_peer_endpoint_is_not_a_redaction_alias() {
        let (_dir, runtime) =
            runtime_with_pair_endpoint(Role::Sharer, Some("10.9.9.2:9797".to_string()));
        let aliases = runtime.peer_aliases();
        assert!(
            !aliases.iter().any(|a| a.contains("10.9.9.2")),
            "a private endpoint was registered as an alias: {aliases:?}"
        );
        // The human name, the key prefix and the pair id stay aliases.
        assert!(
            aliases
                .iter()
                .any(|a| Some(a) == runtime.app.paired_name().as_ref()),
            "{aliases:?}"
        );
    }

    #[test]
    fn connect_returns_a_state_snapshot_with_the_attempt_note() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        match runtime.handle(Request::Connect) {
            Response::State(snapshot) => {
                assert_eq!(snapshot.state, "searching");
                assert!(snapshot.polkit_note, "the uncovered attempt is published");
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
    }

    #[test]
    fn status_returns_the_snapshot_and_does_not_open_a_window() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        assert!(!runtime.presence.is_open(Instant::now()));
        match runtime.handle(Request::Status) {
            Response::State(snapshot) => {
                assert_eq!(snapshot.paired_name, runtime.app.paired_name());
                assert_eq!(snapshot.status_text, runtime.app.status_text());
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert!(
            !runtime.presence.is_open(Instant::now()),
            "a status poll must not count as an open window"
        );
    }

    #[test]
    fn set_theme_persists_only_the_theme() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.app.set_autostart(true);
        runtime.app.set_kill_switch(true).unwrap();
        let before = stored_settings(&runtime);
        assert_eq!(before.role, Role::Sharer);
        assert!(before.autostart);
        assert!(before.kill_switch);

        for (name, expected) in [("dark", ThemeMode::Dark), ("light", ThemeMode::Light)] {
            match runtime.handle(Request::SetTheme {
                theme: name.to_string(),
            }) {
                Response::State(snapshot) => assert_eq!(snapshot.theme, name),
                other => panic!("expected a State snapshot, got {other:?}"),
            }
            let after = stored_settings(&runtime);
            assert_eq!(after.theme, expected);
            assert_eq!(after.role, before.role);
            assert_eq!(after.autostart, before.autostart);
            assert_eq!(after.kill_switch, before.kill_switch);
        }
    }

    #[test]
    fn set_theme_rejects_an_unknown_value_without_saving() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        let before = stored_settings(&runtime);
        match runtime.handle(Request::SetTheme {
            theme: "purple".to_string(),
        }) {
            Response::Err { message } => assert!(message.contains("unknown theme"), "{message}"),
            other => panic!("expected an error, got {other:?}"),
        }
        let after = stored_settings(&runtime);
        assert_eq!(after.theme, before.theme);
        assert_eq!(after.role, before.role);
    }

    #[test]
    fn old_set_settings_without_theme_keeps_the_saved_theme() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.app.set_theme(ThemeMode::Light);
        runtime.app.save().unwrap();
        let before = stored_settings(&runtime);
        assert_eq!(before.theme, ThemeMode::Light);

        match runtime.handle(Request::SetSettings {
            role: "sharer".to_string(),
            autostart: before.autostart,
            kill_switch: before.kill_switch,
            tray_only_when_closed: before.tray_only_when_closed,
            theme: None,
            flags: BTreeMap::new(),
            nickname: None,
        }) {
            Response::State(snapshot) => assert_eq!(snapshot.theme, "light"),
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert_eq!(stored_settings(&runtime).theme, ThemeMode::Light);
    }

    #[test]
    fn finish_returns_ok_only_when_there_is_a_note() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        assert!(matches!(
            runtime.finish(Ok(()), Some("Rule added".to_string())),
            Response::Ok { note: Some(_) }
        ));
        assert!(matches!(runtime.finish(Ok(()), None), Response::State(_)));
    }

    fn flags_path(runtime: &Runtime) -> PathBuf {
        runtime.config_dir.join(crate::flags::FILE_NAME)
    }

    fn set_settings_with_flags(runtime: &mut Runtime, flags: BTreeMap<String, bool>) -> Response {
        let before = stored_settings(runtime);
        runtime.handle(Request::SetSettings {
            role: match before.role {
                Role::Sharer => "sharer".to_string(),
                Role::Joiner => "joiner".to_string(),
            },
            autostart: before.autostart,
            kill_switch: before.kill_switch,
            tray_only_when_closed: before.tray_only_when_closed,
            theme: Some(before.theme.as_str().to_string()),
            flags,
            nickname: None,
        })
    }

    /// The drops tile counts the real drops only: an absence of the phone and
    /// a stop on the phone are not faults and are not counted.
    #[test]
    fn the_drops_tile_counts_only_the_real_drops() {
        let now = 1_712_345_678_901;
        let file = "JFD1 1 drop at=1712326800000 reason=no-network\n\
                    JFD1 1 away at=1712326800000\n\
                    JFD1 1 back at=1712327000000 down=200000\n";
        let (_dir, mut runtime) =
            runtime_with_pair_endpoint_and_drops(Role::Sharer, None, Some(file));
        runtime.flags.set("drop_history", true);
        let tiles = runtime.tiles(now, 0, None, None);
        let drops = tiles
            .iter()
            .find(|tile| tile.id == jafnify_core::tiles::ID_DROPS)
            .expect("the drops tile");
        assert_eq!(
            drops.big.as_ref().map(|big| big.value.as_str()),
            Some("0"),
            "the away is not a drop: {drops:?}"
        );
        assert_eq!(drops.label, "DROPS 24 H");
        let facts = drops.facts.as_ref().expect("the facts of the tile");
        assert!(
            facts.lines.iter().any(|line| line.contains("Phone away")),
            "{facts:?}"
        );
        assert!(
            facts.lines.iter().any(|line| line.contains("not a drop")),
            "{facts:?}"
        );
    }

    /// The scope of a tile is stored in its own file and read back: the state
    /// file stays readable by an older build.
    #[test]
    fn the_scope_of_a_tile_lives_in_its_own_file() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime
            .set_tile_scope(jafnify_core::tiles::ID_DROPS, "SESSION")
            .unwrap();
        let path = runtime.config_dir.join(crate::tile_scopes::FILE_NAME);
        let stored = crate::tile_scopes::load(&path);
        assert_eq!(
            stored
                .get(jafnify_core::tiles::ID_DROPS)
                .map(String::as_str),
            Some("SESSION")
        );
        assert_eq!(
            runtime.app.tile_scope(jafnify_core::tiles::ID_DROPS),
            jafnify_core::scope::TileScope::Session
        );
        let state = std::fs::read_to_string(runtime.config_dir.join("state.toml")).unwrap();
        assert!(
            !state.contains("tile_scopes"),
            "the scopes leaked into state.toml: {state}"
        );
    }

    /// A scope the tile does not have is refused: the choice stays what the
    /// core allows.
    #[test]
    fn a_scope_the_tile_does_not_have_is_refused() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        assert!(runtime
            .set_tile_scope(jafnify_core::tiles::ID_TODAY, "24 H")
            .is_err());
        assert!(runtime
            .set_tile_scope(jafnify_core::tiles::ID_DROPS, "nonsense")
            .is_err());
        assert_eq!(
            runtime.app.tile_scope(jafnify_core::tiles::ID_TODAY),
            jafnify_core::scope::TileScope::Today
        );
    }

    /// While the phone is away there is no alarm: the laptop waits for the
    /// phone and comes back on its own.
    #[test]
    fn no_alarm_while_the_phone_is_away() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("notify_connection_change", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        runtime
            .app
            .set_outage_reason(Some(jafnify_core::drop_history::REASON_NO_NETWORK));
        assert!(
            runtime
                .notify_connection(ConnectionState::Connected, ConnectionState::Searching, now)
                .is_none(),
            "an absence of the phone gets no alarm"
        );
        assert!(lines.lock().unwrap().is_empty());
    }

    /// The label of the today tile and its numbers belong to the same scope:
    /// a session scope without a session falls back to today, and the tile
    /// says so. A tile that says "SESSION" over the bytes of today would lie.
    #[test]
    fn the_today_tile_names_the_scope_of_its_numbers() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as i64)
            .unwrap_or(0);
        let hour = jafnify_core::usage::hour_start_ms(now_ms);
        // An hour of sharing from this morning, but no session: the counters
        // never moved, so the sharing session never started.
        let usage = format!(
            "JFU1 1 hour={hour} up=1000000 down=5000000 min_up=0 min_down=0 \
             peak_up=0 peak_down=1000 connected_ms=60000 samples=10 net=5g\n"
        );
        let (_dir, mut runtime) =
            runtime_with_pair_endpoint_drops_and_usage(Role::Joiner, None, None, Some(&usage));
        runtime.flags.set("usage_statistics", true);
        runtime.app.set_usage_enabled(true);
        runtime
            .set_tile_scope(jafnify_core::tiles::ID_TODAY, "SESSION")
            .unwrap();
        let tiles = runtime.tiles(now_ms, 0, None, None);
        let today = tiles
            .iter()
            .find(|tile| tile.id == jafnify_core::tiles::ID_TODAY)
            .expect("the layout always keeps the slot");
        assert_eq!(
            today.big.as_ref().map(|big| big.value.as_str()),
            Some("--"),
            "a session that shared nothing shows the muted state, not the bytes of today: {today:?}"
        );
        runtime
            .set_tile_scope(jafnify_core::tiles::ID_TODAY, "TODAY")
            .unwrap();
        let tiles = runtime.tiles(now_ms, 0, None, None);
        let today = tiles
            .iter()
            .find(|tile| tile.id == jafnify_core::tiles::ID_TODAY)
            .expect("the today tile");
        assert_eq!(today.label, "TODAY", "{today:?}");
        let big = today.big.as_ref().expect("the number of the tile");
        assert!(big.value.contains("MB"), "{today:?}");
    }

    /// The return of an absence is quiet: the samplers resume silently.
    #[test]
    fn the_return_of_an_absence_is_quiet() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("notify_connection_change", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        runtime.app.set_away_quiet(true);
        assert!(
            runtime
                .notify_connection(ConnectionState::Retry, ConnectionState::Connected, now)
                .is_none(),
            "the return of an absence gets no line"
        );
        assert!(lines.lock().unwrap().is_empty());
    }

    #[test]
    fn the_local_name_lives_in_its_own_file_not_in_state_toml() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        let before = stored_settings(&runtime);
        let response = runtime.handle(Request::SetSettings {
            role: "joiner".to_string(),
            autostart: before.autostart,
            kill_switch: before.kill_switch,
            tray_only_when_closed: before.tray_only_when_closed,
            theme: None,
            flags: BTreeMap::new(),
            nickname: Some("Van laptop".to_string()),
        });
        match response {
            Response::State(snapshot) => assert_eq!(snapshot.nickname, "Van laptop"),
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        let state = std::fs::read_to_string(runtime.config_dir.join("state.toml")).unwrap();
        assert!(
            !state.contains("Van laptop"),
            "the name leaked into state.toml"
        );
        let name_path = runtime.config_dir.join(crate::local_name::FILE_NAME);
        assert_eq!(crate::local_name::load(&name_path), "Van laptop");
        // A request without the field (an older window) keeps the name.
        set_settings_with_flags(&mut runtime, BTreeMap::new());
        assert_eq!(runtime.snapshot().nickname, "Van laptop");
    }

    #[test]
    fn the_snapshot_carries_every_declared_flag_at_its_effective_value() {
        let (_dir, runtime) = runtime_with_pair(Role::Joiner);
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.flags.len(), jafnify_core::flags::FLAGS.len());
        for flag in jafnify_core::flags::FLAGS {
            assert_eq!(
                snapshot.flags.get(flag.id),
                Some(&flag.default),
                "{}",
                flag.id
            );
        }
    }

    #[test]
    fn set_settings_ignores_an_unknown_flag_and_writes_no_store() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        match set_settings_with_flags(
            &mut runtime,
            BTreeMap::from([("no_such_flag".to_string(), true)]),
        ) {
            Response::State(snapshot) => {
                assert!(!snapshot.flags.contains_key("no_such_flag"));
                for flag in jafnify_core::flags::FLAGS {
                    assert_eq!(
                        snapshot.flags.get(flag.id),
                        Some(&flag.default),
                        "{}",
                        flag.id
                    );
                }
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert!(!flags_path(&runtime).exists(), "a refused flag was written");
    }

    #[test]
    fn set_settings_ignores_an_unwired_flag_and_writes_no_store() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        match set_settings_with_flags(
            &mut runtime,
            BTreeMap::from([("unicast_search".to_string(), true)]),
        ) {
            Response::State(snapshot) => {
                assert_eq!(snapshot.flags.get("unicast_search"), Some(&false));
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert!(
            !flags_path(&runtime).exists(),
            "an unwired flag was written"
        );
    }

    #[test]
    fn set_settings_ignores_a_flag_for_another_platform() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        match set_settings_with_flags(
            &mut runtime,
            BTreeMap::from([("start_after_restart".to_string(), true)]),
        ) {
            Response::State(snapshot) => {
                assert_eq!(snapshot.flags.get("start_after_restart"), Some(&false));
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert!(!flags_path(&runtime).exists());
    }

    #[test]
    fn set_settings_never_rewrites_a_refused_key_already_in_the_store() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        let path = flags_path(&runtime);
        std::fs::write(&path, "unicast_search = true\n").unwrap();
        runtime.flags = crate::flags::FlagStore::load(&path);
        let before = std::fs::read_to_string(&path).unwrap();

        let _ = set_settings_with_flags(
            &mut runtime,
            BTreeMap::from([("unicast_search".to_string(), false)]),
        );

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(runtime.flags.get("unicast_search"));
    }

    fn link_test_answer(runtime: &mut Runtime) -> Response {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        match runtime.start_link_test(ours) {
            Err((_stream, response)) => response,
            Ok(()) => panic!("a worker was started"),
        }
    }

    #[test]
    fn link_test_is_available_without_a_switch() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        // test_link is no longer a row: the action is always available. While
        // idle it says "Connect first." and never starts a probe.
        match link_test_answer(&mut runtime) {
            Response::LinkTest { line, .. } => assert_eq!(line, "Connect first."),
            other => panic!("expected a LinkTest answer, got {other:?}"),
        }
        assert!(!runtime
            .link_test_busy
            .load(std::sync::atomic::Ordering::SeqCst));
        assert!(jafnify_core::flags::find("test_link").is_none());
    }

    #[test]
    fn link_test_while_idle_says_connect_first_without_sending() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        match link_test_answer(&mut runtime) {
            Response::LinkTest { line, .. } => assert_eq!(line, "Connect first."),
            other => panic!("expected a LinkTest answer, got {other:?}"),
        }
        assert!(!runtime
            .link_test_busy
            .load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn a_link_test_worker_that_panics_still_clears_the_busy_flag() {
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = busy.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = LinkTestBusy(flag);
            panic!("injected worker panic");
        }));
        assert!(result.is_err());
        assert!(!busy.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn the_request_loop_never_runs_the_probe() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        runtime.flags.set("test_link", true);
        assert!(matches!(
            runtime.handle(Request::LinkTest),
            Response::Err { .. }
        ));
    }

    #[test]
    fn set_flag_refuses_unknown_and_unwired_ids_without_writing() {
        for id in ["no_such_flag", "unicast_search", "start_after_restart"] {
            let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
            match runtime.handle(Request::SetFlag {
                id: id.to_string(),
                value: true,
            }) {
                Response::State(snapshot) => {
                    if jafnify_core::flags::find(id).is_some() {
                        assert_eq!(snapshot.flags.get(id), Some(&false), "{id}");
                    } else {
                        assert!(!snapshot.flags.contains_key(id), "{id}");
                    }
                }
                other => panic!("expected a State snapshot for {id}, got {other:?}"),
            }
            assert!(!flags_path(&runtime).exists(), "{id} was written");
        }
    }

    #[test]
    fn set_flag_round_trips_through_the_flag_store() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        let state_path = runtime.config_dir.join("state.toml");
        let before_bytes = std::fs::read(&state_path).unwrap();
        let before_mtime = std::fs::metadata(&state_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        match runtime.handle(Request::SetFlag {
            id: "best_spot".to_string(),
            value: true,
        }) {
            Response::State(snapshot) => {
                assert_eq!(snapshot.flags.get("best_spot"), Some(&true));
            }
            other => panic!("expected a State snapshot, got {other:?}"),
        }
        assert!(runtime.flags.get("best_spot"));
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            before_bytes,
            "SetFlag rewrote state.toml"
        );
        assert_eq!(
            std::fs::metadata(&state_path).unwrap().modified().unwrap(),
            before_mtime,
            "SetFlag touched state.toml"
        );
        let flags_text = std::fs::read_to_string(flags_path(&runtime)).unwrap();
        assert!(flags_text.contains("best_spot = true"), "{flags_text}");
    }

    #[test]
    fn usage_statistics_is_always_on_and_has_no_switch() {
        let (_dir, runtime) = runtime_with_pair(Role::Joiner);
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.flags.get("usage_statistics"), Some(&true));
        // The view is built from the standard feature, so it is there even
        // before the first sample; the file only appears when an hour ends.
        assert!(snapshot.usage.is_some());
        assert!(!runtime.config_dir.join("usage.tsv").exists());
    }

    #[test]
    fn usage_statistics_on_writes_the_store_and_shows_three_summaries() {
        let (dir, mut runtime) = runtime_with_pair(Role::Joiner);
        std::fs::write(dir.path().join("rx"), "0").unwrap();
        std::fs::write(dir.path().join("tx"), "0").unwrap();
        runtime.flags.set("usage_statistics", true);
        runtime.app.set_usage_enabled(true);
        runtime.app.sample_session(Instant::now());
        runtime.app.flush_usage();

        assert!(runtime.config_dir.join("usage.tsv").exists());
        let usage = runtime.snapshot().usage.expect("usage view");
        assert_eq!(usage.today.len(), 6);
        assert_eq!(usage.week.len(), 6);
        assert_eq!(usage.month.len(), 6);
        assert!(usage.today[0].starts_with("Shared today:"));
        assert!(usage.week[0].starts_with("Shared this week:"));
        assert!(usage.month[0].starts_with("Shared this month:"));
    }

    #[test]
    fn a_tray_page_hint_is_taken_once_and_cleared() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        runtime.pending_page = Some("settings".to_string());
        match runtime.handle(Request::TakeOpenPage) {
            Response::Page { page } => {
                assert_eq!(page.as_deref(), Some("settings"), "the hint is returned")
            }
            other => panic!("unexpected answer: {other:?}"),
        }
        match runtime.handle(Request::TakeOpenPage) {
            Response::Page { page } => assert_eq!(page, None, "the hint is cleared"),
            other => panic!("unexpected answer: {other:?}"),
        }
    }

    #[test]
    fn all_flags_off_matches_the_0_1_1_snapshot() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Joiner);
        let without_flags = runtime.snapshot();

        // The 0.1.1 golden fields: idle, the stored role, every boolean off,
        // the system theme and no session history.
        assert_eq!(without_flags.state, "idle");
        assert_eq!(without_flags.role, "joiner");
        assert!(!without_flags.autostart);
        assert!(!without_flags.kill_switch);
        assert!(!without_flags.tray_only_when_closed);
        assert_eq!(without_flags.theme, "system");
        assert!(without_flags.session_history.is_empty());

        let path = flags_path(&runtime);
        let mut store = crate::flags::FlagStore::load(&path);
        for flag in jafnify_core::flags::FLAGS {
            store.set(flag.id, false);
        }
        store.save(&path).unwrap();
        runtime.flags = crate::flags::FlagStore::load(&path);
        let all_false = runtime.snapshot();

        let mut golden = without_flags;
        golden.flags = BTreeMap::new();
        // The idle snapshot reports how long ago the last connection was;
        // that clock moves between the two snapshots. It is not a decision.
        golden.last_connected_ago = None;
        let mut actual = all_false;
        actual.flags = BTreeMap::new();
        actual.last_connected_ago = None;
        assert_eq!(
            actual, golden,
            "the flags map is the only decision difference"
        );
    }

    #[test]
    fn the_snapshot_carries_the_phone_stats_cache() {
        let (_dir, runtime) = runtime_with_pair(Role::Joiner);
        let stats = jafnify_core::phone_stats::PhoneStats {
            net: jafnify_core::phone_stats::NetworkKind::Wifi,
            signal: Some(2),
            ..Default::default()
        };
        runtime
            .phone_stats
            .record_for_test(stats.clone(), Instant::now());

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phone_stats, Some(stats));
        assert_eq!(snapshot.phone_stats_age_secs, Some(0));
        assert!(!snapshot.phone_stats_unavailable);
    }

    #[test]
    fn the_drop_context_carries_the_thermal_status() {
        // Android's own word about the heat is what the drop hint reads, so
        // the daemon has to hand it over with the rest of the context.
        let (_dir, runtime) = runtime_with_pair(Role::Joiner);
        let stats = jafnify_core::phone_stats::PhoneStats {
            net: NetworkKind::Mobile5gAdv,
            temp_c_x10: Some(300),
            thr: Some(3),
            ..Default::default()
        };
        runtime.phone_stats.record_for_test(stats, Instant::now());

        let sample = runtime.phone_context().expect("the phone sent a summary");
        assert_eq!(sample.context.thr, Some(3));
        assert_eq!(sample.context.temp_c_x10, Some(300));
    }

    /// A phone summary on 5G with a signal and a DNS answer time.
    fn spot_stats() -> jafnify_core::phone_stats::PhoneStats {
        jafnify_core::phone_stats::PhoneStats {
            net: NetworkKind::Mobile5gAdv,
            signal: Some(3),
            rtt_ms: Some(40),
            dbm: Some(-97),
            ..Default::default()
        }
    }

    #[test]
    fn the_best_spot_session_takes_one_sample_per_phone_summary() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("best_spot", true);
        runtime.app.best_spot_start(Instant::now());
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_best_spot(true);
        let samples = runtime.app.best_spot_state().samples.clone();
        assert_eq!(samples.len(), 1, "{samples:?}");
        assert_eq!(samples[0].net, NetworkKind::Mobile5gAdv);
        assert_eq!(samples[0].level, 3);
        assert_eq!(samples[0].dbm, Some(-97));
        assert_eq!(samples[0].rtt_ms, Some(40));

        // The same summary again: no second sample.
        runtime.update_best_spot(true);
        assert_eq!(runtime.app.best_spot_state().samples.len(), 1);
    }

    /// The phone summary and the generation that counts it belong together: a
    /// summary that arrives while the daemon reads them is the one that is
    /// sampled, not the older one.
    #[test]
    fn a_summary_that_arrives_between_the_reads_is_the_sampled_one() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("best_spot", true);
        runtime.app.best_spot_start(Instant::now());
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        // The daemon reads the summary first ...
        let (stats, _, _) = runtime.phone_stats.snapshot(Instant::now());
        assert!(stats.is_some(), "the first summary is there");
        // ... and the phone answers again before the sample is taken.
        let mut newer = spot_stats();
        newer.signal = Some(1);
        runtime.phone_stats.record_for_test(newer, Instant::now());

        runtime.update_best_spot(true);

        let samples = runtime.app.best_spot_state().samples.clone();
        assert_eq!(samples.len(), 1, "{samples:?}");
        assert_eq!(
            samples[0].level, 1,
            "the newer summary is the sample: {samples:?}"
        );
    }

    #[test]
    fn no_best_spot_sample_lands_while_the_switch_is_off() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.app.best_spot_start(Instant::now());
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_best_spot(true);
        assert!(runtime.app.best_spot_state().samples.is_empty());
    }

    /// The session lives on after its two minutes: its marks and its
    /// verdict stay on the screen, so Stop has to stay available.
    #[test]
    fn the_best_spot_session_shows_its_marks_after_the_two_minutes() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.app.best_spot_start(Instant::now());
        // The two minutes are over: the session does not measure any more.
        runtime.app.age_best_spot_session(200_000);

        let snapshot = runtime.snapshot();
        assert!(!snapshot.best_spot_running, "the session is over");
        assert!(
            snapshot.best_spot_active,
            "the session still shows its marks and its verdict"
        );
    }

    #[test]
    fn the_snapshot_carries_the_best_spot_block() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("best_spot", true);
        runtime.app.best_spot_start(Instant::now());
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());
        runtime.update_best_spot(true);

        let snapshot = runtime.snapshot();
        assert!(snapshot.best_spot_running);
        assert!(
            snapshot
                .best_spot_lines
                .first()
                .is_some_and(|line| line.starts_with("Now: ")),
            "{:?}",
            snapshot.best_spot_lines
        );
    }

    /// The good hours take one sample per phone summary, never two.
    #[test]
    fn the_good_hours_sample_lands_once_per_phone_summary() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("good_hours", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_good_hours(ConnectionState::Connected, Some(0));
        assert_eq!(runtime.app.good_hours_records().len(), 1);

        // The same summary again: no second sample.
        runtime.update_good_hours(ConnectionState::Connected, Some(0));
        assert_eq!(runtime.app.good_hours_records().len(), 1);
    }

    #[test]
    fn no_good_hours_sample_lands_while_the_switch_is_off() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_good_hours(ConnectionState::Connected, Some(0));
        assert!(runtime.app.good_hours_records().is_empty());
    }

    #[test]
    fn the_get_and_clear_commands_work_on_the_good_hours() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("good_hours", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());
        runtime.update_good_hours(ConnectionState::Connected, Some(0));

        match runtime.handle(Request::GetGoodHours) {
            Response::GoodHours { lines } => {
                assert!(!lines.is_empty(), "the daemon hands out the lines")
            }
            other => panic!("unexpected response: {other:?}"),
        }

        match runtime.handle(Request::ClearGoodHours) {
            Response::Ok { note } => assert!(note.is_some()),
            other => panic!("unexpected response: {other:?}"),
        }
        assert!(
            runtime.app.good_hours_records().is_empty(),
            "the clear takes the hour in the making too"
        );
    }

    /// The window shows the good hours only while the switch is on.
    #[test]
    fn the_snapshot_carries_the_good_hours_only_while_the_switch_is_on() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());
        runtime.update_good_hours(ConnectionState::Connected, Some(0));

        assert!(
            runtime.snapshot().good_hours_lines.is_empty(),
            "the switch is off"
        );

        runtime.flags.set("good_hours", true);
        runtime.update_good_hours(ConnectionState::Connected, Some(0));
        let lines = runtime.snapshot().good_hours_lines;
        assert!(!lines.is_empty(), "{lines:?}");
    }

    /// The home screen of the window: the status card and the tiles. The tile
    /// home is a standard feature now, so an updated install always has it.
    #[test]
    fn the_snapshot_always_carries_the_home() {
        let (_dir, runtime) = runtime_with_pair(Role::Sharer);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.flags.get("tile_home"), Some(&true));
        assert!(snapshot.home.is_some());
    }

    #[test]
    fn the_snapshot_carries_the_home_with_the_default_slots() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let ids: Vec<&str> = home.tiles.iter().map(|tile| tile.id.as_str()).collect();
        // The default layout is today's screen plus the average tile, one
        // tile per slot, in this order.
        assert_eq!(
            ids,
            vec!["dns", "signal", "phone", "wifi", "range", "today", "drops", "avg"]
        );
        let dns = home
            .tiles
            .iter()
            .find(|tile| tile.id == "dns")
            .expect("the dns slot");
        assert_ne!(
            dns.big.as_ref().map(|big| big.value.as_str()),
            Some("--"),
            "a fresh phone summary fills the dns slot"
        );
    }

    #[test]
    fn a_missing_value_is_a_muted_tile_not_a_hole() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let range = home
            .tiles
            .iter()
            .find(|tile| tile.id == "range")
            .expect("the range slot is in the default layout");
        assert_eq!(range.big.as_ref().map(|big| big.value.as_str()), Some("--"));
        assert_eq!(
            range.facts.as_ref().map(|facts| facts.lines.clone()),
            Some(vec!["No data yet".to_string()])
        );
    }

    #[test]
    fn a_phone_without_the_net_cap_explains_how_to_turn_it_on() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        let mut stats = spot_stats();
        stats.signal = None;
        stats.dbm = None;
        runtime
            .phone_stats
            .record_for_test(stats, Instant::now());

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let signal = home
            .tiles
            .iter()
            .find(|tile| tile.id == "signal")
            .expect("the signal slot is in the default layout");
        assert_eq!(signal.big.as_ref().map(|big| big.value.as_str()), Some("--"));
        assert_eq!(
            signal.facts.as_ref().map(|facts| facts.lines.clone()),
            Some(vec![
                jafnify_core::tiles::SIGNAL_PERMISSION_LAPTOP.to_string()
            ])
        );
    }

    #[test]
    fn an_old_phone_with_signal_data_keeps_its_signal_tile() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        let mut stats = spot_stats();
        stats.caps_net = false;
        stats.signal = Some(3);
        stats.dbm = Some(-95);
        runtime
            .phone_stats
            .record_for_test(stats, Instant::now());

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let signal = home
            .tiles
            .iter()
            .find(|tile| tile.id == "signal")
            .expect("the signal slot is in the default layout");
        assert_eq!(signal.big.as_ref().map(|big| big.value.as_str()), Some("-95"));
        assert!(
            signal
                .facts
                .as_ref()
                .is_some_and(|facts| facts.lines.iter().all(|line| line != "Turn on Network details on the phone")),
            "the hint never replaces a real reading"
        );
    }

    #[test]
    fn a_phone_with_the_net_cap_keeps_no_data_when_it_has_no_signal() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        let mut stats = spot_stats();
        stats.caps_net = true;
        stats.signal = None;
        stats.dbm = None;
        runtime
            .phone_stats
            .record_for_test(stats, Instant::now());

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let signal = home
            .tiles
            .iter()
            .find(|tile| tile.id == "signal")
            .expect("the signal slot is in the default layout");
        assert_eq!(
            signal.facts.as_ref().map(|facts| facts.lines.clone()),
            Some(vec!["No data yet".to_string()])
        );
    }

    #[test]
    fn no_phone_stats_keeps_no_data_in_the_signal_slot() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let signal = home
            .tiles
            .iter()
            .find(|tile| tile.id == "signal")
            .expect("the signal slot is in the default layout");
        assert_eq!(
            signal.facts.as_ref().map(|facts| facts.lines.clone()),
            Some(vec!["No data yet".to_string()])
        );
    }

    /// The trend rings follow the link: they are cleared when it goes down.
    #[test]
    fn the_trends_forget_the_samples_when_the_link_goes_down() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_trends(ConnectionState::Connected, Some(0));
        assert!(!runtime.trends.dns().is_empty());

        runtime.update_trends(ConnectionState::Retry, Some(0));
        assert!(runtime.trends.dns().is_empty());
    }

    /// One phone summary fills the trend once, not twice.
    #[test]
    fn one_summary_fills_the_trend_once() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());

        runtime.update_trends(ConnectionState::Connected, Some(0));
        runtime.update_trends(ConnectionState::Connected, Some(0));
        assert_eq!(runtime.trends.dns().len(), 1);

        runtime
            .phone_stats
            .record_for_test(spot_stats(), Instant::now());
        runtime.update_trends(ConnectionState::Connected, Some(0));
        assert_eq!(runtime.trends.dns().len(), 2);
    }

    /// A phone that stopped answering says nothing: its slots fall back to
    /// the muted "--" tile, never to a stale value.
    #[test]
    fn a_stale_phone_summary_shows_a_muted_tile() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("tile_home", true);
        runtime.flags.set("phone_stats", true);
        let old = Instant::now() - Duration::from_secs(FRESH_PHONE_SECS + 5);
        runtime.phone_stats.record_for_test(spot_stats(), old);

        let home = runtime.snapshot().home.expect("the tile home is standard");
        let dns = home
            .tiles
            .iter()
            .find(|tile| tile.id == "dns")
            .expect("the dns slot");
        assert_eq!(dns.big.as_ref().map(|big| big.value.as_str()), Some("--"));
        assert_eq!(
            dns.facts.as_ref().map(|facts| facts.lines.clone()),
            Some(vec!["No data yet".to_string()])
        );
    }

    /// The phone says it limits itself: the warning is the phone's own word.
    #[test]
    fn the_throttling_phone_gets_one_notification() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let mut stats = hot_stats();
        stats.thr = Some(3);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(
            *lines.lock().unwrap(),
            vec!["Phone is throttling because of heat".to_string()]
        );

        // The phone keeps limiting itself: no second line.
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert_eq!(lines.lock().unwrap().len(), 1);
    }

    /// A phone that reports the heat is not warned about by its temperature.
    #[test]
    fn a_phone_that_reports_the_heat_is_not_warned_about_by_the_temperature() {
        let (_dir, mut runtime) = runtime_with_pair(Role::Sharer);
        runtime.flags.set("phone_heat_warning", true);
        let fake = crate::notification::FakeNotifier::new();
        let lines = fake.shared();
        runtime.set_notifier(Box::new(fake));

        let now = Instant::now();
        let mut stats = hot_stats();
        stats.thr = Some(1);
        runtime.update_phone_heat(ConnectionState::Connected, Some(&stats), Some(1), now);
        assert!(lines.lock().unwrap().is_empty());
    }
}
