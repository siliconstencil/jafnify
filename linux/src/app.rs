//! `App` controller: composes jafnify-core primitives (pairing,
//! discovery, handshake, config generation) with the kernel WireGuard
//! backend and pkexec root escalation into one testable state machine.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jafnify_core::config::{InboundGuard, KillSwitch};
use jafnify_core::known::{pair_record, stored_pair, KnownError, KnownTrack};
use jafnify_core::link::RxWatch;
use jafnify_core::pairing::{PairingCode, PairingError};
use jafnify_core::pake::PakeLink;
use jafnify_core::probe::LocalIface;
use jafnify_core::snapshot::{classify_path, Freshness, Health, RetryView};
use jafnify_core::state::Peer;
use jafnify_core::state::Role;
use jafnify_core::timing::{
    LINK_BACK_ANSWER, LIVENESS_DEAD_AFTER, LIVENESS_DEAD_AFTER_NO_PING, LIVENESS_PROBE_IDLE,
    MIN_ROUTE_TOGGLE,
};
use jafnify_core::{
    ConfigGenerator, ConnectionState, Discovery, DiscoveryConfig, Event, Handshake, KeyPair,
    Orchestrator, WireGuardConfig, ANNOUNCE_INTERVAL, OLD_PEER_MESSAGE, RETRY_INTERVAL,
};
use serde::{Deserialize, Serialize};

/// Test seam: uplink state for an interface, replacing the carrier and
/// address read.
pub type LinkStateFn = Arc<dyn Fn(&str) -> Option<bool> + Send + Sync>;

use crate::backend::{WgBackend, WgError};
use crate::connection_log::{self, ConnectionLog};
use crate::diag::{self, Diag};
use crate::drop_store::DropStore;
use crate::exec;
use crate::firewall::{lan_subnet, ufw_enabled, FirewallCheck, FirewallKind, Verdict, UFW_CONF};
use crate::good_hours_store::GoodHoursStore;
use crate::helper::{adoption_match, reuse_decision, HandshakeAge, LiveConfig, Reuse};
use crate::owner::{decide as ownership_decision, OwnershipInputs, TunnelOwner};
use crate::root::RootCommands;
use crate::session::{SessionHistory, SessionRecord, SessionStats, SessionTracker};
use crate::settings::{ensure_private_dir, save_state, SettingsError, WindowGeometry};
use crate::theme::ThemeMode;
use crate::ui_text::{FOREIGN_TUNNEL, LINK_LOST, TEARDOWN_DENIED};
use crate::uplink::{
    parse_operstate, parse_route_src, parse_uplink_route, read_uplink_state_with, LinkChange,
    LinkWatch,
};
use crate::usage_store::UsageStore;
use jafnify_core::best_spot::{self, Mark, Sample};
use jafnify_core::drop_history::{self, DropRecord, PhoneContext};
use jafnify_core::good_hours::{self, HourRecord};
use jafnify_core::phone_exit::{self, ExitReason};
use jafnify_core::phone_stats::NetworkKind;
use jafnify_core::usage::{UsageSample, UsageSampler, UsageSummary, UsageWindow};
use jafnify_core::wifi_leg::WifiLeg;

pub use jafnify_core::{CONNECT_TIMEOUT, RX_STALL_TIMEOUT};

/// How old a phone summary may be for it to say something about the phone
/// now. The heat warning, the good-time notice and the good-hours sampling
/// share the rule.
pub const FRESH_PHONE_SECS: u64 = 30;

/// The tunnel we brought up, so a later process can adopt it without a
/// privileged status call. It holds public config fields only.
#[derive(Serialize, Deserialize)]
struct TunnelMarker {
    peer_public_key: String,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    allowed_ips: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    kill_switch: bool,
}

/// After this long, start() auto-accepts the local code (mutual pairing).
const AUTO_ACCEPT_DELAY: Duration = Duration::from_secs(3);
/// WireGuard interface name on Linux.
pub const IFACE: &str = "jafn0";
/// Unprivileged rx counter of the tunnel interface (`wg show` needs root).
pub const RX_BYTES_PATH: &str = "/sys/class/net/jafn0/statistics/rx_bytes";
/// Unprivileged tx counter of the tunnel interface.
pub const TX_BYTES_PATH: &str = "/sys/class/net/jafn0/statistics/tx_bytes";
/// How often to check the shared main-table suppress rule while jafn0 exists.
const SUPPRESS_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Back off after a failed repair instead of prompting every 2 s.
const SUPPRESS_RETRY: Duration = Duration::from_secs(30);
/// While Connected, a flat rx counter for this long sends one known-peer probe.
pub const RESET_PROBE_AFTER: Duration = Duration::from_secs(5);
/// A verified Proof from a new peer nonce starts this window: the display is
/// neutral ("Reconnecting") and the peer must come back with data before it
/// closes, or the peer session is treated as reset. Twelve seconds is the
/// display's own neutral window and covers the real install gap of an app
/// update (ten seconds on a phone), so a quick update is never shown as a
/// lost connection. A peer that does not come back inside it is a drop.
pub const RESET_NEUTRAL_GRACE: Duration = Duration::from_secs(12);
/// Retry delay after an evidence-based peer-session reset.
pub const RETRY_AFTER_RESET: Duration = Duration::from_secs(2);
/// While Connected, no authenticated peer activity for this long makes the
/// display neutral ("Reconnecting"). The known track sends a Hello every 5 s,
/// so a live peer answers a Proof well inside this window. It never changes
/// the orchestrator state, so it is not a drop.
pub const FRESH_GAP: Duration = Duration::from_secs(12);
/// The neutral display word while Connected carries no authenticated data.
pub const RECONNECTING: &str = "Reconnecting...";
/// How often the Connected uplink is sampled (carrier + address).
pub const LINK_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// How often the read-only handshake age is read through the helper.
pub const HANDSHAKE_POLL_INTERVAL: Duration = Duration::from_secs(5);

struct SuppressJob {
    child: Child,
}

struct SessionPlan {
    priv_key: String,
    peer_pub: String,
    guard: InboundGuard,
    resolved: bool,
    kill: Option<KillSwitch>,
    cfg: WireGuardConfig,
    desired: LiveConfig,
}

/// The uplink facts written to the diagnostic journal's STATS record. Every
/// field is optional: an unavailable value is reported as `-1`, never guessed.
#[derive(Debug, Clone, Default)]
struct EgressSnapshot {
    iface: Option<String>,
    src: Option<IpAddr>,
    carrier: Option<bool>,
    operstate: Option<String>,
}

/// The best-spot session of the laptop: the samples of the phone and the
/// spots the user marked. It lives in memory only: nothing is written to the
/// disk and nothing leaves the two devices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BestSpotState {
    /// When the session started, in epoch milliseconds.
    pub started_at: Option<i64>,
    /// The samples of the phone, oldest first.
    pub samples: Vec<Sample>,
    /// The marked spots, oldest first.
    pub marks: Vec<Mark>,
}

impl Drop for SuppressJob {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid pairing code")]
    InvalidCode,
    #[error("no LAN interface found")]
    NoLanInterface,
    #[error("no paired device - use jafnify pair")]
    NoPairedDevice,
    #[error("known-peer error: {0}")]
    Known(#[from] KnownError),
    #[error("pairing error: {0}")]
    Pairing(#[from] PairingError),
    #[error("discovery error: {0}")]
    Discovery(#[from] jafnify_core::discovery::DiscoveryError),
    #[error("pake error: {0}")]
    Pake(#[from] jafnify_core::pake::PakeError),
    #[error("settings error: {0}")]
    Settings(#[from] SettingsError),
    #[error("root command failed: {0}")]
    Root(String),
    #[error("permission was denied")]
    PermissionDenied,
    #[error("another Jafnify connection is active")]
    ForeignTunnel,
    #[error("firewall rule not added: {0}")]
    FirewallRule(String),
    #[error("wireguard backend error: {0}")]
    Wg(#[from] WgError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    PeerFound,
    NoPeerFound,
    Connected,
    Disconnected {
        reason: String,
    },
    RootError(String),
    /// The password dialog was refused or dismissed. The app stopped; it only
    /// asks again when the user does (`App::try_again`).
    PermissionDenied,
}

/// The phone state and the Wi-Fi leg of one moment, pushed by the daemon.
/// The drop history uses it as the context of the drop.
#[derive(Debug, Clone, Copy)]
pub struct PhoneContextSample {
    /// The phone state of that moment.
    pub context: PhoneContext,
    /// How old the phone summary was when the sample was taken.
    pub age_secs: u64,
    /// When the sample was taken.
    pub at: Instant,
    /// The Wi-Fi leg of that moment.
    pub wifi: Option<WifiLeg>,
    /// The WireGuard loss the phone measured, in percent.
    pub wg_loss_pct: Option<u8>,
    /// The handshake round trip the phone measured, in milliseconds.
    pub wg_rtt_ms: Option<u32>,
}

/// An outage in progress: the link is down since this moment. The drop
/// history keeps it until the link is back.
struct OpenDrop {
    at_ms: i64,
    /// The fixed reason word of the outage.
    reason: &'static str,
    /// What the outage turned out to be: a fault of the link, an absence of
    /// the phone or a stop on the phone.
    kind: drop_history::EventKind,
    /// Whether the carrier to the hotspot was down when the outage began.
    /// An outage without a network is an absence of the phone and not a
    /// fault of the link.
    carrier_down: bool,
}

/// An outage that closed on a reason the phone can still explain: the phone's
/// exit reason arrives with the first summary of the new session, after the
/// back line. The annotation has to find that exact outage.
struct Recovery {
    /// The start of the outage, as the drop file records it.
    at_ms: i64,
    /// When the peer came back, for a drop recorded after the fact.
    back_ms: i64,
    /// When the phone's word stops describing this outage.
    until: Instant,
    /// Whether a drop line was already written for this outage. A quick
    /// reset is neutral until the phone's word says otherwise.
    recorded_drop: bool,
}

/// What the phone's exit word did to the outage it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteExit {
    /// No outage was waiting for the word (too late, or another outage began).
    Unused,
    /// An app update: the outage is neutral and not counted.
    Updated,
    /// The user stopped the app: the outage is a stop, not a fault.
    Stopped,
    /// A crash or a system kill of an outage that was still neutral: the drop
    /// is written now, so the desktop must hear about it.
    FaultNow,
    /// A crash or a system kill annotated an outage that was already a
    /// recorded drop: no second notification.
    Annotated,
}

impl NoteExit {
    /// Whether the word was used for an outage.
    pub fn used(self) -> bool {
        self != NoteExit::Unused
    }
}

/// How long after the reconnect the phone may still explain the outage. The
/// first summary of the new session is polled within seconds.
/// The Wi-Fi levels of the kernel; readable without root.
const PROC_WIRELESS_PATH: &str = "/proc/net/wireless";
/// How often the Wi-Fi level is read while Connected.
const RANGE_POLL_INTERVAL: Duration = Duration::from_secs(1);

const EXIT_GRACE: Duration = Duration::from_secs(60);

/// How often the live session's figures are written while it runs. The write
/// rides on the existing sample tick; no other timer is added.
const LIVE_SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// A session a user Stop ended, with the moment and the device it belonged to.
#[derive(Debug, Clone)]
struct ResumeCandidate {
    tracker: SessionTracker,
    stopped_at_ms: i64,
    peer_key: String,
}

pub struct App {
    orchestrator: Orchestrator,
    discovery: Option<Discovery>,
    /// Legacy 0x02/0x04 detection only; the new pairing path is `pake`.
    handshake: Option<Handshake>,
    /// SPAKE2 pairing for the code track (0x05/0x06).
    pake: Option<PakeLink>,
    /// Known-peer track (0x03 Hello/Proof); replaces discovery/handshake.
    known: Option<KnownTrack>,
    /// An existing tunnel is being watched for its first peer data before the
    /// app may call it Connected. `known` is None on this path.
    adopting: bool,
    /// A marker may not be reused until an inspect succeeds or a normal up
    /// rewrites the tunnel. Set when inspect fails, so a stale marker cannot
    /// hide an unreadable config.
    marker_reuse_blocked: bool,
    /// The peer's WireGuard state is new (it said Bye, or announced a Fresh
    /// session while this side was not connected): a kept jafn0 holds stale
    /// session keys, so the next bring-up must not reuse it.
    peer_wg_fresh: bool,
    /// The current jafn0 was reused, not set up for this connection.
    tunnel_reused: bool,
    /// Since when the known-peer sends fail because this computer has no
    /// usable network (Wi-Fi or hotspot gone). Searching waits quietly in
    /// this state instead of cycling through Retry every few seconds.
    no_network_since: Option<Instant>,
    /// Test seam: known-peer sends fail with this kind.
    known_send_fault: Option<std::io::ErrorKind>,
    /// While waiting for the network: when the known track was last opened
    /// again on the current LAN addresses (the old one may be pinned to an
    /// address the returning network no longer gives this computer).
    no_network_refresh_at: Option<Instant>,
    /// Asks NetworkManager to scan while the joiner waits for the network.
    wifi_scan: crate::wifi_scan::WifiScanNudge,
    /// This process brought `jafn0` up, or adopted it after proving it ours.
    /// Every privileged path on the tunnel is gated on this or on `decide`.
    owns_tunnel: bool,
    /// `jafn0` exists and belongs to another instance or pair. The card says
    /// so and the app does not connect until it is gone.
    foreign_tunnel: bool,
    /// The live peer key read once for the ownership decision; `None` means
    /// `inspect` could not read it.
    live_peer_key: Option<String>,
    /// A host-wide systemd-resolved override, read once at build (read-only).
    host_dns_override: Option<String>,
    /// True when name lookups here can go through the tunnel: the joiner
    /// side with systemd-resolved present. Read once at build (read-only).
    resolved_present: bool,
    live_peer_key_checked: bool,
    /// Searches (and retries) run on the known track: set by `start_known` and
    /// once a code pairing has been persisted.
    use_known: bool,
    keypair: Option<KeyPair>,
    local_code: Option<PairingCode>,
    settings: crate::settings::Settings,
    state: crate::settings::LinuxStateFile,
    config_dir: PathBuf,
    root: RootCommands,
    /// Injected `wg` for tests; the card's counters use sysfs, so the app
    /// itself no longer runs `wg show` (the CLI status command does).
    #[allow(dead_code)]
    backend: WgBackend,
    /// User override; None = detect the LAN interface at start().
    iface_pin: Option<std::net::Ipv4Addr>,
    ip_bin: PathBuf,
    /// Ask polkit whether this process may run the tunnel action.
    pkcheck_bin: PathBuf,
    peer_public_key: Option<String>,
    peer_endpoint: Option<SocketAddr>,
    last_announce: Option<Instant>,
    last_error: Option<String>,
    auto_accept_deadline: Option<Instant>,
    auto_accept_delay: Duration,
    rx_bytes_path: PathBuf,
    tx_bytes_path: PathBuf,
    rx_watch: Option<RxWatch>,
    /// Where the readings of the Wi-Fi link come from; injected for tests.
    station: Box<dyn crate::station::StationSource>,
    /// The readings of the last minutes: the evidence of a drop.
    station_ring: jafnify_core::wifi_range::StationRing,
    /// When the last reading went to the local link log.
    last_link_log_ms: Option<i64>,
    /// The Wi-Fi level the laptop hears from the hotspot, one sample a
    /// second while Connected: the range tile and the evidence of a drop.
    range_trend: jafnify_core::tiles::TrendBuffer,
    /// When the level was last read.
    last_range_sample: Option<Instant>,
    /// The uplink interface carrying the peer route, resolved once per
    /// Connected episode.
    link_watch: Option<LinkWatch>,
    /// The resolved uplink was last seen without carrier or an IPv4 address.
    link_lost: bool,
    /// When the uplink was last sampled; throttles `link_poll`.
    last_link_sample: Option<Instant>,
    /// Kill switch OFF: when not Connected, the normal route must win after
    /// this instant. `jafn0` stays for a fast return.
    fallback_at: Option<Instant>,
    /// jafn0's own wg-quick rule was removed right now.
    routing_down: bool,
    /// Retry a failed restore at this instant while Connected.
    restore_at: Option<Instant>,
    /// Liveness: the tunnel's rx counter as last seen, for "how long silent".
    live_watch: Option<RxWatch>,
    /// When the last liveness probe went into jafn0.
    last_probe: Option<Instant>,
    /// Unprivileged ping is not allowed here: probes are plain UDP and the
    /// dead threshold waits for WireGuard's passive keepalive.
    probe_no_ping: bool,
    /// When the routing last changed (fallback or restore).
    last_route_toggle: Option<Instant>,
    /// The peer's data arrived after the last routing change.
    answered_since_toggle: bool,
    /// The uplink came back at this instant and the peer has not answered
    /// since (cleared by the first rx).
    link_back_at: Option<Instant>,
    /// The system's rule list was read once in this Connected period. The
    /// in-memory `routing_down` does not survive a restart, so a rule left by
    /// another process (ExecStopPost, a killed daemon) is found here.
    fallback_rule_checked: bool,
    /// Test override: the uplink interface name (skips the route lookup).
    link_iface: Option<String>,
    /// Test override: uplink state, replacing carrier + address read.
    link_state: Option<LinkStateFn>,
    /// Production sysfs root for the carrier file (`/sys/class/net`).
    uplink_sysfs: PathBuf,
    /// A valid Proof reported a peer nonce outside its known set: the peer's
    /// known-peer session may have restarted. Data inside the grace window
    /// clears it; no data by `reset_deadline` drops the connection.
    peer_reset_hint: bool,
    reset_deadline: Option<Instant>,
    /// A peer session reset is in its neutral window: the display says
    /// Reconnecting and no drop is recorded until the peer comes back or the
    /// window closes.
    reset_neutral: bool,
    /// The start of the neutral reset outage, in session epoch milliseconds.
    reset_neutral_at: Option<i64>,
    /// One explicit Hello probe per flat-rx episode.
    reset_probe_sent: bool,
    /// LAN interfaces of the current search, reused when a retry starts over.
    search_ifaces: Vec<std::net::Ipv4Addr>,
    /// When a searching known track last compared its pinned addresses with
    /// the ones this computer has now. A network that came back may have a new
    /// address; the search must follow it without a restart.
    search_recheck_at: Option<Instant>,
    connecting_since: Option<Instant>,
    /// One diagnostic record per Connecting attempt when a wrong-code Confirm
    /// arrives. It never stops the search.
    pake_wrong_code_logged: bool,
    /// A polkit prompt was refused; set until the user starts again.
    denied: bool,
    /// When the tunnel became Connected (seen by a tick); None otherwise.
    connected_since: Option<Instant>,
    /// The displayed connected clock. The phone is the authority: its elapsed
    /// value from each summary reply is adopted, with the local start as the
    /// fallback for an old phone or before the first reply.
    conn_clock: jafnify_core::connclock::ConnectedClock,
    /// The monotonic instant the clock's milliseconds count from.
    clock_origin: Instant,
    /// The last authenticated Proof from the peer (the known-track
    /// heartbeat). A gap makes the display neutral without a drop.
    last_peer_proof: Option<Instant>,
    /// Live session statistics, fed from the same state and counters.
    session: SessionTracker,
    /// Bounded, redacted connection log shown, copied and saved by the GUI.
    connection_log: ConnectionLog,
    /// Last readable `(sent, received)` counters. Reused when the interface
    /// disappears, so a drop still has a counter baseline to close.
    last_traffic: Option<(u64, u64)>,
    /// Completed sessions loaded from `sessions_path`, newest first, plus the
    /// live session's last saved figures.
    session_history: SessionHistory,
    /// Injected for tests; production uses `<config_dir>/sessions.toml`.
    sessions_path: PathBuf,
    /// Only the tray daemon owns `sessions_path`; a CLI App never writes it.
    session_history_owner: bool,
    /// When the live session was last written, to keep the periodic save
    /// bounded. The write rides on `sample_session`.
    last_live_save: Option<Instant>,
    /// The session a user Stop ended, kept a short while so a quick Connect
    /// to the same device continues it (`scope::resumes`). Memory only: a
    /// restart of the daemon starts a new session.
    resume_candidate: Option<ResumeCandidate>,
    /// True while `usage_statistics` is on; the sampler runs only then.
    usage_on: bool,
    /// The laptop's hourly usage, fed from the same counters as the session.
    usage_sampler: UsageSampler,
    /// Completed hours; production uses `<config_dir>/usage.tsv`.
    usage_store: UsageStore,
    /// True while `drop_history` is on; the drops are recorded only then.
    drop_history_on: bool,
    /// The drop history of the file; production uses `<config_dir>/drops.tsv`.
    drop_store: DropStore,
    /// The phone state of the last moment, for the context of a drop.
    phone_context: Option<PhoneContextSample>,
    /// The outage in progress, if any. The drop history owns it, not the
    /// session: a lost link is recorded even when the counters are unreadable.
    drop_open: Option<OpenDrop>,
    /// The outage that just closed and may still be explained by the phone's
    /// exit reason in the first summary of the new session.
    recovery: Option<Recovery>,
    /// The drop the connection log has to close. It is set where the drop
    /// line is written and cleared by the line that closes it.
    logged_drop: Option<i64>,
    /// True while the phone has said goodbye: the outage is a stop on the
    /// phone and not a fault of the link.
    peer_stopped: bool,
    /// The fixed reason word of the outage in progress, when there is one.
    /// It is set even when the drop history is off: the window asks it to
    /// know whether the phone was the reason.
    outage_reason: Option<&'static str>,
    /// True while the return of the phone has to be quiet: the last outage
    /// was an absence of the phone. The flag is taken once, so the next
    /// outage can be announced.
    away_quiet: bool,
    /// The scope of each tile that sums over time: tile id to scope word.
    /// The file is `<config_dir>/tile-scopes.toml`.
    tile_scopes: BTreeMap<String, String>,
    /// The home layout: the grid, the wide slot and the tile of each slot.
    /// The file is `<config_dir>/home-layout.toml`.
    home_layout: jafnify_core::home_layout::Layout,
    /// The best-spot session: the samples of the phone and the marked spots.
    /// In memory only.
    best_spot: BestSpotState,
    /// True while the good-hours switch is on; the samples are taken only
    /// then.
    good_hours_on: bool,
    /// True while the link is up and the phone summary is fresh enough to
    /// say something about the phone now.
    good_hours_ready: bool,
    /// The hour of the phone in the making. In memory only: a crash loses at
    /// most this hour.
    good_hours_sampler: good_hours::Sampler,
    /// The finished hours; production uses `<config_dir>/good_hours.tsv`.
    good_hours_store: GoodHoursStore,
    /// Base for turning monotonic tick times into wall-clock epoch ms.
    session_epoch: Instant,
    session_epoch_ms_base: i64,
    /// Listens for our own announcements to tell a blocked search from a
    /// quiet one (see `firewall`).
    firewall: Option<FirewallCheck>,
    firewall_verdict: Verdict,
    /// LAN interface the allow rule would be scoped to, found once a block shows.
    firewall_iface: Option<String>,
    ufw_conf: PathBuf,
    /// Optional event log (production); tests leave it off.
    logger: Option<crate::applog::Logger>,
    /// When the last disconnect happened, for the reconnect duration log.
    disconnected_at: Option<Instant>,
    /// Marker naming the peer of the tunnel this app brought up.
    marker_path: PathBuf,
    /// True while the current connection attempt expects a password prompt.
    /// Armed when the attempt starts, refreshed around each tunnel call, and
    /// cleared once it is no longer true. Published as the view's polkit note;
    /// never inferred from the connection state.
    polkit_prompt_expected: bool,
    /// The installed wg-quick config (tests use a temp path).
    installed_conf_path: PathBuf,
    /// Rx counter when the current bring-up/reconnect started; Connected needs
    /// a later counter value, not merely a non-zero one.
    reconnect_rx: Option<u64>,
    /// A non-blocking helper `suppress` call, if one is running.
    suppress_job: Option<SuppressJob>,
    /// Last unprivileged `ip rule show` check.
    last_suppress_check: Option<Instant>,
    /// Do not retry a failed helper repair before this time.
    suppress_retry_at: Option<Instant>,
    /// A missing suppress rule was repaired; say so once the next unprivileged
    /// check sees it present.
    suppress_restore_pending: bool,
    /// Diagnostic journal (production); tests use a temp path or None.
    diag: Option<Diag>,
    /// When the tunnel's rx counter last advanced (peer data still flowing).
    last_peer_rx: Option<Instant>,
    /// The liveness watchdog says the tunnel is dead: used as the neutral
    /// signal on a code/adopted tunnel, which has no known-track heartbeat.
    tunnel_dead: bool,
    /// When the last diag STATS record was written.
    last_diag_stats: Option<Instant>,
    /// Last observed tunnel rx counter, for the diag cadence only.
    last_diag_rx: Option<u64>,
    /// The peer's newest handshake age, read through the read-only helper.
    handshake_age: HandshakeAge,
    /// A helper call in flight, so the tick never blocks on `pkexec`.
    handshake_job: Option<std::sync::mpsc::Receiver<HandshakeAge>>,
    /// When the handshake age was last asked for; throttles the root call.
    last_handshake_query: Option<Instant>,
    /// A window is showing the state. The handshake age is display-only, so
    /// the root helper is asked only while someone can see the answer.
    viewer_open: bool,
    /// Current uplink facts for the diag STATS record.
    diag_egress: EgressSnapshot,
}

impl App {
    pub fn new(config_dir: PathBuf) -> Self {
        AppBuilder::new(config_dir).build()
    }

    pub fn state(&self) -> ConnectionState {
        self.orchestrator.state()
    }

    pub fn role(&self) -> Role {
        self.settings.role
    }

    pub fn autostart(&self) -> bool {
        self.settings.autostart
    }

    pub fn set_autostart(&mut self, enabled: bool) {
        self.settings.autostart = enabled;
    }

    pub fn peers(&self) -> &[Peer] {
        &self.state.peers
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// A device is paired: its key is stored and `start_known` can reconnect.
    pub fn is_paired(&self) -> bool {
        stored_pair(&self.state.peers).is_some()
    }

    /// A valid Proof from the peer carried a nonce outside its current and
    /// previous nonce; the connection is waiting for data in the grace window.
    pub fn peer_reset_pending(&self) -> bool {
        self.peer_reset_hint
    }

    /// While Connected, the uplink carrying the peer route is down. The
    /// connection state itself stays Connected.
    pub fn link_lost(&self) -> bool {
        self.link_lost
    }

    /// English status line for the GUI/CLI.
    pub fn status_text(&self) -> String {
        self.status_text_at(Instant::now())
    }

    /// `status_text` with the caller's clock, so tests with an injected clock
    /// see the same countdown they pass to `tick_at`.
    pub fn status_text_at(&self, now: Instant) -> String {
        match self.state() {
            ConnectionState::Idle if self.foreign_tunnel => FOREIGN_TUNNEL.to_string(),
            ConnectionState::Idle => "Not sharing".to_string(),
            ConnectionState::Searching => "Searching for the other device".to_string(),
            ConnectionState::Pairing => "Pairing".to_string(),
            ConnectionState::Connecting => "Connecting".to_string(),
            ConnectionState::Connected if self.link_lost => LINK_LOST.to_string(),
            ConnectionState::Connected if self.data_stale(now) => RECONNECTING.to_string(),
            ConnectionState::Connected => "Connected".to_string(),
            ConnectionState::Retry => {
                let reason = self
                    .orchestrator
                    .last_disconnect_reason()
                    .map(str::to_string);
                match self.retry_secs_left_at(now) {
                    Some(remaining_secs) => RetryView {
                        remaining_secs,
                        reason,
                    }
                    .line(),
                    None => "Connection lost - retrying".to_string(),
                }
            }
        }
    }

    /// Whole seconds until the retry search starts, rounded up; None outside
    /// Retry. Uses the orchestrator's own deadline, so a reset's 2 s retry is
    /// shown as 2 s, not the ordinary 10 s.
    pub fn retry_secs_left_at(&self, now: Instant) -> Option<u64> {
        self.orchestrator
            .retry_remaining_at(now)
            .map(|left| left.as_secs() + u64::from(left.subsec_nanos() > 0))
    }

    pub fn retry_secs_left(&self) -> Option<u64> {
        self.retry_secs_left_at(Instant::now())
    }

    /// The local pairing code outlived its lifetime before pairing completed.
    /// Linux has no countdown: the card only says so once the track expires.
    pub fn code_expired_at(&self, now: Instant) -> bool {
        matches!(
            self.state(),
            ConnectionState::Searching | ConnectionState::Pairing | ConnectionState::Connecting
        ) && self.orchestrator.code_expired_at(now)
    }

    pub fn code_expired(&self) -> bool {
        self.code_expired_at(Instant::now())
    }

    /// The card's health line, built from the latest handshake age and the
    /// peer's freshness. The wording lives in jafnify-core.
    pub fn health_at(&self, now: Instant) -> Health {
        let handshake = match self.handshake_age {
            HandshakeAge::Seconds(seconds) => Freshness::Age(seconds),
            HandshakeAge::Never => Freshness::Never,
            HandshakeAge::Unknown => Freshness::Unknown,
        };
        let data_age = (self.state() == ConnectionState::Connected)
            .then(|| {
                self.last_peer_rx
                    .map(|at| now.saturating_duration_since(at).as_secs())
            })
            .flatten();
        Health {
            handshake,
            path: classify_path(handshake, data_age),
            old_peer: self.orchestrator.old_peer_seen(),
        }
    }

    pub fn health(&self) -> Health {
        self.health_at(Instant::now())
    }

    pub fn health_line(&self) -> Option<String> {
        self.health().line()
    }

    /// The verdict on the main card: no moving handshake age.
    pub fn health_verdict(&self) -> Option<String> {
        self.health().verdict()
    }

    /// The handshake age for the Details view.
    pub fn handshake_detail(&self) -> Option<String> {
        self.health().handshake_detail()
    }

    pub fn set_role(&mut self, role: Role) {
        self.settings.role = role;
    }

    /// Display name of the paired device ("Phone - Blue Falcon").
    pub fn paired_name(&self) -> Option<String> {
        stored_pair(&self.state.peers).map(|p| p.name.clone())
    }

    /// The live peer endpoint, as the tunnel sees it ("1.2.3.4:9797"). None
    /// while no peer is known. A hint for the diagnostics export only.
    pub fn peer_endpoint(&self) -> Option<String> {
        self.peer_endpoint.map(|e| e.to_string())
    }

    /// True when name lookups on this computer go through the tunnel: the
    /// joiner side with systemd-resolved present. Read-only.
    pub fn dns_through_tunnel(&self) -> bool {
        self.settings.role == Role::Joiner && self.resolved_present
    }

    /// Unix seconds of the paired device's last connection.
    pub fn last_connected(&self) -> Option<u64> {
        stored_pair(&self.state.peers).and_then(|p| p.last_connected)
    }

    /// The password dialog was refused and nothing has been started since.
    pub fn denied(&self) -> bool {
        self.denied
    }

    /// The current connection attempt will ask for a password (the installed
    /// rule does not cover this user). Visible during Searching/Connecting
    /// before the call, then cleared once the attempt no longer needs it.
    pub fn polkit_prompt_expected(&self) -> bool {
        self.polkit_prompt_expected
    }

    /// Start again after a denial, on the track that was refused (the paired
    /// device, or the same code). Does nothing when nothing was denied.
    pub fn try_again(&mut self) -> Result<(), AppError> {
        if !self.denied {
            return Ok(());
        }
        if self.use_known {
            self.start_known()
        } else if let Some(code) = self.local_code.clone() {
            self.start(code.as_str())
        } else {
            self.denied = false;
            Ok(())
        }
    }

    /// How long the tunnel has been Connected, as of `now`: the phone's value
    /// while it is fresh, else this side's own first-data start.
    pub fn connected_secs_at(&self, now: Instant) -> Option<u64> {
        self.conn_clock.elapsed_secs(self.clock_ms(now))
    }

    pub fn connected_secs(&self) -> Option<u64> {
        self.connected_secs_at(Instant::now())
    }

    /// (sent, received) bytes of the tunnel interface, as the laptop sees them.
    pub fn traffic(&self) -> Option<(u64, u64)> {
        let sent = read_counter(&self.tx_bytes_path)?;
        let received = read_counter(&self.rx_bytes_path)?;
        Some((sent, received))
    }

    /// Statistics for the session that is still open.
    pub fn current_session(&self) -> SessionStats {
        self.session.current()
    }

    /// Completed sessions, newest first.
    pub fn session_history(&self) -> Vec<SessionRecord> {
        self.session_history.recent(crate::session::MAX_SESSIONS)
    }

    /// Turn local usage sampling on or off. Turning it off flushes the current
    /// hour once; turning it on never writes anything until the first sample.
    pub fn set_usage_enabled(&mut self, enabled: bool) {
        if enabled == self.usage_on {
            return;
        }
        if !enabled {
            self.flush_usage();
        }
        self.usage_on = enabled;
    }

    /// Store the partial hour and start a fresh baseline. Safe to call when
    /// usage is off or empty.
    pub fn flush_usage(&mut self) {
        if let Some(current) = self.usage_sampler.current().copied() {
            self.usage_store.push(current);
        }
        self.usage_sampler = UsageSampler::new();
    }

    /// The three local summaries over the stored hours plus the partial one.
    pub fn usage_summaries(&self, now: Instant, utc_offset_minutes: i32) -> [UsageSummary; 3] {
        let mut buckets = self.usage_store.buckets().to_vec();
        if let Some(current) = self.usage_sampler.current().copied() {
            jafnify_core::usage::push_bucket(&mut buckets, current);
        }
        let now_ms = self.session_epoch_ms_at(now);
        UsageWindow::ALL
            .map(|window| UsageSummary::build(&buckets, window, now_ms, utc_offset_minutes))
    }

    /// Structured connection log: the newest 500 lines, oldest first.
    pub fn connection_log(&self) -> Vec<String> {
        self.connection_log.tail(connection_log::MAX_LINES)
    }

    /// Turn the drop history on or off. A new drop is only written while the
    /// switch is on. A drop that was written is always closed with its back
    /// line, also when the switch went off in the meantime; turning the
    /// switch off keeps what was recorded.
    pub fn set_drop_history_enabled(&mut self, enabled: bool) {
        self.drop_history_on = enabled;
    }

    /// The phone state of this moment, for the context of the next drop.
    pub fn set_phone_context(&mut self, sample: Option<PhoneContextSample>) {
        self.phone_context = sample;
    }

    /// The drops of the file, oldest first.
    pub fn drop_records(&self) -> Vec<DropRecord> {
        self.drop_store.records()
    }

    /// The events of the file, oldest first: the faults of the link, the
    /// absences of the phone and the stops on the phone.
    pub fn drop_events(&self) -> Vec<drop_history::Event> {
        self.drop_store.events()
    }

    /// The numbers of the sharing that is running, for the session scope of
    /// a tile. A reconnect inside the same sharing and an absence of the
    /// phone do not end it.
    pub fn scope_session(&self) -> Option<jafnify_core::scope::ScopeSession> {
        let current = self.session.current();
        if current.started_at == 0 {
            return None;
        }
        Some(jafnify_core::scope::ScopeSession {
            started_at_ms: current.started_at,
            up_bytes: current.up_bytes,
            down_bytes: current.down_bytes,
            peak_down_bps: current.peak_down_bps,
        })
    }

    /// The scope of one tile: the stored choice, or the core default when
    /// the tile has none.
    pub fn tile_scope(&self, tile_id: &str) -> jafnify_core::scope::TileScope {
        jafnify_core::scope::TileScope::for_tile(
            tile_id,
            self.tile_scopes.get(tile_id).map(String::as_str),
        )
    }

    /// Whether the phone cannot be reached: the hotspot of the phone went
    /// away or this computer has no usable network. Such an outage is an
    /// absence, not a fault, and it gets no alarm: the laptop waits for the
    /// phone and comes back on its own.
    pub fn phone_unreachable(&self) -> bool {
        self.outage_reason == Some(drop_history::REASON_NO_NETWORK)
    }

    /// Whether the return of the phone has to be quiet: the last outage was
    /// an absence of the phone. The flag is taken once, so the next outage
    /// can be announced again.
    pub fn take_away_quiet(&mut self) -> bool {
        std::mem::take(&mut self.away_quiet)
    }

    /// The reason of the outage in progress, for the tests of the daemon.
    #[cfg(test)]
    pub fn set_outage_reason(&mut self, reason: Option<&'static str>) {
        self.outage_reason = reason;
    }

    /// Mark the return of the phone as quiet, for the tests of the daemon.
    #[cfg(test)]
    pub fn set_away_quiet(&mut self, quiet: bool) {
        self.away_quiet = quiet;
    }

    /// Store the scope of one tile. The core owns the words; the window only
    /// passes the choice in.
    pub fn set_tile_scope(&mut self, tile_id: &str, scope: &str) {
        self.tile_scopes
            .insert(tile_id.to_string(), scope.to_string());
    }

    /// The stored scope of every tile that has one.
    pub fn tile_scopes(&self) -> BTreeMap<String, String> {
        self.tile_scopes.clone()
    }

    /// Write the scopes to their own file. The scopes are not in `state.toml`:
    /// that file is read with `deny_unknown_fields`, so a new key there would
    /// make an older build set the file aside and start unpaired.
    pub fn save_tile_scopes(&mut self) -> Result<(), std::io::Error> {
        let path = self.config_dir.join(crate::tile_scopes::FILE_NAME);
        crate::tile_scopes::save(&path, &self.tile_scopes)
    }

    /// The home layout, already normalized for the laptop.
    pub fn home_layout(&self) -> jafnify_core::home_layout::Layout {
        self.home_layout
            .clone()
            .normalize(jafnify_core::home_layout::Platform::Linux)
    }

    /// Store a layout, then write it. The core owns the validation: unknown
    /// ids are dropped, duplicates removed and a short list filled from the
    /// default, so a hand-made text can never leave a hole.
    pub fn set_home_layout(
        &mut self,
        layout: jafnify_core::home_layout::Layout,
    ) -> Result<(), std::io::Error> {
        let layout = layout.normalize(jafnify_core::home_layout::Platform::Linux);
        let path = self.config_dir.join(jafnify_core::home_layout::FILE_NAME);
        crate::home_layout::save(&path, &layout)?;
        self.home_layout = layout;
        Ok(())
    }

    /// Store a layout from the saved text. A corrupt text is refused, so the
    /// choice on screen never silently becomes something else.
    pub fn set_home_layout_text(&mut self, text: &str) -> Result<(), String> {
        let layout = jafnify_core::home_layout::Layout::parse(text)
            .ok_or_else(|| "not a home layout".to_string())?;
        self.set_home_layout(layout).map_err(|e| e.to_string())
    }

    /// Delete the drop history file.
    pub fn clear_drop_history(&mut self) {
        self.drop_store.clear();
        // The open outage goes with it: a cleared history gets no back line.
        self.drop_open = None;
        self.recovery = None;
    }

    /// Start a best-spot session: the old samples and marks go away. The
    /// session lives in memory only and lasts two minutes.
    pub fn best_spot_start(&mut self, now: Instant) {
        self.best_spot = BestSpotState {
            started_at: Some(self.session_epoch_ms_at(now)),
            ..BestSpotState::default()
        };
    }

    /// Stop the best-spot session and forget it.
    pub fn best_spot_stop(&mut self) {
        self.best_spot = BestSpotState::default();
    }

    /// Mark the spot the phone sits in right now. A mark without a session
    /// is ignored: there is nothing to compare it with.
    pub fn best_spot_mark(&mut self, now: Instant) {
        if self.best_spot.started_at.is_none() {
            return;
        }
        let at_ms = self.session_epoch_ms_at(now);
        let number = self.best_spot.marks.len() + 1;
        self.best_spot.marks.push(Mark { at_ms, number });
    }

    /// One sample of the phone. A sample outside the session is ignored: the
    /// session lasts two minutes.
    pub fn best_spot_sample(&mut self, sample: Sample) {
        let Some(started_at) = self.best_spot.started_at else {
            return;
        };
        if best_spot::session_over(started_at, sample.at_ms) {
            return;
        }
        self.best_spot.samples.push(sample);
    }

    /// The best-spot session: the samples and the marked spots.
    pub fn best_spot_state(&self) -> &BestSpotState {
        &self.best_spot
    }

    /// True while the session still measures the phone.
    pub fn best_spot_running(&self, now_ms: i64) -> bool {
        match self.best_spot.started_at {
            Some(started_at) => !best_spot::session_over(started_at, now_ms),
            None => false,
        }
    }

    /// True while the session shows its marks and its verdict. The session
    /// lives on after its two minutes: only Stop clears it.
    pub fn best_spot_showing(&self) -> bool {
        self.best_spot.started_at.is_some()
    }

    /// Test seam: age the best-spot session, so a test can see the state
    /// after its two minutes without waiting for them.
    #[doc(hidden)]
    pub fn age_best_spot_session(&mut self, by_ms: i64) {
        if let Some(started_at) = self.best_spot.started_at {
            self.best_spot.started_at = Some(started_at - by_ms);
        }
    }

    /// The gate of the good-hours sampling: the switch, whether the link is
    /// up and how old the phone summary is. A sample outside the gate is
    /// ignored, so nothing is learned while the switch is off, while the
    /// link is down or from a summary that is too old.
    pub fn set_good_hours_gate(&mut self, enabled: bool, connected: bool, age_secs: Option<u64>) {
        self.good_hours_on = enabled;
        self.good_hours_ready =
            connected && age_secs.map(|age| age <= FRESH_PHONE_SECS).unwrap_or(false);
    }

    /// One sample of the phone for the good hours. A sample outside the gate
    /// is ignored. A sample that finishes an hour writes the hour to the
    /// file.
    pub fn good_hours_sample(&mut self, sample: good_hours::Sample) {
        if !self.good_hours_on || !self.good_hours_ready {
            return;
        }
        if let Some(record) = self.good_hours_sampler.push(sample) {
            self.write_good_hours(record);
        }
    }

    /// The hours the learning knows: the finished ones of the file and the
    /// hour in the making.
    pub fn good_hours_records(&self) -> Vec<HourRecord> {
        let mut records = self.good_hours_store.records();
        if let Some(current) = self.good_hours_sampler.current() {
            records.push(current);
        }
        records
    }

    /// The lines of the good-hours section, in the core wording.
    pub fn good_hours_lines(&self, now_ms: i64, utc_offset_minutes: i32) -> Vec<String> {
        let records = self.good_hours_records();
        let summary = good_hours::Summary::build(&records, now_ms, utc_offset_minutes);
        summary.lines(now_ms, utc_offset_minutes)
    }

    /// Delete the good-hours file and forget the hour in the making.
    pub fn clear_good_hours(&mut self) {
        self.good_hours_store.clear();
        self.good_hours_sampler = good_hours::Sampler::new();
    }

    /// Write the hour in the making to the file. The daemon calls this when
    /// it stops, so a clean stop does not lose the hour in the making. The
    /// hour is not over, so it is written as a half hour: it never gives a
    /// day, but it is not lost either.
    pub fn flush_good_hours(&mut self) {
        if let Some(record) = self.good_hours_sampler.current() {
            self.write_good_hours(record);
        }
    }

    /// Write one finished hour to the file.
    fn write_good_hours(&mut self, record: HourRecord) {
        if let Err(e) = self.good_hours_store.append(record.line()) {
            self.log_event("WARN", &format!("could not write the good hours: {e}"));
        }
    }

    /// The context of a drop: the phone state of the last moment, if the
    /// summary is young enough, and the Wi-Fi leg of that moment.
    fn drop_context(&self, now: Instant) -> (Option<PhoneContext>, Option<WifiLeg>) {
        let Some(sample) = &self.phone_context else {
            return (None, None);
        };
        let age = sample.age_secs + now.saturating_duration_since(sample.at).as_secs();
        // The phone state is only worth keeping while it is fresh. The Wi-Fi
        // leg is measured on its own: it says how the link behaved, also
        // when the phone stopped answering.
        let phone = if age <= drop_history::CONTEXT_MAX_AGE_SECS {
            Some(sample.context)
        } else {
            None
        };
        (phone, sample.wifi)
    }

    /// Write one drop into the history, with the context of that moment. The
    /// reason is a fixed word: the free text can hold an address. Returns the
    /// hint the link evidence gave, so the connection log can name the range.
    fn record_drop(
        &mut self,
        reason: &str,
        disconnected_at_ms: i64,
        now: Instant,
    ) -> Option<&'static str> {
        if !self.drop_history_on {
            return None;
        }
        let (phone, wifi) = self.drop_context(now);
        let mut link = self
            .station_ring
            .evidence(disconnected_at_ms, self.session_epoch_ms_at(now));
        // The phone's own numbers of the link, while its summary is fresh.
        if phone.is_some() {
            if let Some(sample) = &self.phone_context {
                link.phone_loss_pct = sample.wg_loss_pct;
                link.phone_rtt_ms = sample.wg_rtt_ms;
            }
        }
        let record = DropRecord {
            at_ms: disconnected_at_ms,
            reason: drop_history::reason_word(reason).to_string(),
            phone,
            wifi,
            down_ms: None,
            exit: None,
            link,
        };
        let hint = record.hint();
        let written = self.drop_store.append(record.drop_line());
        if let Err(e) = written {
            self.log_event("WARN", &format!("could not write the drop history: {e}"));
        }
        Some(hint)
    }

    /// Write the reconnect that closes the newest open drop.
    fn record_back(&mut self, connected_at_ms: i64, duration_ms: u64) {
        let written = self
            .drop_store
            .append(DropRecord::back_line(connected_at_ms, duration_ms));
        if let Err(e) = written {
            self.log_event("WARN", &format!("could not write the drop history: {e}"));
        }
    }

    /// Open a drop: an established link is lost. The drop history keeps its
    /// own outage, so a lost link is recorded even when the session counters
    /// cannot be read. One outage is one drop, however often the state
    /// changes while the peer is searched for.
    ///
    /// A Bye from the phone is not a fault: the phone stopped sharing, so the
    /// outage is a stop on the phone and never a drop.
    fn open_drop(&mut self, reason: &str, now: Instant) -> Option<&'static str> {
        let at_ms = self.session_epoch_ms_at(now);
        self.open_drop_at(reason, at_ms, now)
    }

    /// Open a drop that began at `at_ms`: a neutral reset records its outage
    /// from the moment the peer went quiet, not from the moment the window
    /// closed and the drop was finally decided.
    fn open_drop_at(&mut self, reason: &str, at_ms: i64, now: Instant) -> Option<&'static str> {
        if !self.drop_history_on || self.drop_open.is_some() {
            return None;
        }
        // A newer outage must never take an exit word meant for an older one.
        self.recovery = None;
        let kind = if self.peer_stopped {
            drop_history::EventKind::Stopped
        } else {
            drop_history::EventKind::Drop
        };
        let hint = if kind == drop_history::EventKind::Stopped {
            let written = self.drop_store.append(DropRecord::stopped_line(at_ms));
            if let Err(e) = written {
                self.log_event("WARN", &format!("could not write the drop history: {e}"));
            }
            None
        } else {
            self.record_drop(reason, at_ms, now)
        };
        self.drop_open = Some(OpenDrop {
            at_ms,
            reason: drop_history::reason_word(reason),
            kind,
            carrier_down: self.link_lost,
        });
        hint
    }

    /// An absence of the phone that lasts longer than the edge is not a
    /// fault: the outage becomes an away event, written once and
    /// reclassifying the drop it belongs to. One outage is one record.
    fn update_away(&mut self, now: Instant) {
        let Some(open) = self.drop_open.as_ref() else {
            return;
        };
        if open.kind != drop_history::EventKind::Drop {
            return;
        }
        // Only the phone being unreachable is an absence. A loss of data or
        // a reset is a fault of the link and stays a drop. The carrier of
        // the hotspot counts too: when it went down, the phone was away.
        if open.reason != drop_history::REASON_NO_NETWORK && !open.carrier_down {
            return;
        }
        let now_ms = self.session_epoch_ms_at(now);
        let down_ms = now_ms.saturating_sub(open.at_ms).max(0) as u64;
        if !drop_history::is_away(down_ms) {
            return;
        }
        let at_ms = open.at_ms;
        let written = self.drop_store.append(DropRecord::away_line(at_ms));
        if let Err(e) = written {
            self.log_event("WARN", &format!("could not write the drop history: {e}"));
        }
        if let Some(open) = self.drop_open.as_mut() {
            open.kind = drop_history::EventKind::Away;
        }
    }

    /// Close the open drop: the link is back.
    fn close_drop(&mut self, now: Instant) {
        self.outage_reason = None;
        let Some(open) = self.drop_open.take() else {
            return;
        };
        let now_ms = self.session_epoch_ms_at(now);
        let down_ms = now_ms.saturating_sub(open.at_ms).max(0) as u64;
        if open.kind == drop_history::EventKind::Away {
            // The absence is over: the log says how long the phone was away,
            // not that the link was lost.
            let line = drop_history::away_words(down_ms);
            self.log_event("INFO", &line);
            self.connection_log.push("away", &line, now_ms);
            // The return of the phone is quiet: the samplers resume silently.
            self.away_quiet = true;
        }
        // The phone can explain some outages only after the reconnect: a
        // session reset or a loss of data is what an app update or a crash
        // looks like from outside. Keep the exact outage for its word.
        if open.kind == drop_history::EventKind::Drop
            && matches!(
                open.reason,
                drop_history::REASON_RESET | drop_history::REASON_NO_DATA
            )
        {
            self.recovery = Some(Recovery {
                at_ms: open.at_ms,
                back_ms: now_ms,
                until: now + EXIT_GRACE,
                recorded_drop: true,
            });
        }
        self.record_back(now_ms, down_ms);
    }

    /// The phone said why its app's previous process ended. An outage the
    /// phone can explain stops being a drop: an update or a user stop is
    /// neutral, a crash or a system kill stays a drop with a truthful hint.
    /// Returns whether the word was used.
    pub fn note_phone_exit(&mut self, reason: ExitReason, now: Instant) -> NoteExit {
        // A summary answer is proof the peer is back: a neutral reset whose
        // data has not advanced yet still gets its word used.
        let recovery = match self.recovery.take() {
            Some(recovery) => recovery,
            None => match self.reset_neutral_at.take() {
                Some(at_ms) => {
                    self.reset_neutral = false;
                    self.reset_deadline = None;
                    self.peer_reset_hint = false;
                    self.reset_probe_sent = false;
                    Recovery {
                        at_ms,
                        back_ms: self.session_epoch_ms_at(now),
                        until: now + EXIT_GRACE,
                        recorded_drop: false,
                    }
                }
                None => return NoteExit::Unused,
            },
        };
        if now > recovery.until {
            return NoteExit::Unused;
        }
        match reason.neutral() {
            Some(phone_exit::Neutral::Updated) => {
                // A quick reset was never a drop: the update line is the one
                // neutral record of the outage. A long outage already has its
                // drop line, which this line reclassifies.
                let line = DropRecord::updated_line(recovery.at_ms);
                if let Err(e) = self.drop_store.append(line) {
                    self.log_event("WARN", &format!("could not write the drop history: {e}"));
                    return NoteExit::Unused;
                }
                self.log_event("INFO", drop_history::UPDATED_LABEL);
                self.connection_log.push(
                    "event",
                    drop_history::UPDATED_LABEL,
                    self.session_epoch_ms_at(now),
                );
                NoteExit::Updated
            }
            Some(phone_exit::Neutral::Stopped) => {
                let line = DropRecord::stopped_line(recovery.at_ms);
                if let Err(e) = self.drop_store.append(line) {
                    self.log_event("WARN", &format!("could not write the drop history: {e}"));
                    return NoteExit::Unused;
                }
                self.log_event("INFO", drop_history::STOPPED_LABEL);
                self.connection_log.push(
                    "event",
                    drop_history::STOPPED_LABEL,
                    self.session_epoch_ms_at(now),
                );
                NoteExit::Stopped
            }
            None => {
                if recovery.recorded_drop {
                    // The drop already exists: the word only sharpens its hint.
                    let line = DropRecord::exit_line(recovery.at_ms, reason);
                    if let Err(e) = self.drop_store.append(line) {
                        self.log_event("WARN", &format!("could not write the drop history: {e}"));
                        return NoteExit::Unused;
                    }
                    self.log_event("INFO", &format!("phone app exit: {}", reason.word()));
                    self.connection_log.push(
                        "event",
                        &format!("phone app exit: {}", reason.word()),
                        self.session_epoch_ms_at(now),
                    );
                    return NoteExit::Annotated;
                }
                // A quick reset turned out to be a crash: the neutral window
                // kept the drop from being counted, so record it now, from
                // the outage start to the moment the peer came back.
                self.record_drop("peer session reset", recovery.at_ms, now);
                let down_ms = recovery.back_ms.saturating_sub(recovery.at_ms).max(0) as u64;
                self.record_back(recovery.back_ms, down_ms);
                let line = DropRecord::exit_line(recovery.at_ms, reason);
                if let Err(e) = self.drop_store.append(line) {
                    self.log_event("WARN", &format!("could not write the drop history: {e}"));
                }
                let words = format!("phone app exit: {}", reason.word());
                self.log_event("INFO", &words);
                let drop_line = connection_log::drop_line_with_hint(
                    drop_history::REASON_RESET,
                    recovery.at_ms,
                    recovery.back_ms,
                    None,
                );
                self.connection_log
                    .push("drop", &drop_line, self.session_epoch_ms_at(now));
                self.connection_log.push(
                    "reconnect",
                    &connection_log::reconnected_line(recovery.back_ms),
                    self.session_epoch_ms_at(now),
                );
                NoteExit::FaultNow
            }
        }
    }

    /// Write one delivered desktop notification, and the reading behind it,
    /// to the connection log. The wording is built by the caller.
    pub fn log_notification(&mut self, text: &str, now: Instant) {
        self.connection_log
            .push("notice", text, self.session_epoch_ms_at(now));
    }

    /// Monotonic base matching `session_epoch_ms`. Tests use it to derive tick
    /// instants relative to the app's clock.
    pub fn session_epoch(&self) -> Instant {
        self.session_epoch
    }

    /// Feed one session sample from the current state and counters. The first
    /// sample is only the counter baseline. Sampling the same instant twice
    /// adds no bytes and no connected time.
    pub fn sample_session(&mut self, now: Instant) {
        let traffic = self.traffic();
        if traffic.is_some() {
            self.last_traffic = traffic;
        }
        let Some((sent, received)) = traffic.or(self.last_traffic) else {
            return;
        };
        let state = format!("{:?}", self.state());
        let endpoint = self
            .peer_endpoint
            .map(|e| e.to_string())
            .unwrap_or_default();
        let reason = self
            .orchestrator
            .last_disconnect_reason()
            .unwrap_or("")
            .to_string();
        let now_ms = self.session_epoch_ms_at(now);
        self.session
            .sample(now_ms, &state, received, sent, &endpoint, &reason);

        if self.usage_on {
            let connected = self.state() == ConnectionState::Connected;
            let sample = UsageSample {
                at_ms: now_ms,
                up_bytes: sent,
                down_bytes: received,
                connected,
                network: NetworkKind::Unknown,
            };
            if let Some(completed) = self.usage_sampler.step(sample) {
                self.usage_store.push(completed);
            }
        }
        self.save_live_session(now);
    }

    /// Write the live session's last figures at most once per interval. The
    /// write rides on the existing sample tick. A record with no activity is
    /// not worth a file: it says nothing about a session.
    fn save_live_session(&mut self, now: Instant) {
        if !self.session_history_owner {
            return;
        }
        let stats = self.session.current();
        if stats.connected_ms == 0
            && stats.up_bytes == 0
            && stats.down_bytes == 0
            && stats.drops == 0
            && stats.reconnects.is_empty()
        {
            return;
        }
        // The first active sample starts the interval; the record is written
        // on a later sample. This keeps a short test or a short connect from
        // paying for a write before it has anything worth saving.
        let Some(last) = self.last_live_save else {
            self.last_live_save = Some(now);
            return;
        };
        if now.saturating_duration_since(last) < LIVE_SESSION_SAVE_INTERVAL {
            return;
        }
        self.last_live_save = Some(now);
        self.session_history.set_live(stats);
        if let Err(e) = self.session_history.save(&self.sessions_path) {
            self.last_error = Some(format!("could not save the live session: {e}"));
        }
    }

    fn session_epoch_ms_at(&self, now: Instant) -> i64 {
        let elapsed = now.saturating_duration_since(self.session_epoch);
        self.session_epoch_ms_base
            .saturating_add(elapsed.as_millis().min(i64::MAX as u128) as i64)
    }

    // ---- diagnostic journal ----------------------------------------------
    //
    // Everything below is additive: it reads counters and writes records, and
    // must never alter a return, a branch, a timer or a connection decision.

    fn diag_event(&mut self, now: Instant, name: &str, fields: &[(&str, &str)]) {
        let ms = self.session_epoch_ms_at(now);
        let Some(diag) = self.diag.as_mut() else {
            return;
        };
        let _ = diag.event(ms, name, fields);
    }

    /// `diag_event` for owned field values built at runtime.
    fn diag_event_fields(&mut self, now: Instant, name: &str, fields: &[(&'static str, String)]) {
        if self.diag.is_none() {
            return;
        }
        let refs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.diag_event(now, name, &refs);
    }

    fn diag_session_start(&mut self, now: Instant) {
        let role = role_name(self.settings.role);
        self.diag_event(now, "session_start", &[("role", role)]);
    }

    fn diag_session_stop(&mut self, now: Instant, reason: &str) {
        self.diag_event(now, "session_stop", &[("reason", reason)]);
    }

    /// `-1` for an unavailable numeric value; never a fabricated zero.
    fn diag_number(value: Option<u64>) -> String {
        value
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-1".to_string())
    }

    /// The shared discovery/known control-socket counters plus their last
    /// rx/tx instants. These are not the kernel WireGuard data plane, so the
    /// fields are named `control_*`, not `udp_*`.
    fn diag_control_fields(&self) -> Vec<(&'static str, String)> {
        let control = jafnify_core::wire::stats();
        let errno = control
            .map(|s| s.last_errno)
            .filter(|errno| *errno != 0)
            .map(|errno| errno.to_string())
            .unwrap_or_else(|| "-1".to_string());
        let last_rx = control
            .and_then(|s| s.last_rx)
            .map(|t| self.session_epoch_ms_at(t).to_string())
            .unwrap_or_else(|| "-1".to_string());
        let last_tx = control
            .and_then(|s| s.last_tx)
            .map(|t| self.session_epoch_ms_at(t).to_string())
            .unwrap_or_else(|| "-1".to_string());
        vec![
            (
                "control_rx_packets",
                Self::diag_number(control.map(|s| s.rx_packets)),
            ),
            (
                "control_rx_bytes",
                Self::diag_number(control.map(|s| s.rx_bytes)),
            ),
            (
                "control_tx_packets",
                Self::diag_number(control.map(|s| s.tx_packets)),
            ),
            (
                "control_tx_bytes",
                Self::diag_number(control.map(|s| s.tx_bytes)),
            ),
            (
                "control_send_errors",
                Self::diag_number(control.map(|s| s.send_errors)),
            ),
            (
                "control_recv_errors",
                Self::diag_number(control.map(|s| s.recv_errors)),
            ),
            ("control_last_errno", errno),
            ("last_rx_ms", last_rx),
            ("last_tx_ms", last_tx),
        ]
    }

    /// The kernel tunnel interface counters (sysfs, no privileges needed).
    fn diag_tunnel_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "wg_rx_bytes",
                Self::diag_number(read_counter(&self.rx_bytes_path)),
            ),
            (
                "wg_tx_bytes",
                Self::diag_number(read_counter(&self.tx_bytes_path)),
            ),
        ]
    }

    /// Record one STATS sample when the cadence says it is due. The cadence
    /// itself lives in `diag::stats_due`; it is driven by the existing tick.
    fn diag_poll(&mut self, now: Instant) {
        if self.diag.is_none() {
            return;
        }
        if !diag::stats_due(now, self.last_diag_stats, self.last_peer_rx) {
            return;
        }
        let fields = self.diag_stats_fields();
        let ms = self.session_epoch_ms_at(now);
        let refs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if let Some(diag) = self.diag.as_mut() {
            let _ = diag.stats(ms, &refs);
        }
        self.last_diag_stats = Some(now);
    }

    /// Whether a window is showing the state. Routing and liveness never read
    /// the handshake age, so with no window open the helper is not called.
    pub fn set_viewer_open(&mut self, open: bool) {
        self.viewer_open = open;
    }

    /// Read the peer's handshake age at most once every
    /// [HANDSHAKE_POLL_INTERVAL] while the tunnel is up and a window is open.
    /// The helper is read-only and runs off the tick thread, so a slow or
    /// prompting `pkexec` can never freeze the app; its answer is simply late.
    fn handshake_poll(&mut self, now: Instant) {
        if self.state() != ConnectionState::Connected || !self.viewer_open {
            self.handshake_age = HandshakeAge::Unknown;
            self.handshake_job = None;
            self.last_handshake_query = None;
            return;
        }
        if let Some(job) = self.handshake_job.as_ref() {
            match job.try_recv() {
                Ok(age) => {
                    self.handshake_age = age;
                    self.handshake_job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.handshake_age = HandshakeAge::Unknown;
                    self.handshake_job = None;
                }
            }
        }
        if self
            .last_handshake_query
            .is_some_and(|last| now.saturating_duration_since(last) < HANDSHAKE_POLL_INTERVAL)
        {
            return;
        }
        self.last_handshake_query = Some(now);
        let root = self.root.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(root.handshake_age());
        });
        self.handshake_job = Some(rx);
    }

    /// The tunnel rx counter advanced (or restarted) => peer data is flowing.
    /// A flat counter leaves `last_peer_rx` alone, which switches the diag
    /// cadence to 1 s after 3 s. This never touches `rx_watch`.
    fn note_diag_peer_rx(&mut self, now: Instant) {
        let Some((_sent, received)) = self.last_traffic else {
            return;
        };
        match self.last_diag_rx {
            None if received > 0 => self.last_peer_rx = Some(now),
            Some(previous) if received > previous => self.last_peer_rx = Some(now),
            Some(previous) if received < previous => {
                self.last_peer_rx = (received > 0).then_some(now);
            }
            _ => {}
        }
        self.last_diag_rx = Some(received);
    }

    fn diag_stats_fields(&mut self) -> Vec<(&'static str, String)> {
        let mut fields = self.diag_control_fields();
        let peer = self
            .peer_endpoint
            .map(|endpoint| endpoint.to_string())
            .unwrap_or_else(|| "-1".to_string());
        fields.push(("peer", peer));
        // Prefer the route source; fall back to an address already chosen for
        // the search, never to a guessed one.
        self.refresh_diag_egress();
        let local = self
            .diag_egress
            .src
            .map(|ip| ip.to_string())
            .or_else(|| self.search_ifaces.first().map(|ip| ip.to_string()))
            .or_else(|| self.iface_pin.map(|ip| ip.to_string()))
            .unwrap_or_else(|| "-1".to_string());
        fields.push(("local", local));
        // The Linux data plane is kernel WireGuard: no handshake age is
        // readable without root, so the field is honestly unavailable.
        fields.push(("wg_handshake_ms", "-1".to_string()));
        fields.extend(self.diag_tunnel_fields());
        fields.push((
            "egress_iface",
            self.diag_egress
                .iface
                .clone()
                .unwrap_or_else(|| "-1".to_string()),
        ));
        fields.push((
            "egress_src",
            self.diag_egress
                .src
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "-1".to_string()),
        ));
        fields.push((
            "carrier",
            match self.diag_egress.carrier {
                Some(true) => "1",
                Some(false) => "0",
                None => "-1",
            }
            .to_string(),
        ));
        fields.push((
            "operstate",
            self.diag_egress
                .operstate
                .clone()
                .unwrap_or_else(|| "-1".to_string()),
        ));
        fields.push(("state", connection_state_name(self.state()).to_string()));
        fields
    }

    /// Refresh the egress iface/src/carrier/operstate snapshot. `link_poll`
    /// is the single source of `link_change` events; this only fills STATS.
    fn refresh_diag_egress(&mut self) {
        let iface = self
            .link_watch
            .as_ref()
            .map(|watch| watch.iface().to_string())
            .or_else(|| self.link_iface.clone());
        let src = self.route_src();
        let (carrier, operstate) = match &iface {
            Some(name) => {
                let base = self.uplink_sysfs.join(name);
                let carrier = std::fs::read_to_string(base.join("carrier"))
                    .ok()
                    .map(|text| text.trim() == "1");
                let operstate = std::fs::read_to_string(base.join("operstate"))
                    .ok()
                    .and_then(|text| parse_operstate(&text).map(str::to_string));
                (carrier, operstate)
            }
            None => (None, None),
        };
        self.diag_egress = EgressSnapshot {
            iface,
            src,
            carrier,
            operstate,
        };
    }

    /// The source address of the route to the peer, from read-only `ip`.
    /// Loopback peers cannot carry an uplink route, so no process is run.
    fn route_src(&self) -> Option<IpAddr> {
        let endpoint = self.peer_endpoint?;
        if endpoint.ip().is_loopback() {
            return None;
        }
        let mut cmd = Command::new(&self.ip_bin);
        cmd.args(["-4", "route", "get", &endpoint.ip().to_string()]);
        let output = exec::output(cmd).ok()?;
        if !output.status.success() {
            return None;
        }
        parse_route_src(&String::from_utf8_lossy(&output.stdout))
    }

    /// The 35 s/no-data decision, with the counters as they stood then.
    fn diag_no_data(&mut self, now: Instant, reason: &str) {
        if self.diag.is_none() {
            return;
        }
        let mut fields = vec![("reason", reason.to_string())];
        fields.extend(self.diag_control_fields());
        fields.extend(self.diag_tunnel_fields());
        self.diag_event_fields(now, "no_data", &fields);
    }

    /// A known-peer session-reset decision and its reason.
    fn diag_peer_reset(&mut self, now: Instant, reason: &str) {
        if self.diag.is_none() {
            return;
        }
        let mut fields = vec![("reason", reason.to_string())];
        fields.extend(self.diag_control_fields());
        fields.extend(self.diag_tunnel_fields());
        self.diag_event_fields(now, "peer_reset", &fields);
    }

    /// A known frame (Hello or Proof) left this process during `poll_detailed`.
    /// The core does not label the frame; exact 36/70-byte sends are named.
    fn diag_known_tx(&mut self, now: Instant, before: Option<jafnify_core::wire::HubStats>) {
        if self.diag.is_none() {
            return;
        }
        let Some(after) = jafnify_core::wire::stats() else {
            return;
        };
        let before = before.unwrap_or_default();
        let sent = after.tx_packets.saturating_sub(before.tx_packets);
        if sent == 0 {
            return;
        }
        let bytes = after.tx_bytes.saturating_sub(before.tx_bytes);
        let mut fields = vec![
            ("source", "known".to_string()),
            ("sent", sent.to_string()),
            ("bytes", bytes.to_string()),
        ];
        if bytes == 36 {
            fields.push(("frame", "hello".to_string()));
        } else if bytes == 70 {
            fields.push(("frame", "proof".to_string()));
        }
        self.diag_event_fields(now, "probe", &fields);
    }

    /// Close the live session as a user Stop.
    fn end_session(&mut self, now: Instant) {
        self.close_session(now, "stopped");
    }

    /// Close the live session and store it unless it never saw any activity or
    /// connected time. A failed write is reported through `last_error`.
    fn close_session(&mut self, now: Instant, reason: &str) {
        self.sample_session(now);
        self.flush_usage();
        let now_ms = self.session_epoch_ms_at(now);
        self.connection_log.push(
            "session",
            &connection_log::session_ended_line(reason),
            now_ms,
        );
        self.diag_session_stop(now, reason);
        let record = self.session.finish(now_ms, reason);
        let had_live = self.session_history.clear_live();
        if record.last_activity_at == 0
            && record.connected_ms == 0
            && record.up_bytes == 0
            && record.down_bytes == 0
            && record.drops == 0
            && record.reconnects.is_empty()
        {
            if had_live && self.session_history_owner {
                let _ = self.session_history.save(&self.sessions_path);
            }
            return;
        }
        self.session_history.push(record);
        if self.session_history_owner {
            if let Err(e) = self.session_history.save(&self.sessions_path) {
                self.last_error = Some(format!("could not save the session: {e}"));
            }
        }
    }

    /// Close the live session when the daemon is exiting cleanly. An empty
    /// session after a Stop is left alone: it has nothing to store and would
    /// only add a noise record.
    pub fn close_session_on_exit(&mut self) {
        if !self.session_history_owner {
            return;
        }
        let stats = self.session.current();
        if stats.connected_ms == 0
            && stats.up_bytes == 0
            && stats.down_bytes == 0
            && stats.drops == 0
            && stats.reconnects.is_empty()
        {
            return;
        }
        self.close_session(Instant::now(), "App closed");
    }

    pub fn save(&mut self) -> Result<(), SettingsError> {
        self.state.settings = self.settings.clone();
        save_state(&self.config_dir.join("state.toml"), &self.state)
    }

    /// Pairing track: search with a 4-digit code. A successful pairing
    /// replaces any stored device.
    pub fn start(&mut self, code: &str) -> Result<(), AppError> {
        let code = PairingCode::parse(code).ok_or(AppError::InvalidCode)?;
        if self.block_on_foreign_tunnel() {
            return Ok(());
        }
        // A new attempt starts clean: an old failure is not news any more.
        self.last_error = None;
        // A new pairing is a new session, even right after a stop.
        self.resume_candidate = None;
        self.begin_run(Instant::now());
        self.log_event("INFO", "search started (code track)");
        let ifaces = self.lan_ifaces()?;
        self.use_known = false;
        self.open_search(code, ifaces)?;
        self.diag_session_start(Instant::now());
        Ok(())
    }

    /// Known track: reconnect to the paired device with no code.
    pub fn start_known(&mut self) -> Result<(), AppError> {
        if !self.is_paired() {
            return Err(AppError::NoPairedDevice);
        }
        if self.block_on_foreign_tunnel() {
            return Ok(());
        }
        self.last_error = None;
        if !self.try_resume(Instant::now()) {
            self.begin_run(Instant::now());
        }
        self.log_event("INFO", "search started (known peer)");
        if self.adopt_existing(Instant::now())? {
            self.diag_session_start(Instant::now());
            return Ok(());
        }
        let ifaces = self.lan_ifaces()?;
        self.open_known_search(ifaces, Instant::now())?;
        self.diag_session_start(Instant::now());
        Ok(())
    }

    /// Take over our own tunnel that an earlier process left up, but only
    /// after the helper confirms it carries this stored session. The marker is
    /// a hint, never proof. Adoption starts in Connecting and needs peer data
    /// before the app may call it Connected.
    fn adopt_existing(&mut self, now: Instant) -> Result<bool, AppError> {
        // Adoption never raises the tunnel itself; any note armed by an earlier
        // attempt is stale until a search starts.
        self.polkit_prompt_expected = false;
        if !self.tunnel_present() {
            return Ok(false);
        }
        let Some(peer) = self.stored_peer() else {
            return Ok(false);
        };
        if !self.root.has_helper() {
            self.log_event(
                "WARN",
                "jafn0 exists but the helper is missing; not adopted",
            );
            return Ok(false);
        }
        self.use_known = true;
        self.known = None;
        self.keypair = peer
            .private_key
            .as_deref()
            .and_then(|k| KeyPair::from_private_b64(k).ok());
        self.peer_public_key = Some(peer.public_key.clone());
        self.peer_endpoint = peer
            .last_endpoint
            .as_deref()
            .and_then(|e| e.parse::<SocketAddr>().ok());
        let plan = match self.desired_session() {
            Ok(plan) => plan,
            Err(e) => {
                self.log_event("WARN", &format!("cannot build adoption config: {e}"));
                return Ok(false);
            }
        };
        let live = match self.read_live_config() {
            Ok(live) => live,
            Err(e) => {
                self.log_event("WARN", &format!("inspect jafn0: {e}"));
                return Ok(false);
            }
        };
        if !adoption_match(&plan.desired, &live) {
            self.log_event("WARN", "jafn0 does not match this stored pair; not adopted");
            self.marker_reuse_blocked = true;
            return Ok(false);
        }
        self.peer_endpoint = live
            .endpoint
            .as_deref()
            .and_then(|e| e.parse::<SocketAddr>().ok())
            .or(self.peer_endpoint);
        self.write_marker(&live);
        self.reconnect_rx = self.read_rx();
        self.search_ifaces.clear();
        self.connected_since = None;
        self.conn_clock.disconnected();
        self.last_peer_proof = None;
        self.disconnected_at = None;
        self.rx_watch = None;
        self.connecting_since = Some(now);
        self.last_error = None;
        self.denied = false;
        self.polkit_prompt_expected = false;
        self.adopting = true;
        // Kept from an earlier process: its session keys may be stale.
        self.tunnel_reused = true;
        self.marker_reuse_blocked = false;
        self.owns_tunnel = true;
        self.foreign_tunnel = false;
        self.live_peer_key_checked = true;
        self.live_peer_key = Some(self.peer_public_key.clone().unwrap_or_default());
        // An earlier process falls back before it exits, so this jafn0 may be
        // up with its routing already down. Record that before Connecting:
        // the first Connected tick then restores it on data instead of calling
        // a state that predates the connection a fault.
        self.routing_down = if self.settings.kill_switch {
            false
        } else {
            adopted_routing_down(tunnel_rule_installed(&self.root.wg_bin, &self.ip_bin))
        };
        self.orchestrator.start_known_search();
        self.orchestrator.known_peer_found();
        // The known track runs for an adopted tunnel too: without it the
        // peer's Bye and Fresh would go unseen until the next loss.
        match self.lan_ifaces() {
            Ok(ifaces) => {
                let cfg = DiscoveryConfig {
                    unicast: self.joiner_gateways(&ifaces),
                    ifaces: ifaces.clone(),
                    ..Default::default()
                };
                let opened = self.keypair.as_ref().map(|keypair| {
                    KnownTrack::open(&cfg, keypair, &peer.public_key, self.settings.role, now)
                });
                match opened {
                    Some(Ok(mut track)) => {
                        track.fail_sends(self.known_send_fault);
                        self.known = Some(track);
                        self.search_ifaces = ifaces;
                        self.search_recheck_at = None;
                    }
                    Some(Err(e)) => {
                        self.log_event("WARN", &format!("adopted without a known track: {e}"))
                    }
                    None => self.log_event("WARN", "adopted without a known track: no key"),
                }
            }
            Err(e) => self.log_event("WARN", &format!("adopted without a known track: {e}")),
        }
        self.log_event(
            "INFO",
            "adopted existing jafn0 after inspect; waiting for peer data",
        );
        Ok(true)
    }

    fn stored_peer(&self) -> Option<Peer> {
        self.state
            .peers
            .iter()
            .find(|p| p.private_key.is_some())
            .cloned()
    }

    fn read_marker(&self) -> Option<TunnelMarker> {
        let text = std::fs::read_to_string(&self.marker_path).ok()?;
        toml::from_str::<TunnelMarker>(&text).ok()
    }

    fn write_marker(&self, live: &LiveConfig) {
        let marker = TunnelMarker {
            peer_public_key: live.peer_public_key.clone(),
            endpoint: live.endpoint.clone(),
            allowed_ips: live.allowed_ips.clone(),
            address: live.address.clone(),
            kill_switch: live.kill_switch,
        };
        self.write_marker_inner(&marker);
    }

    fn write_marker_inner(&self, marker: &TunnelMarker) {
        let text = toml::to_string(marker).expect("marker serializable");
        if let Some(parent) = self.marker_path.parent() {
            let _ = ensure_private_dir(parent);
        }
        if std::fs::write(&self.marker_path, text).is_ok() {
            let _ =
                std::fs::set_permissions(&self.marker_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    fn marker_matches(&self, desired: &LiveConfig) -> bool {
        self.read_marker().is_some_and(|marker| {
            marker.peer_public_key == desired.peer_public_key
                && marker.endpoint == desired.endpoint
                && marker.allowed_ips == desired.allowed_ips
                && marker.address == desired.address
                && marker.kill_switch == desired.kill_switch
        })
    }

    fn clear_marker(&self) {
        let _ = std::fs::remove_file(&self.marker_path);
    }

    /// The interfaces discovery runs on. A sharer uses every up,
    /// multicast-capable LAN interface: the other device may be on one that
    /// is not the default route (ethernet upstream, hotspot downstream). A
    /// joiner uses every up private LAN interface, for the same reason: the
    /// laptop's default route may be on Ethernet or a second Wi-Fi while the
    /// phone is on the hotspot. The saved peer's subnet goes first, so the
    /// network that leads to the phone is tried before the others. The
    /// default-route interface is the fallback. A user pin wins.
    fn lan_ifaces(&mut self) -> Result<Vec<std::net::Ipv4Addr>, AppError> {
        let ifaces = self.search_addresses();
        if !ifaces.is_empty() {
            return Ok(ifaces);
        }
        self.lan_ip().map(|ip| vec![ip])
    }

    /// Never fall back to an unpinned socket: with a tunnel up the kernel
    /// would route the multicast group into it (e.g. wg0, table 51820).
    fn lan_ip(&mut self) -> Result<std::net::Ipv4Addr, AppError> {
        let Some(iface) = self
            .iface_pin
            .or_else(|| detect_lan(&self.ip_bin).map(|(_, ip, _)| ip))
        else {
            self.last_error = Some(
                "no LAN interface found - connect to a network (tunnel and loopback interfaces are ignored)"
                    .to_string(),
            );
            return Err(AppError::NoLanInterface);
        };
        Ok(iface)
    }

    /// Forget the paired device: one privileged call takes the tunnel down,
    /// removes the config and the per-user polkit rule, then the record and key
    /// are deleted.
    pub fn forget(&mut self) -> Result<(), AppError> {
        self.polkit_prompt_expected = false;
        // Sample before the root call too; a refused forget must not end the
        // session, but it still captures the final counters.
        let now = Instant::now();
        self.sample_session(now);
        let tunnel = self.tunnel_present();
        let conf = self.installed_conf_present();
        // The rule directory is root-only, so the app must not stat it. A
        // stored pair is its own reason: after a reboot an idle Forget still
        // needs the helper to remove the pair's config/rule leftovers.
        let paired = !self.state.peers.is_empty();
        if !tunnel {
            // Idle forget still removes a kill switch or config left by a
            // failed bring-up, where no interface exists to run PostDown.
            let _ = self.run_teardown(false);
        }
        if tunnel || conf || paired {
            self.run_root("forget", self.root.forget_command())?;
        }
        self.clear_marker();
        self.end_session(now);
        self.reset_session(now);
        if !self.state.peers.is_empty() {
            let mut state = self.state.clone();
            state.peers.clear();
            save_state(&self.config_dir.join("state.toml"), &state)?;
            self.state = state;
        }
        Ok(())
    }

    /// Begin a clean search: new sockets (so no frame queued from an earlier
    /// peer can be taken as live), new keypair, new discovery nonce. Used by
    /// `start` and whenever a retry goes back to Searching.
    fn open_search(
        &mut self,
        code: PairingCode,
        ifaces: Vec<std::net::Ipv4Addr>,
    ) -> Result<(), AppError> {
        let keypair = KeyPair::generate();
        let cfg = DiscoveryConfig {
            unicast: self.joiner_gateways(&ifaces),
            ifaces: ifaces.clone(),
            ..Default::default()
        };
        self.discovery = Some(Discovery::bind(&cfg)?);
        self.handshake = Some(Handshake::bind(&cfg)?);
        let own_port = match self.settings.role {
            Role::Sharer => jafnify_core::LISTEN_PORT,
            Role::Joiner => 0,
        };
        self.pake = Some(PakeLink::open(
            &cfg,
            self.settings.role,
            &code,
            &keypair,
            own_port,
        )?);
        self.known = None;
        self.adopting = false;
        self.keypair = Some(keypair);
        self.local_code = Some(code.clone());
        self.pake_wrong_code_logged = false;
        self.denied = false;
        // A new attempt may need a password as soon as a peer is found, so the
        // note is visible during Searching/Connecting, before the root call.
        self.polkit_prompt_expected = !self.pkcheck_authorized();
        self.search_ifaces = ifaces;
        self.search_recheck_at = None;
        self.peer_public_key = None;
        self.peer_endpoint = None;
        self.last_announce = None;
        self.connecting_since = None;
        self.rx_watch = None;
        // Armed by PeerFound, never by start(): a lone device must not pair
        // with its own multicast loopback.
        self.auto_accept_deadline = None;
        self.start_firewall_check();
        self.orchestrator.start_search(code);
        Ok(())
    }

    /// Begin a known-peer search with the stored pair key: no 0x01/0x02 at
    /// all, only 0x03 Hello/Proof. The tunnel is raised with the STORED key,
    /// after an accepted Proof (joiner) or at once (sharer).
    fn open_known_search(
        &mut self,
        ifaces: Vec<std::net::Ipv4Addr>,
        now: Instant,
    ) -> Result<(), AppError> {
        let peer = stored_pair(&self.state.peers).ok_or(AppError::NoPairedDevice)?;
        let private = peer.private_key.clone().unwrap_or_default();
        let keypair =
            KeyPair::from_private_b64(&private).map_err(|e| KnownError::Key(e.to_string()))?;
        let cfg = DiscoveryConfig {
            unicast: self.joiner_gateways(&ifaces),
            ifaces: ifaces.clone(),
            ..Default::default()
        };
        let mut track =
            KnownTrack::open(&cfg, &keypair, &peer.public_key, self.settings.role, now)?;
        track.fail_sends(self.known_send_fault);
        self.known = Some(track);
        self.adopting = false;
        self.use_known = true;
        self.discovery = None;
        self.handshake = None;
        self.pake = None;
        self.keypair = Some(keypair);
        self.local_code = None;
        self.denied = false;
        // The known track may bring the tunnel up at any tick; publish the
        // prompt hint now, not only for the duration of the blocking call.
        self.polkit_prompt_expected = !self.pkcheck_authorized();
        self.search_ifaces = ifaces;
        self.search_recheck_at = None;
        self.peer_public_key = None;
        self.peer_endpoint = None;
        self.last_announce = None;
        self.connecting_since = None;
        self.rx_watch = None;
        self.auto_accept_deadline = None;
        self.start_firewall_check();
        self.orchestrator.start_known_search();
        Ok(())
    }

    /// Default gateways of the search interfaces, for unicast discovery. Only
    /// the joiner asks: on a hotspot its gateway is the phone, whose multicast
    /// may never reach us.
    fn joiner_gateways(&self, ifaces: &[std::net::Ipv4Addr]) -> Vec<std::net::Ipv4Addr> {
        if self.settings.role != Role::Joiner {
            return Vec::new();
        }
        let names: Vec<String> = detect_lan_ifaces(&self.ip_bin)
            .into_iter()
            .filter(|i| ifaces.contains(&i.ip))
            .map(|i| i.name)
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let routes = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
        jafnify_core::route::gateways_for(&routes, &names)
    }

    // ---- host firewall (see `firewall`) ---------------------------------------

    /// ufw is enabled on this computer.
    pub fn ufw_active(&self) -> bool {
        std::fs::read_to_string(&self.ufw_conf).is_ok_and(|c| ufw_enabled(&c))
    }

    /// Begin judging whether the host firewall eats our discovery frames.
    fn start_firewall_check(&mut self) {
        let cfg = DiscoveryConfig {
            ifaces: self.search_ifaces.clone(),
            ..Default::default()
        };
        self.firewall = FirewallCheck::start(&cfg, Instant::now(), self.ufw_active());
        self.firewall_verdict = Verdict::Unknown;
        self.firewall_iface = None;
    }

    fn end_firewall_check(&mut self) {
        self.firewall = None;
        self.firewall_verdict = Verdict::Unknown;
        self.firewall_iface = None;
    }

    /// Read what the watch has heard and update the verdict. `tick_at` does
    /// this on every tick; the LAN interface is looked up once, when a block
    /// first shows, to scope the rule.
    pub fn firewall_poll(&mut self, now: Instant) -> Verdict {
        let Some(check) = self.firewall.as_mut() else {
            return Verdict::Unknown;
        };
        let verdict = check.poll(now);
        let newly_blocked = matches!(verdict, Verdict::Blocked(_))
            && !matches!(self.firewall_verdict, Verdict::Blocked(_));
        self.firewall_verdict = verdict;
        if newly_blocked {
            self.firewall_iface = self.lan_iface();
        }
        verdict
    }

    /// The host firewall is dropping our discovery frames, and the search is
    /// still going: what is blocking (None = no block seen, or not searching).
    pub fn firewall_blocked(&self) -> Option<FirewallKind> {
        match self.firewall_verdict {
            Verdict::Blocked(kind) if self.state() == ConnectionState::Searching => Some(kind),
            _ => None,
        }
    }

    /// The default-route LAN interface a rule would be scoped to ("wlo1"),
    /// once known.
    pub fn firewall_iface(&self) -> Option<&str> {
        self.firewall_iface.as_deref()
    }

    /// The default-route LAN interface from read-only `ip` queries.
    pub fn lan_iface(&self) -> Option<String> {
        detect_lan(&self.ip_bin).map(|(name, _, _)| name)
    }

    /// The interface the tunnel uses, once the link watch knows it.
    pub fn uplink_iface(&self) -> Option<&str> {
        self.link_watch.as_ref().map(|watch| watch.iface())
    }

    /// The network of the default-route LAN interface, from read-only `ip`
    /// queries; None if there is none or it is not LAN-sized.
    pub fn lan_subnet(&self) -> Option<String> {
        let (_, ip, prefix) = detect_lan(&self.ip_bin)?;
        lan_subnet(ip, prefix)
    }

    /// Add the one narrow ufw rule (UDP, the discovery group and port, on the
    /// LAN interface) through pkexec. Call only after the user said yes.
    /// Returns the interface it was scoped to. Never touches anything else: no
    /// other rule, no disabling, no reset.
    pub fn allow_firewall(&mut self) -> Result<String, AppError> {
        if !self.ufw_active() {
            return Err(AppError::FirewallRule(
                "ufw is not enabled, so no ufw rule can be added (run `jafnify firewall rule` for the rule text)".to_string(),
            ));
        }
        let Some(iface) = self.lan_iface() else {
            return Err(AppError::FirewallRule(
                "no LAN interface found to scope the rule to".to_string(),
            ));
        };
        self.run_root("ufw allow", self.root.ufw_allow_command(&iface))?;
        if self.firewall.is_some() {
            // Judge the search afresh under the new rule.
            self.start_firewall_check();
        }
        Ok(iface)
    }

    fn stored_peer_pk(&self) -> Option<String> {
        stored_pair(&self.state.peers).map(|p| p.public_key.clone())
    }

    pub fn accept_code(&mut self, code: &str) -> Result<(), AppError> {
        self.orchestrator.try_accept_code(code)?;
        // Connecting announces the public key right away.
        self.last_announce = None;
        Ok(())
    }

    /// Announce our discovery token (frame 0x01) once per ANNOUNCE_INTERVAL.
    fn announce_discovery(&mut self, now: Instant) -> Result<(), AppError> {
        let due = self
            .last_announce
            .map(|t| now.duration_since(t) >= ANNOUNCE_INTERVAL)
            .unwrap_or(true);
        if due {
            if let (Some(d), Some(token)) = (&self.discovery, self.orchestrator.own_token()) {
                d.announce(&token)?;
                self.last_announce = Some(now);
                self.diag_event(now, "probe", &[("source", "discovery"), ("frame", "0x01")]);
            }
        }
        Ok(())
    }

    pub fn tick(&mut self) -> Result<Option<UiEvent>, AppError> {
        self.tick_at(Instant::now())
    }

    /// `tick` with the caller's clock (tests inject time).
    /// Test seam: make the known-peer sends fail with `kind`, as with no
    /// network (None restores).
    #[doc(hidden)]
    pub fn fail_known_sends(&mut self, kind: Option<std::io::ErrorKind>) {
        self.known_send_fault = kind;
        if let Some(track) = self.known.as_mut() {
            track.fail_sends(kind);
        }
    }

    /// Test seam: only the current known track's sends fail with `kind`; a
    /// track opened later starts clean (a track pinned to a stale address).
    #[doc(hidden)]
    pub fn fail_current_known_track(&mut self, kind: Option<std::io::ErrorKind>) {
        if let Some(track) = self.known.as_mut() {
            track.fail_sends(kind);
        }
    }

    pub fn tick_at(&mut self, now: Instant) -> Result<Option<UiEvent>, AppError> {
        let before = self.state();
        let mut event = if self.known.is_some() {
            self.tick_known(now)
        } else if self.adopting {
            self.tick_adopted(now)
        } else {
            self.tick_code(now)?
        };
        // An adopted tunnel has no KnownTrack; watch its sysfs counter
        // directly until a loss moves us to Retry and opens a fresh track.
        if self.known.is_none() && self.state() == ConnectionState::Connected {
            if let Some(reason) = self.rx_stalled(now) {
                self.diag_no_data(now, &reason);
                event = Some(self.known_lost(reason, now, false));
            }
        }
        let was_retry = self.state() == ConnectionState::Retry;
        if let Some(ev) = self.orchestrator.tick(now) {
            match ev {
                Event::NoPeerFound => event = Some(UiEvent::NoPeerFound),
            }
        }
        if was_retry && self.state() == ConnectionState::Searching {
            // The 10 s retry elapsed: search again from scratch.
            if let Some(ev) = self.restart_search(now) {
                event = Some(ev);
            }
        }
        self.foreign_poll();
        self.track_connected(now);
        self.handshake_poll(now);
        self.range_poll(now);
        // Before the sample, so a link loss this tick is in the same stats.
        self.link_poll(now);
        self.fallback_poll(now);
        self.sample_session(now);
        self.note_diag_peer_rx(now);
        self.diag_poll(now);
        self.firewall_poll(now);
        self.suppress_poll(now);
        let after = self.state();
        if before != after {
            self.log_event("INFO", &format!("state {before:?} -> {after:?}"));
            self.connection_log.push(
                "state",
                &connection_log::state_line(&format!("{before:?}"), &format!("{after:?}")),
                self.session_epoch_ms_at(now),
            );
            let from = format!("{before:?}");
            let to = format!("{after:?}");
            self.diag_event(now, "state", &[("from", &from), ("to", &to)]);
        }
        // The link is back: the open drop closes here, on every path that
        // reaches Connected. The code track does not go through
        // `known_connected`, so the close cannot live there alone.
        if after == ConnectionState::Connected {
            self.close_drop(now);
            // The connection log gets its line for the same return.
            self.log_reconnected(now);
        }
        // The phone may be away for longer than the edge: the outage is then
        // an absence and not a fault of the link.
        self.update_away(now);
        Ok(event)
    }

    /// Keeps the connected clock, and stamps the stored pair's last connection
    /// the moment the tunnel comes up.
    fn track_connected(&mut self, now: Instant) {
        let session = self.state() == ConnectionState::Connected;
        let connected = self.display_connected_at(now);
        match (connected, self.connected_since) {
            (true, None) => {
                // Any earlier error is stale once data is flowing, except a
                // pairing-save failure: that one is about the current pair and
                // is cleared only by a later successful save.
                if self
                    .last_error
                    .as_deref()
                    .is_some_and(|e| !is_pairing_save_error(e))
                {
                    self.last_error = None;
                }
                self.connected_since = Some(now);
                // A display-only neutral keeps the phone's value across the
                // blip; a first connect or a new session starts fresh.
                self.conn_clock.resume(self.clock_ms(now));
                self.polkit_prompt_expected = false;
                self.stamp_last_connected();
            }
            (false, Some(_)) => {
                self.connected_since = None;
                if session {
                    self.conn_clock.pause();
                } else {
                    self.conn_clock.disconnected();
                }
            }
            _ => {}
        }
        if !session {
            self.conn_clock.disconnected();
        }
    }

    /// The clock's milliseconds; both the local start and the adopted phone
    /// value use this one base.
    fn clock_ms(&self, now: Instant) -> i64 {
        now.saturating_duration_since(self.clock_origin).as_millis() as i64
    }

    /// While Connected, no authenticated peer activity for this long means the
    /// tunnel carries nothing: the display is neutral. The orchestrator state
    /// and the drop/away rules are untouched.
    fn data_stale(&self, now: Instant) -> bool {
        // A peer session reset is neutral until the peer comes back: the
        // display must say Reconnecting at once, even though the reset Proof
        // was a fresh peer frame.
        if self.reset_neutral {
            return true;
        }
        if self.last_peer_proof.is_some() {
            // The known track has the Hello/Proof heartbeat.
            let proof_fresh = self
                .last_peer_proof
                .is_some_and(|at| now.saturating_duration_since(at) <= FRESH_GAP);
            let rx_fresh = self
                .last_peer_rx
                .is_some_and(|at| now.saturating_duration_since(at) <= FRESH_GAP);
            !(proof_fresh || rx_fresh)
        } else {
            // A code or adopted tunnel has no known track: the liveness
            // watchdog's own verdict is the authenticated-data signal.
            self.tunnel_dead
        }
    }

    /// The display says Connected only while the tunnel is Connected, the
    /// uplink is up and authenticated peer activity is fresh.
    fn display_connected_at(&self, now: Instant) -> bool {
        self.state() == ConnectionState::Connected && !self.link_lost && !self.data_stale(now)
    }

    /// The display state for the tiles and the CLI. Same rule as the status
    /// text, so the home card never claims Connected while it says Reconnecting.
    pub fn display_connected(&self) -> bool {
        self.display_connected_at(Instant::now())
    }

    /// Adopt the phone's connected time from a summary reply received at
    /// `at`. `None` is an old phone: the local clock stays.
    pub fn observe_phone_connected_ms(&mut self, sconn_ms: Option<u64>, at: Instant) {
        if let Some(ms) = sconn_ms {
            let at_ms = self.clock_ms(at);
            // The phone's elapsed value rewound: its app was updated or it
            // restarted. The up time and the live session bytes must describe
            // the same, new phone session.
            if self.conn_clock.peer_rewound(ms, at_ms) {
                self.conn_clock.restart(self.clock_ms(at));
                self.start_phone_session(at);
            }
            self.conn_clock.observe_peer_elapsed(ms, at_ms);
        }
    }

    /// The phone started a new sharing session. The live session record
    /// follows the same event as the connected clock; today's usage keeps its
    /// own counters.
    fn start_phone_session(&mut self, at: Instant) {
        self.log_event("INFO", "phone session restarted");
        let now_ms = self.session_epoch_ms_at(at);
        // Close the old session in the history: it ended where the phone's
        // new session began, not as a drop. Today's usage is a separate
        // sampler and keeps its counters.
        let old = self.session.finish(now_ms, "phone session restarted");
        if old.connected_ms > 0 || old.up_bytes > 0 || old.down_bytes > 0 {
            self.session_history.push(old);
            if self.session_history_owner {
                if let Err(e) = self.session_history.save(&self.sessions_path) {
                    self.log_event("WARN", &format!("could not save the session: {e}"));
                }
            }
        }
        let role = role_name(self.settings.role);
        self.session = SessionTracker::new(role, now_ms);
        self.last_live_save = None;
        self.connected_since = Some(at);
    }

    /// Read the Wi-Fi level of the uplink once a second while Connected. No
    /// Wi-Fi uplink (a cable, a lab veth, no wireless extensions) is no
    /// sample: the tile is absent and the drops keep their old hints.
    fn range_poll(&mut self, now: Instant) {
        if self.state() != ConnectionState::Connected {
            return;
        }
        if self
            .last_range_sample
            .is_some_and(|last| now.saturating_duration_since(last) < RANGE_POLL_INTERVAL)
        {
            return;
        }
        self.last_range_sample = Some(now);
        let Some(iface) = self.uplink_iface().map(str::to_string) else {
            return;
        };
        let Some(sample) = self.station.read(&iface) else {
            return;
        };
        let at_ms = self.session_epoch_ms_at(now);
        self.station_ring.push(at_ms, sample);
        if let Some(level) = sample.level() {
            self.range_trend.push(at_ms, i64::from(level));
        }
        self.log_link_sample(at_ms, &sample);
    }

    /// Keep one reading every ten seconds in `link.tsv`, while the drop
    /// history is on: the data the range thresholds are tuned on. Bounded,
    /// local, numbers only.
    fn log_link_sample(&mut self, at_ms: i64, sample: &jafnify_core::wifi_range::StationSample) {
        if !self.drop_history_on {
            return;
        }
        if self
            .last_link_log_ms
            .is_some_and(|last| at_ms - last < jafnify_core::wifi_range::LINK_LOG_EVERY_MS)
        {
            return;
        }
        self.last_link_log_ms = Some(at_ms);
        let line = jafnify_core::wifi_range::link_log_line(at_ms, sample);
        let path = self.config_dir.join("link.tsv");
        if let Err(e) =
            crate::station::append_bounded(&path, &line, crate::station::LINK_LOG_MAX_BYTES)
        {
            self.log_event("WARN", &format!("could not write the link log: {e}"));
        }
    }

    /// The range of the link now: the newest reading and the evidence of
    /// the seconds before it. None without a recent reading.
    pub fn range_report(&self, now: Instant) -> Option<jafnify_core::wifi_range::RangeReport> {
        let sample = self.station_now(now)?;
        let now_ms = self.session_epoch_ms_at(now);
        let recent = self.station_ring.evidence(now_ms, now_ms);
        jafnify_core::wifi_range::RangeReport::of(&sample, &recent)
    }

    /// The newest reading of the Wi-Fi link, when it is recent.
    pub fn station_now(&self, now: Instant) -> Option<jafnify_core::wifi_range::StationSample> {
        self.station_ring
            .latest_since(self.session_epoch_ms_at(now) - 3_000)
    }

    /// The Wi-Fi level of the last few seconds, when there is one.
    pub fn range_now(&self, now: Instant) -> Option<i32> {
        let now_ms = self.session_epoch_ms_at(now);
        // An old level is no level: the tile goes when the readings stop.
        self.range_trend
            .latest_since(now_ms - 3_000)
            .map(|value| value as i32)
    }

    /// The ring of the Wi-Fi levels, for the range tile.
    pub fn range_trend(&self) -> &jafnify_core::tiles::TrendBuffer {
        &self.range_trend
    }

    /// Observe the uplink while Connected only. A transition updates the
    /// indication, the session counters and the connection log; it never calls
    /// `connection_lost`, touches `rx_watch` or changes the state machine, so
    /// the 35 s no-data drop and the 10 s retry stay exactly as they were.
    fn link_poll(&mut self, now: Instant) {
        if self.state() != ConnectionState::Connected {
            self.link_watch = None;
            self.link_lost = false;
            self.last_link_sample = None;
            return;
        }
        // Sample carrier + address at most once per interval; the first
        // Connected tick is immediate.
        if self
            .last_link_sample
            .is_some_and(|last| now.saturating_duration_since(last) < LINK_POLL_INTERVAL)
        {
            return;
        }
        self.last_link_sample = Some(now);
        // A loopback peer (or a loopback interface pin) has no uplink. Never
        // run `ip` for it; tests stay hermetic and the product does not label
        // an address that cannot carry the peer route.
        if self.link_iface.is_none()
            && (self.iface_pin.is_some_and(|ip| ip.is_loopback())
                || self
                    .peer_endpoint
                    .is_some_and(|endpoint| endpoint.ip().is_loopback()))
        {
            self.link_watch = None;
            self.link_lost = false;
            return;
        }
        if self.link_watch.is_none() {
            let iface = self
                .link_iface
                .clone()
                .or_else(|| self.detect_uplink_iface());
            let Some(iface) = iface else {
                return;
            };
            self.link_watch = Some(LinkWatch::new(iface.clone()));
            self.diag_event(
                now,
                "link_change",
                &[("reason", "interface"), ("iface", &iface)],
            );
        }
        let Some(iface) = self
            .link_watch
            .as_ref()
            .map(|watch| watch.iface().to_string())
        else {
            return;
        };
        let up = match &self.link_state {
            Some(state) => state(&iface).unwrap_or(false),
            // A named interface that cannot be inspected counts as down.
            None => read_uplink_state_with(&iface, &self.uplink_sysfs).unwrap_or(false),
        };
        let change = self.link_watch.as_mut().and_then(|watch| watch.observe(up));
        match change {
            Some(LinkChange::Lost) => {
                self.link_lost = true;
                self.session.link_down();
                self.connection_log.push(
                    "link",
                    &format!("link down ({iface})"),
                    self.session_epoch_ms_at(now),
                );
                self.diag_event(
                    now,
                    "link_change",
                    &[("reason", "carrier"), ("iface", &iface), ("carrier", "0")],
                );
                // A link that is merely down changes nothing: there is no
                // network either way, and a blip back to the same hotspot
                // rides through. LinkChange::Back decides.
            }
            Some(LinkChange::Back) => {
                self.link_lost = false;
                self.session.link_up();
                // "Back" may be another network: probe at once. The peer on
                // the same hotspot answers in milliseconds; silence for
                // LINK_BACK_ANSWER means another network (fallback_poll).
                self.last_probe = None;
                self.link_back_at = Some(now);
                self.connection_log.push(
                    "link",
                    &format!("link up ({iface})"),
                    self.session_epoch_ms_at(now),
                );
                self.diag_event(
                    now,
                    "link_change",
                    &[("reason", "carrier"), ("iface", &iface), ("carrier", "1")],
                );
            }
            None => {}
        }
    }

    /// The interface carrying the peer route, from `ip -4 route get` with the
    /// injected `ip`. Read-only, and only resolved while Connected.
    fn detect_uplink_iface(&self) -> Option<String> {
        let target = self.peer_endpoint?.ip().to_string();
        let mut cmd = Command::new(&self.ip_bin);
        cmd.args(["-4", "route", "get", &target]);
        let output = exec::output(cmd).ok()?;
        if !output.status.success() {
            return None;
        }
        parse_uplink_route(&String::from_utf8_lossy(&output.stdout))
    }

    /// Write "now" as the paired device's last connection (state file is
    /// atomic and 0600 as always). Without a stored pair there is nothing to
    /// stamp and nothing is written.
    fn stamp_last_connected(&mut self) {
        let Some(i) = self
            .state
            .peers
            .iter()
            .position(|p| p.private_key.is_some())
        else {
            return;
        };
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut state = self.state.clone();
        state.peers[i].last_connected = Some(unix);
        match save_state(&self.config_dir.join("state.toml"), &state) {
            Ok(()) => self.state = state,
            Err(e) => self.last_error = Some(format!("could not save the state: {e}")),
        }
    }

    /// A bring-up failed. A refused password prompt stops everything and waits
    /// for the user; a foreign tunnel blocks the attempt without touching it;
    /// any other failure takes the tunnel down and retries.
    fn bring_up_failed(&mut self, e: AppError, now: Instant) -> UiEvent {
        if matches!(e, AppError::PermissionDenied) {
            return self.permission_denied();
        }
        if matches!(e, AppError::ForeignTunnel) {
            return self.foreign_blocked();
        }
        let reason = e.to_string();
        self.log_event("ERROR", &format!("bring-up failed: {reason}"));
        self.connection_lost(reason.clone(), now, true);
        UiEvent::Disconnected { reason }
    }

    /// Back to Idle without asking for root again: nothing was brought up, so
    /// there is no `wg-quick down` (it would prompt once more). The code, or
    /// the known track, is kept for `try_again`.
    fn permission_denied(&mut self) -> UiEvent {
        self.clear_attempt();
        self.last_error = Some("Permission was denied".to_string());
        self.denied = true;
        UiEvent::PermissionDenied
    }

    /// Another instance owns `jafn0`: stop the attempt, keep the tunnel, and
    /// let the card show the plain foreign status.
    fn foreign_blocked(&mut self) -> UiEvent {
        self.clear_attempt();
        self.owns_tunnel = false;
        self.foreign_tunnel = true;
        self.denied = false;
        self.last_error = None;
        UiEvent::Disconnected {
            reason: FOREIGN_TUNNEL.to_string(),
        }
    }

    /// Drop the local half of a failed attempt without a root call.
    fn clear_attempt(&mut self) {
        let _ = std::fs::remove_file(self.config_dir.join(format!("{IFACE}.conf")));
        self.orchestrator = Orchestrator::new();
        self.discovery = None;
        self.handshake = None;
        self.pake = None;
        self.known = None;
        self.adopting = false;
        self.keypair = None;
        self.peer_public_key = None;
        self.peer_endpoint = None;
        self.last_announce = None;
        self.rx_watch = None;
        self.connecting_since = None;
        self.auto_accept_deadline = None;
        self.end_firewall_check();
        self.polkit_prompt_expected = false;
    }

    /// An adopted tunnel is not Connected until its rx counter moves. If the
    /// peer stays silent for CONNECT_TIMEOUT, tear the stale interface down and
    /// continue on the normal known-peer path.
    fn tick_adopted(&mut self, now: Instant) -> Option<UiEvent> {
        if self.state() != ConnectionState::Connecting {
            self.adopting = false;
            return None;
        }
        let since = *self.connecting_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= self.connect_timeout() {
            return Some(self.adopted_timeout(now));
        }
        self.rx_advanced().then(|| self.known_connected(now))
    }

    fn adopted_timeout(&mut self, now: Instant) -> UiEvent {
        self.adopting = false;
        let reason = format!(
            "no data from peer within {} s",
            self.connect_timeout().as_secs()
        );
        self.log_event("WARN", &format!("adopted tunnel timed out: {reason}"));
        match self.run_teardown(true) {
            Ok(()) => match self.lan_ifaces() {
                Ok(ifaces) => match self.open_known_search(ifaces, now) {
                    Ok(()) => UiEvent::Disconnected { reason },
                    Err(e) => {
                        let reason = e.to_string();
                        self.connection_lost(reason.clone(), now, false);
                        UiEvent::Disconnected { reason }
                    }
                },
                Err(e) => {
                    let reason = e.to_string();
                    self.connection_lost(reason.clone(), now, false);
                    UiEvent::Disconnected { reason }
                }
            },
            Err(AppError::PermissionDenied) => self.permission_denied(),
            Err(e) => {
                let reason = e.to_string();
                self.connection_lost(reason.clone(), now, false);
                UiEvent::Disconnected { reason }
            }
        }
    }

    /// Known track (stored pair, no code): Hello/Proof, then the tunnel with
    /// the stored key. The connect timeout and loss rules are the code
    /// track's.
    fn tick_known(&mut self, now: Instant) -> Option<UiEvent> {
        let state = self.state();
        if !matches!(
            state,
            ConnectionState::Searching | ConnectionState::Connecting | ConnectionState::Connected
        ) {
            return None;
        }
        // Searching follows the network that leads to the phone: an address
        // that came back (or a new one) reopens the track instead of leaving
        // it pinned to the network that existed while the hotspot was away.
        if state == ConnectionState::Searching {
            self.recheck_search_ifaces(now);
        }
        // A sharer serves from the start; its endpoint is learned from the
        // first authenticated WireGuard packet.
        if state == ConnectionState::Searching
            && self.settings.role == Role::Sharer
            && self.peer_public_key.is_none()
        {
            self.peer_public_key = self.stored_peer_pk();
            if let Err(e) = self.bring_up_tunnel() {
                return Some(self.bring_up_failed(e, now));
            }
        }
        let old_peer = self.known.as_ref().is_some_and(|t| t.old_peer_seen());
        if old_peer
            && matches!(
                state,
                ConnectionState::Searching | ConnectionState::Connecting
            )
        {
            let touched = self.peer_public_key.is_some();
            self.connection_lost(OLD_PEER_MESSAGE.to_string(), now, touched);
            return Some(UiEvent::Disconnected {
                reason: OLD_PEER_MESSAGE.to_string(),
            });
        }
        let udp_before = jafnify_core::wire::stats();
        let polled = {
            let track = self.known.as_mut()?;
            track.set_connected(state == ConnectionState::Connected);
            track.poll_detailed(now)
        };
        let polled = match polled {
            Ok(poll) => poll,
            Err(e) => return Some(self.known_lost(e.to_string(), now, false)),
        };
        let no_network = self
            .known
            .as_ref()
            .and_then(|t| t.no_network().map(str::to_string));
        match no_network {
            // A live session with no network is lost, as before.
            Some(text) if state != ConnectionState::Searching => {
                return Some(self.known_lost(
                    format!("known-peer socket error: {text}"),
                    now,
                    false,
                ));
            }
            // Searching: wait for the network quietly; one line, not a Retry
            // every few seconds that would flood the log.
            Some(text) => {
                self.note_no_network(&text, now);
                self.nudge_wifi_scan(now);
                self.refresh_known_track_if_due(now);
            }
            None => self.note_network_back(now),
        }
        self.diag_known_tx(now, udp_before);
        let proof_from = polled.endpoint;
        if proof_from.is_some() {
            self.last_peer_proof = Some(now);
        }
        // A Proof is the peer itself answering: it is back. It can arrive in
        // any known-track state (Retry included). Probe through jafn0 at once;
        // the routing returns when the peer's data does (fallback_poll).
        if self.routing_down && proof_from.is_some() {
            self.last_probe = None;
        }
        match state {
            ConnectionState::Searching => {
                if polled.peer_fresh {
                    self.peer_wg_fresh = true;
                }
                if let Some(from) = proof_from {
                    if self.peer_public_key.is_none() {
                        // Joiner: the sharer's WireGuard endpoint, from its Proof.
                        self.peer_public_key = self.stored_peer_pk();
                        self.peer_endpoint = Some(from);
                        if let Err(e) = self.bring_up_tunnel() {
                            return Some(self.bring_up_failed(e, now));
                        }
                    }
                    self.orchestrator.known_peer_found();
                    self.connecting_since = None;
                    return Some(UiEvent::PeerFound);
                }
                // WireGuard data from the stored peer needs no Proof.
                if self.peer_public_key.is_some() && self.rx_advanced() {
                    return Some(self.known_connected(now));
                }
            }
            ConnectionState::Connecting => {
                if polled.peer_fresh {
                    self.peer_wg_fresh = true;
                    if self.tunnel_reused && self.peer_public_key.is_some() {
                        // The kept jafn0 holds the peer's old session keys;
                        // the restarted peer cannot read them. Set it up again
                        // now instead of waiting for WireGuard's own rekey.
                        if let Err(e) = self.bring_up_tunnel() {
                            return Some(self.bring_up_failed(e, now));
                        }
                    }
                }
                let since = *self.connecting_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= self.connect_timeout() {
                    if self.adopting {
                        // An adopted tunnel that never carried data: take it
                        // down and search afresh, as before the track ran.
                        return Some(self.adopted_timeout(now));
                    }
                    let reason = format!(
                        "no data from peer within {} s",
                        self.connect_timeout().as_secs()
                    );
                    return Some(self.known_lost(reason, now, true));
                }
                if self.rx_advanced() {
                    return Some(self.known_connected(now));
                }
            }
            _ => {
                let connected = state == ConnectionState::Connected;
                if connected && polled.peer_bye {
                    return Some(self.peer_said_bye(now));
                }
                if connected && polled.peer_restarted {
                    self.peer_restarted(now);
                }
                let (advanced, flat) = self.rx_flat(now);
                if advanced {
                    self.clear_reset_watch(now);
                }
                // The reset hint only matters while Connected: the previous
                // state's Proof is a normal part of reconnecting.
                if polled.session_reset.is_some() && !advanced {
                    self.note_peer_reset(now);
                }
                if self.reset_neutral && self.reset_deadline.is_some_and(|deadline| now >= deadline) {
                    let at_ms = self.reset_neutral_at.take();
                    self.reset_neutral = false;
                    let reason = format!(
                        "peer session reset; no data for {} s",
                        RESET_NEUTRAL_GRACE.as_secs()
                    );
                    return Some(self.known_lost_after_reset_at(reason, at_ms, now));
                }
                if !self.reset_probe_sent && flat >= RESET_PROBE_AFTER {
                    let probed = self.known.as_mut().map_or(Ok(()), |track| track.probe(now));
                    if let Err(e) = probed {
                        return Some(self.known_lost(e.to_string(), now, false));
                    }
                    self.reset_probe_sent = true;
                    self.log_event("DEBUG", "peer data flat; known-peer probe sent");
                    self.diag_event(now, "probe", &[("source", "known"), ("reason", "flat_rx")]);
                }
                if flat >= RX_STALL_TIMEOUT {
                    let reason = format!("no data from peer for {} s", RX_STALL_TIMEOUT.as_secs());
                    self.diag_no_data(now, &reason);
                    return Some(self.known_lost(reason, now, false));
                }
            }
        }
        None
    }

    /// The peer stopped its session (Bye). Drop at once on the short retry,
    /// and as a joiner hand the routing back now instead of after the
    /// liveness timeout, so the laptop stays online.
    /// The peer said goodbye: it stopped sharing. The outage is a stop on the
    /// phone and not a fault of the link.
    pub fn peer_said_bye(&mut self, now: Instant) -> UiEvent {
        let reason = "peer disconnected".to_string();
        self.diag_event(now, "peer_bye", &[]);
        // The peer's WireGuard state is gone with it: never reuse jafn0's
        // session keys for the next connection.
        self.peer_wg_fresh = true;
        let joiner = self.settings.role == jafnify_core::state::Role::Joiner;
        if joiner && !self.routing_down {
            self.fall_back_routing();
        }
        // The phone stopped sharing on purpose: the outage is a stop on the
        // phone, not a fault of the link.
        self.peer_stopped = true;
        self.connection_lost_with_retry(reason.clone(), now, false, RETRY_AFTER_RESET);
        self.peer_stopped = false;
        UiEvent::Disconnected { reason }
    }

    /// The peer announced a new session while this side still counted the
    /// old one: the connected clock and the session record start again.
    fn peer_restarted(&mut self, now: Instant) {
        self.log_event("INFO", "peer restarted: new session");
        self.diag_event(now, "peer_restart", &[]);
        let now_ms = self.session_epoch_ms_at(now);
        self.session.peer_restart(now_ms, "Peer restarted");
        self.connected_since = Some(now);
        self.conn_clock.restart(self.clock_ms(now));
        self.clear_reset_watch(now);
        // A peer that announced a new session may have done so because its
        // app was replaced (or it crashed). Keep the restart for the word of
        // the phone's first summary, even when no reset Proof was seen.
        if self.recovery.is_none() {
            self.recovery = Some(Recovery {
                at_ms: now_ms,
                back_ms: now_ms,
                until: now + EXIT_GRACE,
                recorded_drop: false,
            });
        }
    }

    fn known_connected(&mut self, now: Instant) -> UiEvent {
        self.peer_wg_fresh = false;
        self.orchestrator.confirm_connected();
        self.adopting = false;
        self.connecting_since = None;
        self.rx_watch = None;
        self.reconnect_rx = None;
        self.clear_reset_watch(now);
        self.polkit_prompt_expected = false;
        let endpoint = self
            .peer_endpoint
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let reconnected = self
            .disconnected_at
            .take()
            .map(|t| now.saturating_duration_since(t).as_secs());
        match reconnected {
            Some(secs) => self.log_event(
                "INFO",
                &format!("reconnected after {secs} s (peer {endpoint})"),
            ),
            None => self.log_event("INFO", &format!("connected (peer {endpoint})")),
        }
        self.sample_session(now);
        // The connection log closes the drop on the way back to Connected:
        // the tick writes the "reconnected at" line there, and that line
        // carries no endpoint. The journal line above keeps the peer.
        self.close_drop(now);
        UiEvent::Connected
    }

    fn known_lost(&mut self, reason: String, now: Instant, teardown: bool) -> UiEvent {
        self.connection_lost(reason.clone(), now, teardown);
        UiEvent::Disconnected { reason }
    }

    /// A peer session reset with no data inside the neutral window: drop on
    /// the reset-specific 2 s retry instead of the ordinary 10 s one. The
    /// drop keeps the start of the outage, not the moment the window closed.
    fn known_lost_after_reset_at(
        &mut self,
        reason: String,
        at_ms: Option<i64>,
        now: Instant,
    ) -> UiEvent {
        self.diag_peer_reset(now, "no_data_after_grace");
        // The window is over; the drop takes over from here.
        self.reset_neutral = false;
        self.reset_neutral_at = None;
        self.reset_deadline = None;
        self.connection_lost_with_retry_at(reason.clone(), at_ms, now, false, RETRY_AFTER_RESET);
        UiEvent::Disconnected { reason }
    }

    /// Pairing track (code): 0x01 discovery, 0x02 key exchange, tunnel.
    fn tick_code(&mut self, now: Instant) -> Result<Option<UiEvent>, AppError> {
        let mut event = None;
        // A version-1 handshake from the peer: it can never pair with this build.
        let old_peer = match (&self.handshake, &self.local_code) {
            (Some(h), Some(code)) => h.old_peer_seen(code),
            _ => false,
        } || self.pake.as_ref().is_some_and(|p| p.old_peer_seen());
        if old_peer
            && matches!(
                self.state(),
                ConnectionState::Searching | ConnectionState::Pairing | ConnectionState::Connecting
            )
        {
            let touched = self.peer_public_key.is_some();
            self.connection_lost(OLD_PEER_MESSAGE.to_string(), now, touched);
            return Ok(Some(UiEvent::Disconnected {
                reason: OLD_PEER_MESSAGE.to_string(),
            }));
        }
        match self.state() {
            ConnectionState::Connecting => {
                // Connecting lasts until the first WireGuard data arrives from
                // the peer, not until its Confirm does: the frame only says the
                // peer exists, rx > 0 says the tunnel works.
                let since = *self.connecting_since.get_or_insert(now);
                let tunnel_up = self.peer_public_key.is_some();
                // Code track: the tunnel is brought up fresh, so its routing
                // already sends into jafn0; the plain timeout applies.
                if now.saturating_duration_since(since) >= CONNECT_TIMEOUT {
                    let reason =
                        format!("no data from peer within {} s", CONNECT_TIMEOUT.as_secs());
                    self.connection_lost(reason.clone(), now, tunnel_up);
                    return Ok(Some(UiEvent::Disconnected { reason }));
                }
                let poll = match self.pake.as_mut() {
                    Some(pake) => pake.poll(now)?,
                    None => return Ok(None),
                };
                if poll.old_peer {
                    let touched = self.peer_public_key.is_some();
                    self.connection_lost(OLD_PEER_MESSAGE.to_string(), now, touched);
                    return Ok(Some(UiEvent::Disconnected {
                        reason: OLD_PEER_MESSAGE.to_string(),
                    }));
                }
                let should_log = self
                    .last_announce
                    .map(|t| now.duration_since(t) >= ANNOUNCE_INTERVAL)
                    .unwrap_or(true);
                if should_log {
                    self.last_announce = Some(now);
                    let now_ms = self.session_epoch_ms_at(now);
                    if let Some(diag) = self.diag.as_mut() {
                        let _ =
                            diag.event(now_ms, "probe", &[("source", "pake"), ("frame", "0x05")]);
                    }
                }
                if poll.wrong_code && !self.pake_wrong_code_logged {
                    self.pake_wrong_code_logged = true;
                    let now_ms = self.session_epoch_ms_at(now);
                    if let Some(diag) = self.diag.as_mut() {
                        let _ = diag.event(
                            now_ms,
                            "probe",
                            &[("source", "pake"), ("frame", "wrong-code")],
                        );
                    }
                }
                if let Some(peer) = poll.peer {
                    if !tunnel_up {
                        self.peer_public_key =
                            Some(base64::engine::general_purpose::STANDARD.encode(peer.public_key));
                        self.peer_endpoint = Some(peer.endpoint);
                        match self.bring_up_tunnel() {
                            // Fresh window: bring-up may have waited on a polkit
                            // prompt.
                            Ok(()) => self.connecting_since = None,
                            Err(e) => event = Some(self.bring_up_failed(e, now)),
                        }
                    }
                }
                if tunnel_up && self.rx_advanced() {
                    self.orchestrator.confirm_connected();
                    self.connecting_since = None;
                    self.rx_watch = None;
                    // Traffic proves the tunnel: only now remember the pair.
                    self.persist_pairing();
                    event = Some(UiEvent::Connected);
                }
            }
            state @ (ConnectionState::Searching | ConnectionState::Pairing) => {
                // Both ends keep announcing while Pairing so the other side
                // can still find us.
                self.announce_discovery(now)?;
                if state == ConnectionState::Pairing
                    && self.auto_accept_deadline.is_some_and(|d| now >= d)
                {
                    self.auto_accept_deadline = None;
                    let code = self.local_code.as_ref().map(|c| c.as_str().to_string());
                    if let Some(code) = code {
                        self.accept_code(&code)?;
                    }
                }
                if state == ConnectionState::Searching {
                    if let Some(d) = &self.discovery {
                        if let Some((from, token)) = d.try_receive(Duration::ZERO)? {
                            if self.orchestrator.on_discovery_event(&token) {
                                d.learn(from);
                                self.auto_accept_deadline = Some(now + self.auto_accept_delay);
                                event = Some(UiEvent::PeerFound);
                            }
                        }
                    }
                }
            }
            ConnectionState::Connected => {
                if let Some(reason) = self.rx_stalled(now) {
                    self.diag_no_data(now, &reason);
                    self.connection_lost(reason.clone(), now, false);
                    event = Some(UiEvent::Disconnected { reason });
                }
                // Counters for the card come from the interface's sysfs files
                // (`read_rx`, `App::traffic`), which need no privileges. A
                // `wg show` failure is not an error here.
            }
            _ => {}
        }
        Ok(event)
    }

    /// Store this pair (own private key, peer key, endpoint hint) after the
    /// first rx > 0 of a code pairing; it replaces any earlier device. The
    /// loaded settings are written back unchanged. From now on a retry
    /// searches on the known track.
    fn persist_pairing(&mut self) {
        let (Some(keypair), Some(peer_pk)) = (&self.keypair, &self.peer_public_key) else {
            return;
        };
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let endpoint = self.peer_endpoint.map(|e| e.to_string());
        let rec = match pair_record(keypair, peer_pk, self.settings.role, endpoint, unix) {
            Ok(rec) => rec,
            Err(e) => {
                self.last_error = Some(format!("could not save the pairing: {e}"));
                return;
            }
        };
        let mut state = self.state.clone();
        state.peers = vec![rec];
        match save_state(&self.config_dir.join("state.toml"), &state) {
            Ok(()) => {
                self.state = state;
                self.use_known = true;
                if self
                    .last_error
                    .as_deref()
                    .is_some_and(is_pairing_save_error)
                {
                    self.last_error = None;
                }
            }
            Err(e) => self.last_error = Some(format!("could not save the pairing: {e}")),
        }
    }

    /// Remove a tunnel, config or kill-switch rules left behind by a crashed
    /// daemon. A tunnel that the stored pair can adopt is left for
    /// `start_known`; a tunnel owned by another instance is never touched.
    pub fn cleanup_stale_state(&mut self) -> Result<(), AppError> {
        let tunnel = self.interface_present();
        if tunnel {
            match self.assess_ownership() {
                // Another instance owns jafn0: leave everything alone.
                TunnelOwner::Foreign => return Ok(()),
                // Ours: a tunnel this pair can adopt is left for start_known;
                // a stale one from a forgotten pair is torn down below.
                TunnelOwner::Ours => {}
                TunnelOwner::None => {}
            }
        }
        let adoptable = jafnify_core::terms::is_accepted(&self.config_dir) && self.is_paired();
        if tunnel && adoptable {
            return Ok(());
        }
        self.run_teardown(tunnel)
    }

    /// Tell a connected peer this side is stopping, so it ends its session
    /// at once instead of after the data-stall timeout. Best effort.
    pub fn say_bye(&mut self) {
        if !matches!(
            self.state(),
            ConnectionState::Connected | ConnectionState::Connecting
        ) {
            return;
        }
        if let Some(track) = self.known.as_mut() {
            match track.send_bye() {
                Ok(()) => self.log_event("INFO", "bye sent to the peer"),
                Err(e) => self.log_event("WARN", &format!("bye not sent: {e}")),
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), AppError> {
        self.log_event("INFO", "stop requested");
        self.say_bye();
        self.polkit_prompt_expected = false;
        // Sample the final counters before teardown removes the interface.
        let now = Instant::now();
        self.sample_session(now);
        // Never trust `wg show` here (unprivileged users get NotFound). The
        // interface's own sysfs counter needs no root, though: with no `jafn0`
        // there is nothing to tear down, and asking for the password (a polkit
        // prompt) for nothing is wrong, e.g. on Cancel while still Searching.
        let tunnel = self.tunnel_present();
        let teardown = if tunnel && self.interface_present() && !self.owns_tunnel_now() {
            // Another instance owns jafn0: end our session, touch nothing.
            Ok(())
        } else {
            self.run_teardown(tunnel)
        };
        self.end_session(now);
        let candidate = self.resume_candidate_at(now);
        self.reset_session(now);
        self.resume_candidate = candidate;
        match teardown {
            Ok(()) => Ok(()),
            Err(AppError::PermissionDenied) => {
                self.last_error = Some(TEARDOWN_DENIED.to_string());
                self.denied = false;
                Err(AppError::PermissionDenied)
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                Ok(())
            }
        }
    }

    /// The first failed send with no network while searching: say so once.
    fn note_no_network(&mut self, text: &str, now: Instant) {
        if self.no_network_since.is_some() {
            return;
        }
        self.no_network_since = Some(now);
        self.no_network_refresh_at = Some(now);
        self.log_event(
            "WARN",
            &format!("no network ({text}); waiting for it to return"),
        );
        self.connection_log.push(
            "network",
            connection_log::NO_NETWORK,
            self.session_epoch_ms_at(now),
        );
    }

    /// While a joiner waits for the network, ask NetworkManager to scan so
    /// a hotspot back in range is rejoined in seconds, not after its 120 s
    /// scan backoff. A sharer provides the network and never asks.
    fn nudge_wifi_scan(&mut self, now: Instant) {
        if self.settings.role != Role::Joiner {
            return;
        }
        if self.wifi_scan.nudge_if_due(now) {
            self.log_event(
                "DEBUG",
                "no network: asked NetworkManager to scan for Wi-Fi",
            );
        }
    }

    /// Test seam: wait for a scan request in flight to finish.
    pub fn wait_wifi_scan_idle(&self, timeout: Duration) -> bool {
        self.wifi_scan.wait_idle(timeout)
    }

    /// A send went out again after a no-network wait.
    fn note_network_back(&mut self, now: Instant) {
        let Some(since) = self.no_network_since.take() else {
            return;
        };
        self.wifi_scan.reset();
        self.no_network_refresh_at = None;
        let secs = now.saturating_duration_since(since).as_secs();
        self.log_event("INFO", &format!("network back after {secs} s"));
        self.connection_log.push(
            "network",
            &connection_log::network_back_line(secs),
            self.session_epoch_ms_at(now),
        );
    }

    /// While waiting for the network, open the known track again every
    /// `RETRY_INTERVAL` on the LAN addresses this computer has now. Silent: no
    /// state change, no log line. Nothing to open while no LAN address exists.
    fn refresh_known_track_if_due(&mut self, now: Instant) {
        let due = self
            .no_network_refresh_at
            .is_none_or(|at| now.saturating_duration_since(at) >= RETRY_INTERVAL);
        if !due {
            return;
        }
        self.no_network_refresh_at = Some(now);
        let ifaces = self.current_lan_ifaces();
        if ifaces.is_empty() {
            return;
        }
        self.reopen_known_track(ifaces, now);
    }

    /// A searching known track follows the network. With a second interface
    /// present, a search opened while the hotspot was away is pinned to that
    /// other network: its sends still succeed (multicast loopback), so neither
    /// the no-network path nor a retry ever revisits the choice. Compare the
    /// pinned list with the addresses this computer has now, at the retry pace,
    /// and reopen only when the list really changed (a returning hotspot adds
    /// its address and puts it first by the peer's subnet).
    fn recheck_search_ifaces(&mut self, now: Instant) {
        let due = self
            .search_recheck_at
            .is_none_or(|at| now.saturating_duration_since(at) >= RETRY_INTERVAL);
        if !due {
            return;
        }
        self.search_recheck_at = Some(now);
        // With no usable network at all the no-network path owns the refresh;
        // it also reopens the same way.
        if self.no_network_since.is_some() {
            return;
        }
        let current = self.current_lan_ifaces();
        if current.is_empty() || current == self.search_ifaces {
            return;
        }
        self.log_event(
            "INFO",
            "search network changed; following the new address list",
        );
        self.diag_event_fields(
            now,
            "search_ifaces",
            &[
                ("from", join_addrs(&self.search_ifaces)),
                ("to", join_addrs(&current)),
            ],
        );
        self.reopen_known_track(current, now);
    }

    /// A new known track on `ifaces` (same stored pair and key), replacing the
    /// current one. Keeps the state and the session as they are.
    fn reopen_known_track(&mut self, ifaces: Vec<std::net::Ipv4Addr>, now: Instant) {
        let (Some(peer), Some(keypair)) = (stored_pair(&self.state.peers), self.keypair.as_ref())
        else {
            return;
        };
        let cfg = DiscoveryConfig {
            unicast: self.joiner_gateways(&ifaces),
            ifaces: ifaces.clone(),
            ..Default::default()
        };
        match KnownTrack::open(&cfg, keypair, &peer.public_key, self.settings.role, now) {
            Ok(mut track) => {
                track.fail_sends(self.known_send_fault);
                self.known = Some(track);
                self.search_ifaces = ifaces;
                self.search_recheck_at = None;
            }
            Err(e) => self.log_event("DEBUG", &format!("known track not reopened: {e}")),
        }
    }

    /// The LAN addresses this computer has right now, with no side effect (no
    /// error text): the pinned one, else the detected ones; empty when none.
    /// A joiner's list is ordered by the saved peer's subnet (see
    /// `jafnify_core::route::joiner_search_ifaces`); the peer's last endpoint
    /// is only a hint, never a filter.
    fn current_lan_ifaces(&self) -> Vec<std::net::Ipv4Addr> {
        self.search_addresses()
    }

    /// The address the saved pair last reached the peer at, when known.
    fn peer_hint_ip(&self) -> Option<std::net::Ipv4Addr> {
        self.stored_peer()
            .and_then(|peer| peer.last_endpoint)
            .and_then(|endpoint| endpoint.parse::<SocketAddr>().ok())
            .and_then(|endpoint| match endpoint.ip() {
                std::net::IpAddr::V4(v4) => Some(v4),
                std::net::IpAddr::V6(_) => None,
            })
    }

    /// The search addresses in order: the pin, else every up LAN address with
    /// the peer's subnet first for a joiner. Empty when none is usable.
    fn search_addresses(&self) -> Vec<std::net::Ipv4Addr> {
        if let Some(pin) = self.iface_pin {
            return vec![pin];
        }
        if self.settings.role == Role::Sharer {
            let all = detect_sharer_ifaces(&self.ip_bin);
            if !all.is_empty() {
                return all;
            }
        } else {
            let all = detect_lan_ifaces(&self.ip_bin);
            let ordered = jafnify_core::route::joiner_search_ifaces(&all, self.peer_hint_ip());
            if !ordered.is_empty() {
                return ordered;
            }
        }
        detect_lan(&self.ip_bin)
            .map(|(_, ip, _)| vec![ip])
            .unwrap_or_default()
    }

    /// Mark the start of a new run in the connection log. Earlier runs stay
    /// above it (and on disk, when the log is persistent).
    fn begin_run(&mut self, now: Instant) {
        let role = role_name(self.settings.role);
        self.connection_log.push(
            "session",
            &connection_log::session_started_line(role),
            self.session_epoch_ms_at(now),
        );
    }

    /// The finished session a Stop leaves behind for a quick Connect. Only a
    /// paired device can come back on the known track.
    fn resume_candidate_at(&self, now: Instant) -> Option<ResumeCandidate> {
        let peer = stored_pair(&self.state.peers)?;
        Some(ResumeCandidate {
            tracker: self.session.clone(),
            stopped_at_ms: self.session_epoch_ms_at(now),
            peer_key: peer.public_key.clone(),
        })
    }

    /// Continue the stopped session when the same device is connected again
    /// inside the grace of the core. True when it was continued.
    fn try_resume(&mut self, now: Instant) -> bool {
        let Some(candidate) = self.resume_candidate.take() else {
            return false;
        };
        let same_peer =
            stored_pair(&self.state.peers).is_some_and(|p| p.public_key == candidate.peer_key);
        let now_ms = self.session_epoch_ms_at(now);
        if !jafnify_core::scope::resumes(candidate.stopped_at_ms, now_ms, same_peer) {
            return false;
        }
        let mut tracker = candidate.tracker;
        tracker.resume();
        self.session_history.take_newest(tracker.id());
        self.session = tracker;
        self.last_live_save = Some(now);
        if self.session_history_owner {
            let stats = self.session.current();
            self.session_history.set_live(stats);
            if let Err(e) = self.session_history.save(&self.sessions_path) {
                self.last_error = Some(format!("could not save the session: {e}"));
            }
        }
        let role = role_name(self.settings.role);
        self.connection_log.push(
            "session",
            &connection_log::session_resumed_line(role),
            now_ms,
        );
        true
    }

    /// Clear the live session fields without touching the stored pair.
    fn reset_session(&mut self, now: Instant) {
        self.resume_candidate = None;
        self.no_network_since = None;
        self.no_network_refresh_at = None;
        self.auto_accept_deadline = None;
        self.orchestrator = Orchestrator::new();
        self.discovery = None;
        self.handshake = None;
        self.pake = None;
        self.known = None;
        self.adopting = false;
        self.use_known = false;
        self.keypair = None;
        self.local_code = None;
        self.peer_public_key = None;
        self.peer_endpoint = None;
        self.last_announce = None;
        self.rx_watch = None;
        self.link_watch = None;
        self.link_lost = false;
        self.last_link_sample = None;
        self.reconnect_rx = None;
        self.clear_reset_watch(now);
        self.search_ifaces.clear();
        self.search_recheck_at = None;
        self.end_firewall_check();
        self.suppress_job = None;
        self.last_suppress_check = None;
        self.suppress_retry_at = None;
        self.suppress_restore_pending = false;
        self.connecting_since = None;
        self.connected_since = None;
        self.conn_clock.disconnected();
        self.denied = false;
        self.polkit_prompt_expected = false;
        self.last_peer_rx = None;
        self.last_peer_proof = None;
        self.last_diag_stats = None;
        self.last_diag_rx = None;
        self.handshake_age = HandshakeAge::Unknown;
        self.handshake_job = None;
        self.last_handshake_query = None;
        self.diag_egress = EgressSnapshot::default();
        // An outage still open when the session ends is closed here: the
        // stop, the forget or the restart ends it, and a record that is never
        // closed would read "not back yet" for ever. A reset or a loss of
        // data keeps its short wait for the word of the phone, so an app
        // update that came with a stop is still told apart from a drop.
        let quiet = self.away_quiet;
        self.close_drop(now);
        self.away_quiet = quiet;
        let recovery = self.recovery.take();
        let role = role_name(self.settings.role);
        self.session = SessionTracker::new(role, self.session_epoch_ms_at(now));
        self.last_live_save = None;
        self.diag_session_start(now);
        self.last_traffic = None;
        // A new session starts without an open outage: a stop, a forget or a
        // restart is not a drop.
        self.drop_open = None;
        self.recovery = recovery;
    }

    /// The root steps of `teardown_plan`. A refused prompt is returned to the
    /// caller; the marker is cleared only after the interface is really gone.
    fn run_teardown(&mut self, tunnel: bool) -> Result<(), AppError> {
        let guard = detect_guard();
        let kill = self.settings.kill_switch && guard != InboundGuard::None;
        let mut first_error = None;
        for step in teardown_plan(tunnel, self.installed_conf_present(), kill) {
            let cmd = match step {
                Teardown::WgDown => self.root.down_command(),
                Teardown::RemoveConf => self.root.remove_conf_command_fixed(),
                Teardown::KillSwitchDown => self
                    .root
                    .kill_switch_down_command(guard == InboundGuard::Iptables),
            };
            match self.run_root("teardown", cmd) {
                Ok(()) => {}
                Err(AppError::PermissionDenied) => {
                    if !self.tunnel_present() {
                        self.clear_marker();
                    }
                    return Err(AppError::PermissionDenied);
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if !self.tunnel_present() {
            self.clear_marker();
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Retry -> Searching: reopen the search. If that fails (interface gone),
    /// stay in the 10 s retry cycle.
    fn restart_search(&mut self, now: Instant) -> Option<UiEvent> {
        // The addresses this computer has now: the network may have come back
        // with a new one. With none at all, a known search keeps its track and
        // waits quietly for the network (tick_known).
        let current = self.current_lan_ifaces();
        let ifaces = if !current.is_empty() {
            current
        } else if self.use_known && self.known.is_some() {
            return None;
        } else if self.search_ifaces.is_empty() {
            self.lan_ifaces().ok()?
        } else {
            self.search_ifaces.clone()
        };
        let reopened = if self.use_known {
            self.open_known_search(ifaces, now)
        } else {
            let code = self.local_code.clone()?;
            self.open_search(code, ifaces)
        };
        match reopened {
            Ok(()) => None,
            Err(e) => {
                let reason = e.to_string();
                self.orchestrator.on_disconnect_at(reason.clone(), now);
                Some(UiEvent::Disconnected { reason })
            }
        }
    }

    /// Observe the rx counter for the reset/probe rules: whether it advanced
    /// this tick, and how long it has been flat. A missing counter is no
    /// progress. The first observation only establishes the baseline.
    fn rx_flat(&mut self, now: Instant) -> (bool, Duration) {
        let rx = self.read_rx();
        let Some(watch) = &mut self.rx_watch else {
            self.rx_watch = Some(RxWatch::new(now, rx));
            return (false, Duration::ZERO);
        };
        let advanced = watch.advanced(now, rx);
        (advanced, watch.unchanged_for(now, rx))
    }

    /// A valid Proof carried a peer nonce this track never saw. Report it once;
    /// data still flowing clears it on the next tick.
    fn note_peer_reset(&mut self, now: Instant) {
        if self.peer_reset_hint {
            return;
        }
        self.peer_reset_hint = true;
        self.reset_neutral = true;
        self.reset_neutral_at = Some(self.session_epoch_ms_at(now));
        self.reset_deadline = Some(now + RESET_NEUTRAL_GRACE);
        self.log_event(
            "WARN",
            &format!(
                "peer session reset suspected; reconnecting for {} s",
                RESET_NEUTRAL_GRACE.as_secs()
            ),
        );
        self.diag_peer_reset(now, "proof_nonce");
    }

    /// The peer came back inside the neutral window: the outage was not a
    /// drop by itself. Keep it for the phone's word, which arrives with the
    /// first summary of the new session and may still say it was an update,
    /// a crash, or nothing at all (an old app).
    fn finish_reset_neutral(&mut self, now: Instant) {
        let Some(at_ms) = self.reset_neutral_at.take() else {
            return;
        };
        self.reset_neutral = false;
        self.recovery = Some(Recovery {
            at_ms,
            back_ms: self.session_epoch_ms_at(now),
            until: now + EXIT_GRACE,
            recorded_drop: false,
        });
        self.log_event("INFO", "reconnecting: the peer is back; waiting for its word");
    }

    fn clear_reset_watch(&mut self, now: Instant) {
        self.finish_reset_neutral(now);
        self.peer_reset_hint = false;
        self.reset_deadline = None;
        self.reset_probe_sent = false;
    }

    /// Reason string if the tunnel's rx counter has not advanced for
    /// RX_STALL_TIMEOUT. An unreadable counter (interface gone) is no progress.
    fn rx_stalled(&mut self, now: Instant) -> Option<String> {
        let rx = self.read_rx();
        let Some(watch) = &mut self.rx_watch else {
            self.rx_watch = Some(RxWatch::new(now, rx));
            return None;
        };
        watch
            .stalled(now, rx)
            .then(|| format!("no data from peer for {} s", RX_STALL_TIMEOUT.as_secs()))
    }

    /// A peer's first post-reconnect data, not merely a counter left nonzero by
    /// the previous connection. A fresh interface starts its counter at zero.
    fn rx_advanced(&self) -> bool {
        match (self.reconnect_rx, self.read_rx()) {
            (Some(base), Some(now)) => now > base,
            (None, Some(now)) => now > 0,
            _ => false,
        }
    }

    /// `jafn0` exists right now (its sysfs counter is there; unprivileged).
    fn tunnel_present(&self) -> bool {
        self.rx_bytes_path.exists()
    }

    /// The interface itself exists. This is the ownership signal: a test's
    /// injected counter file is not mistaken for another instance's tunnel.
    fn interface_present(&self) -> bool {
        self.root.teardown_iface.exists()
    }

    /// The paired device's public key, when this instance has a pair.
    fn stored_peer_key(&self) -> Option<String> {
        self.stored_peer().map(|peer| peer.public_key)
    }

    /// Who owns `jafn0` right now. The live peer key is read through the
    /// read-only helper at most once per tunnel presence, and only when it is
    /// needed: an unpaired instance with a marker must compare that marker
    /// with the live key, so the marker can prove an orphan this config
    /// directory created (`jafnify stop` after an interrupted join). An
    /// unpaired instance without a marker makes no privileged call at all.
    fn assess_ownership(&mut self) -> TunnelOwner {
        if !self.interface_present() {
            self.owns_tunnel = false;
            self.foreign_tunnel = false;
            self.live_peer_key = None;
            self.live_peer_key_checked = false;
            return TunnelOwner::None;
        }
        if self.owns_tunnel {
            self.foreign_tunnel = false;
            return TunnelOwner::Ours;
        }
        let ours = self.stored_peer_key();
        let marker = self.read_marker();
        let marker_names_ours = match (&ours, &marker) {
            (Some(ours), Some(marker)) => marker.peer_public_key == *ours,
            _ => false,
        };
        // With a pair and no matching marker, or with no pair but a marker,
        // the live key is the fact that decides. A marker alone is a hint.
        let need_live = !self.live_peer_key_checked
            && (ours.is_some() || marker.is_some())
            && !marker_names_ours;
        if need_live {
            self.live_peer_key_checked = true;
            if self.root.has_helper() {
                self.live_peer_key = self
                    .read_live_config()
                    .ok()
                    .map(|live| live.peer_public_key);
            }
        }
        // No pair: the live tunnel is ours only when its peer key is exactly
        // the one our marker recorded. A stale marker that names another
        // tunnel therefore stays foreign, and nothing is ever touched.
        let marker_matches = match (&marker, &self.live_peer_key) {
            (Some(marker), Some(live)) => marker.peer_public_key == *live,
            _ => marker_names_ours,
        };
        let owner = ownership_decision(&OwnershipInputs {
            tunnel_present: true,
            live_peer_key: self.live_peer_key.as_deref(),
            our_peer_key: ours.as_deref(),
            brought_up_by_us: false,
            marker_matches,
        });
        self.foreign_tunnel = owner == TunnelOwner::Foreign;
        owner
    }

    /// The other instance's tunnel went away: drop the foreign status so the
    /// card and Connect come back without a restart. Unprivileged (sysfs).
    fn foreign_poll(&mut self) {
        if self.foreign_tunnel && !self.interface_present() {
            self.assess_ownership();
            self.log_event(
                "INFO",
                "the other Jafnify connection ended; ready to connect",
            );
        }
    }

    /// `jafn0` belongs to another instance: show it and do not connect.
    pub fn foreign_tunnel(&self) -> bool {
        self.foreign_tunnel
    }

    /// A host-wide resolved override that Jafnify did not write, if any.
    pub fn host_dns_override(&self) -> Option<&str> {
        self.host_dns_override.as_deref()
    }

    /// True when this process may run a privileged command on `jafn0`.
    fn owns_tunnel_now(&mut self) -> bool {
        self.owns_tunnel || self.assess_ownership() == TunnelOwner::Ours
    }

    /// Refuse to connect over another instance's tunnel: the plain status is
    /// shown and no search starts.
    fn block_on_foreign_tunnel(&mut self) -> bool {
        if self.interface_present() && self.assess_ownership() == TunnelOwner::Foreign {
            self.foreign_tunnel = true;
            true
        } else {
            false
        }
    }

    /// The installed config (root-owned) is there; stat works without root.
    fn installed_conf_present(&self) -> bool {
        self.installed_conf_path.exists()
    }

    /// Received bytes of `jafn0` (sysfs, unprivileged); None if unreadable.
    fn read_rx(&self) -> Option<u64> {
        read_counter(&self.rx_bytes_path)
    }

    /// While `jafn0` exists, make sure the shared main-table suppress rule is
    /// still there. Another tool (a co-existing wg0 going down) can delete it.
    /// The check is unprivileged; a repair is one non-blocking helper call.
    fn suppress_poll(&mut self, now: Instant) {
        if !self.tunnel_present() {
            self.suppress_job = None;
            self.last_suppress_check = None;
            self.suppress_retry_at = None;
            self.suppress_restore_pending = false;
            return;
        }
        // A foreign tunnel's shared rule is not ours to repair.
        if !self.owns_tunnel && self.assess_ownership() == TunnelOwner::Foreign {
            self.suppress_job = None;
            self.last_suppress_check = None;
            self.suppress_retry_at = None;
            self.suppress_restore_pending = false;
            return;
        }
        if let Some(job) = self.suppress_job.as_mut() {
            match job.child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        self.log_event("WARN", "privileged suppress: helper failed");
                        self.suppress_retry_at = Some(now + SUPPRESS_RETRY);
                    }
                    self.suppress_job = None;
                }
                Ok(None) => return,
                Err(e) => {
                    self.log_event("WARN", &format!("privileged suppress: {e}"));
                    self.suppress_job = None;
                    self.suppress_retry_at = Some(now + SUPPRESS_RETRY);
                }
            }
        }
        if self.suppress_retry_at.is_some_and(|t| now < t) {
            return;
        }
        if self
            .last_suppress_check
            .is_some_and(|t| now.saturating_duration_since(t) < SUPPRESS_CHECK_INTERVAL)
        {
            return;
        }
        self.last_suppress_check = Some(now);
        let Ok(out) = Command::new(&self.ip_bin)
            .args(["-4", "rule", "show"])
            .output()
        else {
            return;
        };
        if !out.status.success() {
            return;
        }
        if crate::helper::suppress_rule_present(&String::from_utf8_lossy(&out.stdout)) {
            self.suppress_retry_at = None;
            if self.suppress_restore_pending {
                self.suppress_restore_pending = false;
                self.connection_log.push(
                    "suppress",
                    connection_log::SUPPRESS_RESTORED,
                    self.session_epoch_ms_at(now),
                );
            }
            return;
        }
        if !self.root.has_helper() {
            return;
        }
        match self.root.spawn(self.root.suppress_command()) {
            Ok(child) => {
                self.suppress_restore_pending = true;
                self.suppress_job = Some(SuppressJob { child });
            }
            Err(e) => self.log_event("WARN", &format!("privileged suppress: {e}")),
        }
    }

    /// How long a known-peer reconnect (jafn0 kept) waits for the peer's first
    /// data. After a fallback nothing is routed into jafn0, so the first
    /// handshake comes from the persistent keepalive: wait for it instead of
    /// rebuilding the tunnel. The code track brings jafn0 up fresh and keeps
    /// CONNECT_TIMEOUT.
    fn connect_timeout(&self) -> Duration {
        known_connect_timeout(self.routing_down)
    }

    /// Remove the tunnel's routing (not the interface) so normal networking
    /// works. Public for the CLI `fallback` verb and the shutdown path.
    pub fn fall_back_routing(&mut self) -> bool {
        if self.settings.kill_switch {
            return false;
        }
        // No jafn0: nothing routes into a tunnel that does not exist (the
        // kernel drops its table route with the device), so there is nothing
        // to fall back from and no privileged call to make.
        if !self.interface_present() && !self.tunnel_present() {
            return false;
        }
        // Never re-route a tunnel another instance owns.
        if self.interface_present() && !self.owns_tunnel_now() {
            return false;
        }
        match self.run_root_input("route fallback", self.root.route_fallback_command(), None) {
            Ok(out) => {
                // The route binary reports a resolver step it could not undo.
                // The fallback itself worked, but the DNS fault is real and is
                // shown once.
                if let Some(text) =
                    crate::helper::dns_warning(&String::from_utf8_lossy(&out.stderr))
                {
                    self.log_event("WARN", &format!("dns routing: {text}"));
                }
                self.routing_down = true;
                self.fallback_at = None;
                self.restore_at = None;
                self.last_error = None;
                self.last_route_toggle = Some(Instant::now());
                self.answered_since_toggle = false;
                self.log_event("INFO", "route fallback: normal routing restored");
                true
            }
            Err(e) => {
                self.last_error = Some(format!(
                    "The tunnel is down but its route could not be removed: {e}"
                ));
                self.fallback_at = Some(Instant::now() + RETRY_INTERVAL);
                false
            }
        }
    }

    /// Connected again after a fallback: hand routing back to the tunnel.
    fn restore_routing(&mut self, now: Instant) {
        match self.run_root_input("route restore", self.root.route_restore_command(), None) {
            Ok(out) => {
                self.routing_down = false;
                self.restore_at = None;
                self.last_route_toggle = Some(now);
                self.answered_since_toggle = false;
                // The route binary reports a resolver step it could not undo;
                // the rule is back, but the DNS fault is real and shown once.
                if let Some(text) =
                    crate::helper::dns_warning(&String::from_utf8_lossy(&out.stderr))
                {
                    self.log_event("WARN", &format!("dns routing: {text}"));
                }
                // The IPv6 blackhole could not be put back: IPv6 can leave
                // around the tunnel.
                if let Some(text) =
                    crate::helper::ipv6_warning(&String::from_utf8_lossy(&out.stderr))
                {
                    self.log_event("WARN", &crate::helper::ipv6_warning_line(text));
                }
                self.log_event("INFO", "route restore: traffic back through the tunnel");
            }
            Err(e) => {
                self.restore_at = Some(now + RETRY_INTERVAL);
                self.log_event("WARN", &format!("route restore failed: {e}"));
            }
        }
    }

    /// Routing follows tunnel liveness, every tick: with the kill switch off
    /// the system's own routing is the default and jafn0 carries traffic only
    /// while the peer answers.
    fn fallback_poll(&mut self, now: Instant) {
        if self.state() == ConnectionState::Connected {
            if !self.fallback_rule_checked {
                self.fallback_rule_checked = true;
                let installed = tunnel_rule_installed(&self.root.wg_bin, &self.ip_bin);
                if restore_needed(true, self.routing_down, installed) {
                    // Connected while jafn0's own rule is missing (left by
                    // another process): liveness decides when to hand it back.
                    self.routing_down = true;
                    self.answered_since_toggle = false;
                    self.log_event("WARN", "tunnel routing missing while connected");
                }
            }
        } else {
            self.fallback_rule_checked = false;
        }
        self.tunnel_dead = false;
        let joiner = self.settings.role == jafnify_core::state::Role::Joiner;
        if self.settings.kill_switch || !joiner || !self.tunnel_present() {
            self.live_watch = None;
            self.last_probe = None;
            return;
        }
        // Only this instance's own tunnel may drive routing; a foreign
        // tunnel's rx is not our liveness.
        if !self.owns_tunnel && self.assess_ownership() == TunnelOwner::Foreign {
            self.live_watch = None;
            self.last_probe = None;
            return;
        }
        let rx = self.read_rx();
        let watch = self.live_watch.get_or_insert_with(|| RxWatch::new(now, rx));
        if watch.advanced(now, rx) {
            self.answered_since_toggle = true;
            self.link_back_at = None;
        }
        let silent = watch.silent_for(now);
        let dead_after = if self.probe_no_ping {
            LIVENESS_DEAD_AFTER_NO_PING
        } else {
            LIVENESS_DEAD_AFTER
        };
        // The display neutral rule for a tunnel with no known track.
        self.tunnel_dead = silent >= dead_after;
        let probe_due = self
            .last_probe
            .is_none_or(|at| now.saturating_duration_since(at) >= LIVENESS_PROBE_IDLE);
        if (silent >= LIVENESS_PROBE_IDLE || self.link_back_at.is_some()) && probe_due {
            self.last_probe = Some(now);
            self.send_liveness_probe();
        }
        let inputs = RouteInputs {
            kill_switch: self.settings.kill_switch,
            tunnel_present: true,
            silent,
            dead_after,
            link_back_unanswered: self
                .link_back_at
                .is_some_and(|at| now.saturating_duration_since(at) >= LINK_BACK_ANSWER),
            routing_down: self.routing_down,
            since_toggle: self
                .last_route_toggle
                .map_or(MIN_ROUTE_TOGGLE, |at| now.saturating_duration_since(at)),
            retry_ok: if self.routing_down {
                self.restore_at.is_none_or(|at| now >= at)
            } else {
                self.fallback_at.is_none_or(|at| now >= at)
            },
            answered_since_toggle: self.answered_since_toggle,
        };
        match route_decision(&inputs) {
            RouteAction::None => {}
            RouteAction::Fallback => {
                self.fall_back_routing();
            }
            RouteAction::Restore => self.restore_routing(now),
        }
    }

    /// One small packet into jafn0, bound to the device so it goes through
    /// the tunnel even while the routing is fallen back. An ICMP echo to the
    /// gateway is answered by the peer at once; without unprivileged ping a
    /// UDP packet makes the peer's WireGuard send a passive keepalive.
    /// Failures are ignored: the rx counter is the only evidence used.
    fn send_liveness_probe(&mut self) {
        let device = IFACE.as_bytes();
        if !self.probe_no_ping {
            match socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::ICMPV4),
            ) {
                Ok(sock) => {
                    let _ = sock.bind_device(Some(device));
                    let _ = sock.set_nonblocking(true);
                    // Echo request, id/seq 0: the kernel fills the id.
                    let mut packet = [8u8, 0, 0, 0, 0, 0, 0, 1, b'j', b'f'];
                    let sum = icmp_checksum(&packet);
                    packet[2..4].copy_from_slice(&sum.to_be_bytes());
                    let to: std::net::SocketAddr = (
                        jafnify_core::GATEWAY_DNS_IP
                            .parse::<std::net::Ipv4Addr>()
                            .unwrap_or(std::net::Ipv4Addr::new(10, 233, 0, 1)),
                        0,
                    )
                        .into();
                    let _ = sock.send_to(&packet, &to.into());
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    self.probe_no_ping = true;
                    self.log_event(
                        "INFO",
                        "liveness: unprivileged ping not allowed; UDP probes",
                    );
                }
                Err(_) => return,
            }
        }
        if let Ok(sock) = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None) {
            let _ = sock.bind_device(Some(device));
            let _ = sock.set_nonblocking(true);
            let to: std::net::SocketAddr = (
                jafnify_core::SHARER_IP
                    .parse::<std::net::Ipv4Addr>()
                    .unwrap_or(std::net::Ipv4Addr::new(10, 9, 9, 1)),
                9,
            )
                .into();
            let _ = sock.send_to(b"j", &to.into());
        }
    }

    /// A daemon that died (including SIGKILL) may have left a dead tunnel's
    /// routing installed. At start, with the kill switch off and jafn0 up with
    /// its rule still in place, normal routing wins first; the reconnect
    /// restores it. A fallback state (rule already gone) needs nothing.
    pub fn clear_stale_routing(&mut self) {
        if self.settings.kill_switch {
            return;
        }
        if !(self.root.has_route_binary() || self.root.has_helper()) {
            return;
        }
        if !self.interface_present() {
            return;
        }
        if !self.owns_tunnel && self.assess_ownership() == TunnelOwner::Foreign {
            return;
        }
        if !matches!(
            tunnel_rule_installed(&self.root.wg_bin, &self.ip_bin),
            Some(true)
        ) {
            return;
        }
        self.fall_back_routing();
    }

    /// The tunnel is dead or never came up. `jafn0` routes 0.0.0.0/0 into the
    /// tunnel, so take it down FIRST (the PC regains its normal internet), then
    /// hand over to the orchestrator: Retry, 10 s, Searching.
    /// `tunnel_touched`: a bring-up was attempted, so `jafn0` may exist; skip
    /// the (polkit-prompting) down when nothing was ever brought up.
    fn connection_lost(&mut self, reason: String, now: Instant, teardown: bool) {
        self.connection_lost_with_retry(reason, now, teardown, RETRY_INTERVAL);
    }

    /// The body of `connection_lost`, with the retry delay chosen by the caller.
    fn connection_lost_with_retry(
        &mut self,
        reason: String,
        now: Instant,
        teardown: bool,
        retry: Duration,
    ) {
        self.connection_lost_with_retry_at(reason, None, now, teardown, retry);
    }

    /// `connection_lost_with_retry`, with the start of the outage when it is
    /// not this moment: a neutral reset decided after its window closed began
    /// at the reset, not at the decision.
    fn connection_lost_with_retry_at(
        &mut self,
        reason: String,
        start_ms: Option<i64>,
        now: Instant,
        teardown: bool,
        retry: Duration,
    ) {
        let drops_before = self.session.current().drops;
        // A loss of an established link. The state is still Connected here:
        // the orchestrator moves to Retry further down.
        let was_connected = self.state() == ConnectionState::Connected;
        self.disconnected_at.get_or_insert(now);
        self.log_event("WARN", &format!("disconnected: {reason}"));
        let mut drop_hint = None;
        if was_connected {
            self.outage_reason = Some(drop_history::reason_word(&reason));
            drop_hint = match start_ms {
                Some(at_ms) => self.open_drop_at(&reason, at_ms, now),
                None => self.open_drop(&reason, now),
            };
        }
        // Kill switch OFF means the laptop must never be offline because of a
        // dead tunnel. An explicit stop/failed bring-up tears everything down;
        // on a peer or link loss jafn0 stays for a fast return and the routing
        // follows liveness (fallback_poll). Kill switch ON keeps the blocking.
        if teardown && !self.settings.kill_switch && self.owns_tunnel {
            let _ = self.run_root("tunnel down", self.root.down_command());
            self.routing_down = false;
            self.restore_at = None;
            self.fallback_at = None;
        }
        self.rx_watch = None;
        self.link_watch = None;
        self.link_lost = false;
        self.last_link_sample = None;
        self.reconnect_rx = None;
        self.connecting_since = None;
        self.auto_accept_deadline = None;
        self.adopting = false;
        self.peer_public_key = None;
        self.peer_endpoint = None;
        self.last_announce = None;
        self.clear_reset_watch(now);
        self.orchestrator
            .on_disconnect_at_with_retry(reason, now, retry);
        self.polkit_prompt_expected = false;
        self.sample_session(now);
        let after = self.session.current();
        if after.drops > drops_before {
            if let Some(record) = after.reconnects.last() {
                let range_hint = drop_hint.filter(|hint| *hint == drop_history::HINT_OUT_OF_RANGE);
                self.connection_log.push(
                    "drop",
                    &connection_log::drop_line_with_hint(
                        &record.reason,
                        record.disconnected_at,
                        record.detected_at,
                        range_hint,
                    ),
                    self.session_epoch_ms_at(now),
                );
                // The log has a drop to close: the line that says the link is
                // back comes with the return to Connected.
                self.logged_drop = Some(record.disconnected_at);
            }
        }
    }

    /// The return to Connected after a logged drop: the log gets the line
    /// that closes the drop, in the same time style as the drop line and
    /// without an endpoint or an address.
    fn log_reconnected(&mut self, now: Instant) {
        if self.logged_drop.take().is_none() {
            return;
        }
        let now_ms = self.session_epoch_ms_at(now);
        self.connection_log.push(
            "reconnect",
            &connection_log::reconnected_line(now_ms),
            now_ms,
        );
    }

    fn desired_session(&mut self) -> Result<SessionPlan, AppError> {
        let priv_key = self
            .keypair
            .as_ref()
            .ok_or(AppError::InvalidCode)?
            .private_key_b64();
        let peer_pub = self.peer_public_key.clone().ok_or(AppError::InvalidCode)?;
        let guard = detect_guard();
        if guard == InboundGuard::None {
            eprintln!(
                "warning: neither iptables nor nft is installed - the tunnel comes up \
                 without the inbound guard (new connections to this computer over the tunnel are not blocked)"
            );
        }
        let kill = self.kill_switch_for(guard)?;
        let resolved = resolved_present(
            &tool_present,
            resolved_running(std::path::Path::new("/run/systemd/resolve")),
        );
        let cfg: WireGuardConfig = build_config(
            self.settings.role,
            &priv_key,
            &peer_pub,
            self.peer_endpoint,
            guard,
            resolved,
            kill.as_ref(),
        );
        let desired = LiveConfig::from_config(&cfg, kill.is_some());
        Ok(SessionPlan {
            priv_key,
            peer_pub,
            guard,
            resolved,
            kill,
            cfg,
            desired,
        })
    }

    fn bring_up_tunnel(&mut self) -> Result<(), AppError> {
        let SessionPlan {
            priv_key,
            peer_pub,
            guard,
            resolved,
            kill,
            cfg,
            desired,
        } = self.desired_session()?;
        if self.tunnel_present() && self.peer_wg_fresh {
            self.log_event(
                "INFO",
                "the peer started a new session: setting jafn0 up again",
            );
        }
        if self.tunnel_present() {
            if self.assess_ownership() == TunnelOwner::Foreign {
                self.foreign_tunnel = true;
                return Err(AppError::ForeignTunnel);
            }
            if !self.peer_wg_fresh && !self.marker_reuse_blocked && self.marker_matches(&desired) {
                self.log_event("INFO", "reusing existing jafn0 (no privileged call)");
                self.tunnel_reused = true;
                self.reconnect_rx = self.read_rx();
                self.write_marker(&desired);
                self.marker_reuse_blocked = false;
                self.polkit_prompt_expected = false;
                self.owns_tunnel = true;
                self.foreign_tunnel = false;
                return Ok(());
            }
            if !self.peer_wg_fresh && self.root.has_helper() {
                match self.read_live_config() {
                    Ok(live) => match reuse_decision(&desired, &live) {
                        Reuse::Same => {
                            self.log_event("INFO", "reusing existing jafn0 after inspect");
                            self.tunnel_reused = true;
                            self.reconnect_rx = self.read_rx();
                            self.write_marker(&desired);
                            self.marker_reuse_blocked = false;
                            self.polkit_prompt_expected = false;
                            self.owns_tunnel = true;
                            self.foreign_tunnel = false;
                            return Ok(());
                        }
                        Reuse::Endpoint => {
                            let Some(endpoint) = desired.endpoint.clone() else {
                                return Err(AppError::Root("no endpoint to update".into()));
                            };
                            let cmd = self.root.endpoint_command();
                            if let Err(e) = self.run_root_input(
                                "tunnel endpoint",
                                cmd,
                                Some(endpoint.as_bytes()),
                            ) {
                                self.log_event("WARN", &format!("endpoint update failed: {e}"));
                            } else {
                                self.log_event("INFO", "updated jafn0 endpoint in place");
                                self.tunnel_reused = true;
                                self.reconnect_rx = self.read_rx();
                                self.write_marker(&desired);
                                self.marker_reuse_blocked = false;
                                self.owns_tunnel = true;
                                self.foreign_tunnel = false;
                                return Ok(());
                            }
                        }
                        Reuse::Different => {}
                    },
                    Err(e) => self.log_event("WARN", &format!("inspect jafn0: {e}")),
                }
            }
        }
        let conf_path = self.config_dir.join(format!("{IFACE}.conf"));
        let text = ConfigGenerator.to_wg_quick_text(&cfg);
        let request = crate::helper::request_toml(
            self.settings.role,
            &priv_key,
            &peer_pub,
            self.peer_endpoint,
            guard,
            resolved,
            kill.as_ref(),
        );
        ensure_private_dir(&self.config_dir)?;
        if !self.root.is_available() {
            self.last_error = Some("pkexec not found (polkit package missing)".to_string());
            return Err(AppError::Root("pkexec not found".into()));
        }
        // Replace our own leftover (marker) or a tunnel kept up by the kill
        // switch. A foreign jafn0 without a marker is left for wg-quick to
        // report; the error names the interface.
        if self.tunnel_present()
            && (self.read_marker().is_some()
                || self.settings.kill_switch
                || (self.peer_wg_fresh && self.interface_present()))
        {
            self.log_event(
                "WARN",
                "taking down the existing jafn0 before this connection",
            );
            self.run_root("tunnel down", self.root.down_command())?;
            self.clear_marker();
        }
        if !self.root.has_helper() {
            std::fs::write(&conf_path, text)?;
            // The source copy holds the private key: restrict it, then remove
            // it after the one combined install + up call.
            std::fs::set_permissions(&conf_path, std::fs::Permissions::from_mode(0o600))?;
        }
        let (cmd, input) = self.root.connect_command(&conf_path, &request);
        // From here on the tunnel is this process's own, including a failed
        // bring-up that may have left a partial interface behind.
        self.owns_tunnel = true;
        let up = self.run_root_input("tunnel up", cmd, input.as_deref());
        let _ = std::fs::remove_file(&conf_path);
        let out = up?;
        // The helper reports a DNS-routing failure it could not fix (the
        // default-route step is denied inside wg-quick's profile). The tunnel
        // is up; surface the real reason once instead of hiding it.
        if let Some(text) = crate::helper::dns_warning(&String::from_utf8_lossy(&out.stderr)) {
            self.log_event("WARN", &format!("dns routing: {text}"));
        }
        // The IPv6 guard failed: the tunnel is up, but IPv6 can leave around
        // it. The reason is shown once.
        if let Some(text) = crate::helper::ipv6_warning(&String::from_utf8_lossy(&out.stderr)) {
            self.log_event("WARN", &crate::helper::ipv6_warning_line(text));
        }
        self.write_marker(&desired);
        self.reconnect_rx = Some(0);
        self.marker_reuse_blocked = false;
        self.peer_wg_fresh = false;
        self.tunnel_reused = false;
        self.foreign_tunnel = false;
        self.live_peer_key = Some(peer_pub);
        self.live_peer_key_checked = true;
        Ok(())
    }

    fn read_live_config(&mut self) -> Result<LiveConfig, AppError> {
        let cmd = self.root.inspect_command();
        let out = match self.run_root_input("tunnel inspect", cmd, None) {
            Ok(out) => out,
            Err(e) => {
                self.marker_reuse_blocked = true;
                return Err(e);
            }
        };
        match LiveConfig::from_toml(&String::from_utf8_lossy(&out.stdout)) {
            Ok(live) => Ok(live),
            Err(e) => {
                self.marker_reuse_blocked = true;
                Err(AppError::Root(e.to_string()))
            }
        }
    }

    /// The kill switch for this bring-up: joiner only, and only when turned on.
    /// Refuses (with the reason) rather than run without it.
    fn kill_switch_for(&mut self, guard: InboundGuard) -> Result<Option<KillSwitch>, AppError> {
        if !self.settings.kill_switch || self.settings.role != jafnify_core::state::Role::Joiner {
            return Ok(None);
        }
        let refuse = |app: &mut Self, msg: &str| {
            app.last_error = Some(msg.to_string());
            Err(AppError::Root(msg.to_string()))
        };
        if guard == InboundGuard::None {
            return refuse(
                self,
                "The kill switch needs iptables or nft - install one, or turn it off",
            );
        }
        let lan = self
            .lan_subnet()
            .or_else(|| self.peer_endpoint.map(|e| format!("{}/32", e.ip())));
        match lan {
            Some(lan) => Ok(Some(KillSwitch {
                iptables: guard == InboundGuard::Iptables,
                lan,
            })),
            None => refuse(
                self,
                "No LAN network found for the kill switch - connect to the hotspot first",
            ),
        }
    }

    /// Turn the kill switch on or off. On: applies at the next connection. Off
    /// while the tunnel is up: its rules are removed now, the tunnel stays up.
    pub fn set_kill_switch(&mut self, on: bool) -> Result<(), AppError> {
        let was = self.settings.kill_switch;
        self.settings.kill_switch = on;
        self.save()?;
        if was && !on && self.tunnel_present() {
            match detect_guard() {
                InboundGuard::None => {}
                tool => {
                    let iptables = tool == InboundGuard::Iptables;
                    self.run_root(
                        "kill switch down",
                        self.root.kill_switch_down_command(iptables),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn kill_switch(&self) -> bool {
        self.settings.kill_switch
    }

    pub fn tray_only_when_closed(&self) -> bool {
        self.settings.tray_only_when_closed
    }

    pub fn set_tray_only_when_closed(&mut self, on: bool) {
        self.settings.tray_only_when_closed = on;
    }

    pub fn theme(&self) -> ThemeMode {
        self.settings.theme
    }

    /// The caller persists it (the daemon's settings path saves the state).
    pub fn set_theme(&mut self, theme: ThemeMode) {
        self.settings.theme = theme;
    }

    pub fn details_expanded(&self) -> bool {
        self.settings.details_expanded
    }

    pub fn advanced_expanded(&self) -> bool {
        self.settings.advanced_expanded
    }

    pub fn set_view_state(&mut self, details: bool, advanced: bool) {
        self.settings.details_expanded = details;
        self.settings.advanced_expanded = advanced;
    }

    /// Searching, and this computer has no usable network (the known-peer
    /// sends fail): the search waits for it.
    pub fn no_network(&self) -> bool {
        self.state() == ConnectionState::Searching && self.no_network_since.is_some()
    }

    /// A paired phone is expected; the known track is searching for it.
    pub fn waiting_for_phone(&self) -> bool {
        self.use_known
    }

    pub fn window_geometry(&self) -> WindowGeometry {
        self.settings.window
    }

    /// Ignore a geometry outside the sane range; keep the previous one.
    pub fn set_window_geometry(&mut self, window: WindowGeometry) {
        if let Some(window) = window.sanitized() {
            self.settings.window = window;
        }
    }

    fn run_root(&mut self, label: &str, cmd: Command) -> Result<(), AppError> {
        self.run_root_input(label, cmd, None).map(|_| ())
    }

    fn run_root_input(
        &mut self,
        label: &str,
        cmd: Command,
        input: Option<&[u8]>,
    ) -> Result<Output, AppError> {
        self.log_event("INFO", &format!("privileged {label}: running"));
        let now_ms = self.session_epoch_ms_at(Instant::now());
        let covered = self.pkcheck_authorized();
        let managed = hint_is_managed(label);
        self.arm_polkit_hint(label, covered);
        let out = match self.root.run_with_input(cmd, input) {
            Ok(out) => out,
            Err(e) => {
                if managed {
                    self.clear_polkit_hint();
                }
                self.log_privileged(label, false, now_ms, covered);
                return Err(AppError::Io(e));
            }
        };
        if managed {
            self.clear_polkit_hint();
        }
        if !out.status.success() {
            // pkexec: 126 = the dialog was dismissed, 127 = not authorized.
            if matches!(out.status.code(), Some(126) | Some(127)) {
                self.last_error = Some("Permission was denied".to_string());
                self.log_event("WARN", &format!("privileged {label}: permission denied"));
                self.log_privileged(label, false, now_ms, covered);
                return Err(AppError::PermissionDenied);
            }
            let stderr = redact_keys(&String::from_utf8_lossy(&out.stderr));
            self.last_error = Some(format!("Root privileges required - {stderr}"));
            self.log_event("ERROR", &format!("privileged {label}: {stderr}"));
            self.log_privileged(label, false, now_ms, covered);
            return Err(AppError::Root(stderr));
        }
        self.log_event("INFO", &format!("privileged {label}: ok"));
        self.log_privileged(label, true, now_ms, covered);
        Ok(out)
    }

    /// Whether polkit already authorizes this process for the tunnel action.
    /// The action is bound to the installed helper, so the generic `pkexec
    /// /bin/sh` fallback is never covered by the Jafnify rule. `pkcheck` is
    /// asked as this process (the rule directory is root-only, so the app
    /// cannot read the installed rules). A missing tool or any nonzero exit
    /// means a password prompt can be expected. No `--allow-user-interaction`,
    /// so the check never opens a dialog.
    fn pkcheck_authorized(&self) -> bool {
        if !self.root.has_helper() {
            return false;
        }
        let mut cmd = Command::new(&self.pkcheck_bin);
        let pid = std::process::id().to_string();
        cmd.args(["--action-id", crate::polkit::ACTION, "--process", &pid]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        exec::output(cmd).is_ok_and(|output| output.status.success())
    }

    /// Change the hint only for calls that can raise or lower the tunnel.
    /// Unrelated privileged work (ufw, kill switch, suppress, ...) must leave
    /// the attempt note exactly as it was.
    fn arm_polkit_hint(&mut self, label: &str, covered: bool) {
        match label {
            "tunnel up" | "tunnel down" | "teardown" | "forget" => {
                self.polkit_prompt_expected = !covered;
            }
            "tunnel inspect" | "tunnel endpoint" => {
                self.polkit_prompt_expected = false;
            }
            _ => {}
        }
    }

    fn clear_polkit_hint(&mut self) {
        self.polkit_prompt_expected = false;
    }

    /// Record the user-visible privileged line for tunnel up/down/teardown/
    /// forget only; inspect, endpoint and firewall calls are not shown.
    fn log_privileged(&mut self, label: &str, ok: bool, now_ms: i64, covered: bool) {
        let direction = match label {
            "tunnel up" => "up",
            "tunnel down" | "teardown" | "forget" => "down",
            _ => return,
        };
        self.connection_log.push(
            "privileged",
            &connection_log::privileged_line(direction, ok, covered),
            now_ms,
        );
    }

    fn log_event(&mut self, level: &str, message: &str) {
        if let Some(log) = &mut self.logger {
            log.event(level, message);
        }
    }

    /// Test hook: expose the inner orchestrator (integration tests drive the
    /// state machine directly).
    pub fn orchestrator_mut(&mut self) -> &mut Orchestrator {
        &mut self.orchestrator
    }
}

/// Pure config builder - sharer has no endpoint (learns it from the first
/// packet), joiner pins the peer's multicast source IP with LISTEN_PORT.
fn is_pairing_save_error(error: &str) -> bool {
    error.starts_with("could not save the pairing")
}

pub fn build_config(
    role: Role,
    private_key: &str,
    peer_public_key: &str,
    peer_endpoint: Option<SocketAddr>,
    guard: InboundGuard,
    resolved: bool,
    kill: Option<&KillSwitch>,
) -> WireGuardConfig {
    jafnify_core::config::build_config(
        role,
        private_key,
        peer_public_key,
        peer_endpoint,
        guard,
        resolved,
        kill,
    )
}

/// One root step of stop. A tunnel's down step also removes the installed
/// config and the kill switch (PostDown), so it is the only step then. Without
/// a tunnel, a kill switch left by a failed bring-up and a leftover config are removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    WgDown,
    RemoveConf,
    KillSwitchDown,
}

pub fn teardown_plan(tunnel: bool, conf: bool, kill_switch: bool) -> Vec<Teardown> {
    if tunnel {
        return vec![Teardown::WgDown];
    }
    let mut plan = Vec::new();
    if kill_switch {
        plan.push(Teardown::KillSwitchDown);
    }
    if conf {
        plan.push(Teardown::RemoveConf);
    }
    plan
}

/// Known-peer reconnect timeout. While the routing is fallen back, WireGuard
/// may still hold a young session and re-handshakes only after its ~15 s
/// no-reply timer, so the plain CONNECT_TIMEOUT would race it.
pub fn known_connect_timeout(routing_down: bool) -> Duration {
    if routing_down {
        jafnify_core::timing::RETURN_CONNECT_TIMEOUT
    } else {
        CONNECT_TIMEOUT
    }
}

/// What the routing should do this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAction {
    None,
    /// The tunnel is dead (or the uplink went away): the system's own
    /// routing wins.
    Fallback,
    /// The peer answers again: traffic goes back through the tunnel.
    Restore,
}

/// Everything the routing decision depends on, so it is testable without
/// root, sockets or clocks.
#[derive(Debug, Clone, Copy)]
pub struct RouteInputs {
    pub kill_switch: bool,
    pub tunnel_present: bool,
    /// How long the peer's data has been missing.
    pub silent: Duration,
    /// Silence that means dead (depends on the probe kind).
    pub dead_after: Duration,
    /// The uplink came back and the peer stayed silent for LINK_BACK_ANSWER:
    /// another network. A link that is merely down changes nothing (no
    /// network either way, and a blip back to the same hotspot rides through).
    pub link_back_unanswered: bool,
    pub routing_down: bool,
    pub since_toggle: Duration,
    /// A failed root call's retry time has passed.
    pub retry_ok: bool,
    /// The peer's data arrived after the last routing change.
    pub answered_since_toggle: bool,
}

/// RFC 1071 Internet checksum over an ICMP message.
pub(crate) fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = u16::from_be_bytes([chunk[0], *chunk.get(1).unwrap_or(&0)]);
        sum = sum.wrapping_add(u32::from(word));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Routing follows liveness. Kill switch on or no tunnel: never touch it.
pub fn route_decision(i: &RouteInputs) -> RouteAction {
    if i.kill_switch || !i.tunnel_present || !i.retry_ok || i.since_toggle < MIN_ROUTE_TOGGLE {
        return RouteAction::None;
    }
    let dead = i.link_back_unanswered || i.silent >= i.dead_after;
    match (i.routing_down, dead) {
        (false, true) => RouteAction::Fallback,
        (true, false) if i.answered_since_toggle => RouteAction::Restore,
        _ => RouteAction::None,
    }
}

/// Connected again: is jafn0's own wg-quick rule gone? `None` means the state
/// could not be read (a transient failure must not trigger a privileged call).
pub fn restore_needed(connected: bool, routing_down: bool, rule_installed: Option<bool>) -> bool {
    connected && !routing_down && rule_installed == Some(false)
}

/// The routing state found at adoption. An earlier process falls back before
/// it exits (a dead tunnel's routing must not stay installed), so an adopted
/// jafn0 can be up with its own rule already gone. Record that state now: the
/// first Connected tick then restores the routing because it is down, instead
/// of reporting a state that was known before the connection as "missing while
/// connected". `None` (unreadable) changes nothing.
pub fn adopted_routing_down(rule_installed: Option<bool>) -> bool {
    rule_installed == Some(false)
}

/// Unprivileged read: is jafn0 up with its own `not from all fwmark <mark>
/// lookup <table>` rule? The mark comes from `wg show jafn0 fwmark`, the table
/// is its numeric value (as wg-quick derives it). A failed read or `off` is
/// `None` (no decision); a readable state with the rule missing is
/// `Some(false)`.
pub fn tunnel_rule_installed(wg_bin: &std::path::Path, ip_bin: &std::path::Path) -> Option<bool> {
    // A unit test must never run the host's real wg or ip. Outside tests the
    // guard is off and the real tools are exactly what we want.
    if crate::root::test_guard_active()
        && !(crate::root::is_test_stub(wg_bin) && crate::root::is_test_stub(ip_bin))
    {
        return None;
    }
    let mark_out = std::process::Command::new(wg_bin)
        .args(["show", crate::helper::IFACE, "fwmark"])
        .output()
        .ok()?;
    if !mark_out.status.success() {
        // `wg show` needs root; a normal user (the tray, the ExecStopPost
        // fallback) reads the same fact from unprivileged `ip` output.
        return tunnel_rule_installed_unprivileged(ip_bin);
    }
    let mark = crate::route::parse_fwmark(&String::from_utf8_lossy(&mark_out.stdout))?;
    let rules = std::process::Command::new(ip_bin)
        .args(["-4", "rule", "show"])
        .output()
        .ok()?;
    if !rules.status.success() {
        return None;
    }
    Some(crate::route::tunnel_rule_present(
        &String::from_utf8_lossy(&rules.stdout),
        mark,
        mark,
    ))
}

/// jafn0's rule state from `ip -4 rule show` and `ip -4 route show table
/// all`, both readable without root (see route::iface_rule_installed).
fn tunnel_rule_installed_unprivileged(ip_bin: &std::path::Path) -> Option<bool> {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new(ip_bin)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let rules = run(&["-4", "rule", "show"])?;
    let routes = run(&["-4", "route", "show", "table", "all"])?;
    crate::route::iface_rule_installed(&rules, &routes, crate::helper::IFACE)
}

/// Whether a command is on PATH (or in the sbin directories, where root tools live).
fn tool_present(name: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/sbin"), PathBuf::from("/sbin")])
        .any(|dir| dir.join(name).is_file())
}

/// The guard tool this computer has: iptables, else nft.
pub fn detect_guard() -> InboundGuard {
    InboundGuard::choose(tool_present("iptables"), tool_present("nft"))
}

/// DNS can be routed through jafn0 only when systemd-resolved runs and its
/// client is installed. The runtime directory alone is not proof: a sandbox
/// can mount an empty tmpfs there. The varlink socket is the live daemon.
pub fn resolved_running(root: &std::path::Path) -> bool {
    root.join("io.systemd.Resolve").exists()
}

pub fn resolved_present(has: &dyn Fn(&str) -> bool, running: bool) -> bool {
    running && has("resolvectl")
}

/// A sysfs byte counter; None if unreadable.
fn read_counter(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Mask key-shaped base64 blobs so private material never surfaces in errors.
fn redact_keys(s: &str) -> String {
    let re = regex::Regex::new(r"[A-Za-z0-9+/]{42,44}=").expect("static regex");
    re.replace_all(s, "[redacted]").into_owned()
}

/// Interface name from `ip -4 -o route show default` output: the first
/// default route whose device is not loopback or a tunnel (wg*, jafn*, tun*).
/// (`ip route get default` is useless here: iproute2 resolves `default` as
/// 0.0.0.0 and answers `local 0.0.0.0 dev lo`.)
pub fn parse_lan_iface(route_output: &str) -> Option<String> {
    route_output
        .lines()
        .filter(|l| l.split_whitespace().next() == Some("default"))
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let i = words.iter().position(|w| *w == "dev")?;
            words.get(i + 1).map(|d| d.to_string())
        })
        .find(|d| d != "lo" && !is_tunnel(d))
}

/// Tunnel interfaces by name (wg*, jafn*, tun*): multicast into them goes
/// nowhere useful.
fn is_tunnel(name: &str) -> bool {
    ["wg", "jafn", "tun"].iter().any(|p| name.starts_with(p))
}

/// One diagnostic field from a search's address list ("a,b").
fn join_addrs(addrs: &[std::net::Ipv4Addr]) -> String {
    addrs
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Every IPv4 address of an interface that is up and running, multicast-capable,
/// not loopback, point-to-point or a tunnel, in `ip -4 -o addr show up` order.
/// `link_output` is `ip -o link show up` (its flags decide); an interface
/// missing from it is not used.
pub fn parse_sharer_ifaces(addr_output: &str, link_output: &str) -> Vec<std::net::Ipv4Addr> {
    parse_lan_ifaces(addr_output, link_output)
        .into_iter()
        .map(|i| i.ip)
        .collect()
}

/// `parse_sharer_ifaces` with the interface name and prefix of each address.
pub fn parse_lan_ifaces(addr_output: &str, link_output: &str) -> Vec<LocalIface> {
    let usable: std::collections::HashSet<&str> = link_output
        .lines()
        .filter_map(|l| {
            let name = l.split_whitespace().nth(1)?.trim_end_matches(':');
            let name = name.split('@').next()?;
            let flags: Vec<&str> = l.split('<').nth(1)?.split('>').next()?.split(',').collect();
            let has = |f: &str| flags.contains(&f);
            (has("UP")
                && has("LOWER_UP")
                && has("MULTICAST")
                && !has("LOOPBACK")
                && !has("POINTOPOINT")
                && !is_tunnel(name))
            .then_some(name)
        })
        .collect();
    addr_output
        .lines()
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let name = *words.get(1)?;
            if words.get(2) != Some(&"inet") || !usable.contains(name) {
                return None;
            }
            let (ip, prefix) = words.get(3)?.split_once('/')?;
            Some(LocalIface {
                name: name.to_string(),
                ip: ip.parse().ok()?,
                prefix: prefix.parse().ok()?,
            })
        })
        .collect()
}

/// Every LAN interface address of this computer (read-only `ip` queries);
/// empty if `ip` is unavailable or nothing qualifies.
pub fn detect_lan_ifaces(ip_bin: &std::path::Path) -> Vec<LocalIface> {
    let run = |args: &[&str]| {
        let mut cmd = Command::new(ip_bin);
        cmd.args(args);
        exec::output(cmd)
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    parse_lan_ifaces(
        &run(&["-4", "-o", "addr", "show", "up"]),
        &run(&["-o", "link", "show", "up"]),
    )
}

/// The addresses a sharer announces on.
fn detect_sharer_ifaces(ip_bin: &std::path::Path) -> Vec<std::net::Ipv4Addr> {
    detect_lan_ifaces(ip_bin)
        .into_iter()
        .map(|i| i.ip)
        .collect()
}

/// The addresses a joiner searches on: every up private LAN address that is
/// not a tunnel or a container bridge, with the saved peer's subnet first.
/// Public and link-local addresses are skipped, so a WAN-facing NIC cannot
/// attract discovery. The caller falls back to the default-route interface
/// when this is empty.
pub fn joiner_ifaces(with_addrs: &[LocalIface]) -> Vec<std::net::Ipv4Addr> {
    jafnify_core::route::joiner_search_ifaces(with_addrs, None)
}

/// The joiner's discovery addresses from read-only `ip` queries.
pub fn detect_joiner_ifaces(ip_bin: &std::path::Path) -> Vec<std::net::Ipv4Addr> {
    joiner_ifaces(&detect_lan_ifaces(ip_bin))
}

/// The joiner's discovery addresses with the saved peer's subnet first. The
/// address list keeps every usable network too, so a link change is followed
/// on the next retry without a restart.
pub fn detect_joiner_search_ifaces(
    ip_bin: &std::path::Path,
    peer: Option<std::net::Ipv4Addr>,
) -> Vec<std::net::Ipv4Addr> {
    jafnify_core::route::joiner_search_ifaces(&detect_lan_ifaces(ip_bin), peer)
}

/// First non-loopback IPv4 from `ip -4 -o addr show dev <iface>` output.
pub fn parse_iface_ipv4(addr_output: &str) -> Option<std::net::Ipv4Addr> {
    parse_iface_cidr(addr_output).map(|(ip, _)| ip)
}

/// First non-loopback IPv4 and its prefix length ("10.20.89.70/24") from the
/// same output.
pub fn parse_iface_cidr(addr_output: &str) -> Option<(std::net::Ipv4Addr, u8)> {
    addr_output.lines().find_map(|l| {
        let words: Vec<&str> = l.split_whitespace().collect();
        let i = words.iter().position(|w| *w == "inet")?;
        let (ip, prefix) = words.get(i + 1)?.split_once('/')?;
        let ip: std::net::Ipv4Addr = ip.parse().ok()?;
        (!ip.is_loopback()).then_some((ip, prefix.parse().ok()?))
    })
}

/// Interface, IPv4 and prefix of the LAN that owns the default route, so
/// discovery multicast joins/egress land on the real LAN interface instead of
/// whatever the kernel's multicast route happens to be (e.g. an up wg0
/// tunnel). The interface name is what the firewall rule is scoped to; the
/// address is only used where a subnet is genuinely needed (the kill switch).
/// Read-only `ip` queries; None if there is no usable LAN interface.
pub fn detect_lan(ip_bin: &std::path::Path) -> Option<(String, std::net::Ipv4Addr, u8)> {
    let mut route_cmd = Command::new(ip_bin);
    route_cmd.args(["-4", "-o", "route", "show", "default"]);
    let routes = exec::output(route_cmd).ok()?;
    let iface = parse_lan_iface(&String::from_utf8_lossy(&routes.stdout))?;
    let mut addr_cmd = Command::new(ip_bin);
    addr_cmd.args(["-4", "-o", "addr", "show", "dev", &iface]);
    let addrs = exec::output(addr_cmd).ok()?;
    let (ip, prefix) = parse_iface_cidr(&String::from_utf8_lossy(&addrs.stdout))?;
    Some((iface, ip, prefix))
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Sharer => "sharer",
        Role::Joiner => "joiner",
    }
}

/// Lowercase state name used by the diagnostic STATS record.
fn connection_state_name(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Idle => "idle",
        ConnectionState::Searching => "searching",
        ConnectionState::Pairing => "pairing",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Retry => "retry",
    }
}

/// The privileged calls that can change the armed attempt hint. Tunnel
/// up/down/teardown/forget may prompt; inspect and endpoint observe or update
/// the tunnel and clear the hint. Everything else leaves it unchanged.
fn hint_is_managed(label: &str) -> bool {
    matches!(
        label,
        "tunnel up" | "tunnel down" | "teardown" | "forget" | "tunnel inspect" | "tunnel endpoint"
    )
}

pub struct AppBuilder {
    config_dir: PathBuf,
    iface_pin: Option<std::net::Ipv4Addr>,
    ip_bin: PathBuf,
    link_iface: Option<String>,
    link_state: Option<LinkStateFn>,
    uplink_sysfs: PathBuf,
    wg_bin: PathBuf,
    pkexec_bin: PathBuf,
    pkcheck_bin: PathBuf,
    nmcli_bin: PathBuf,
    helper_bin: PathBuf,
    route_bin: PathBuf,
    wg_quick_bin: PathBuf,
    install_bin: PathBuf,
    ufw_bin: PathBuf,
    ufw_conf: PathBuf,
    auto_accept_delay: Duration,
    rx_bytes_path: PathBuf,
    tx_bytes_path: PathBuf,
    proc_wireless_path: PathBuf,
    station: Option<Box<dyn crate::station::StationSource>>,
    logger: Option<crate::applog::Logger>,
    marker_path: PathBuf,
    polkit_rules_path: PathBuf,
    installed_conf_path: PathBuf,
    teardown_iface_path: PathBuf,
    sessions_path: PathBuf,
    /// Injected epoch-ms base for deterministic session timestamps.
    session_epoch_ms: Option<i64>,
    /// Diagnostic journal path; None disables the journal (tests default).
    diag_path: Option<PathBuf>,
    /// The connection log's backing file; None keeps it in memory only.
    connection_log_path: Option<PathBuf>,
    /// Only the tray daemon owns `sessions_path`. A CLI App must not write the
    /// history while the daemon may be alive.
    session_history_owner: bool,
}

impl AppBuilder {
    pub fn new(config_dir: PathBuf) -> Self {
        let marker_path = config_dir.join("tunnel.toml");
        let test = crate::root::test_guard_active();
        let polkit_rules_path = if test {
            config_dir.join(crate::polkit::RULE_FILE)
        } else {
            PathBuf::from(crate::polkit::RULES_PATH)
        };
        let installed_conf_path = if test {
            config_dir.join("jafn0.conf")
        } else {
            PathBuf::from(format!("/etc/wireguard/{IFACE}.conf"))
        };
        let teardown_iface_path = if test {
            config_dir.join("jafn0.iface")
        } else {
            PathBuf::from(format!("/sys/class/net/{IFACE}"))
        };
        let sessions_path = config_dir.join("sessions.toml");
        // A test can never reach the host's real pkcheck: the default is a
        // path under the config dir that does not exist.
        let pkcheck_bin = if test {
            config_dir.join("pkcheck")
        } else {
            PathBuf::from("pkcheck")
        };
        // Same for nmcli: a test never asks the host's NetworkManager to scan.
        let nmcli_bin = if test {
            config_dir.join("nmcli")
        } else {
            PathBuf::from("nmcli")
        };
        Self {
            config_dir,
            iface_pin: None,
            ip_bin: PathBuf::from("ip"),
            link_iface: None,
            link_state: None,
            uplink_sysfs: PathBuf::from("/sys/class/net"),
            wg_bin: PathBuf::from("wg"),
            pkexec_bin: PathBuf::from("pkexec"),
            pkcheck_bin,
            nmcli_bin,
            helper_bin: PathBuf::from(crate::root::HELPER_PATH),
            route_bin: PathBuf::from(crate::root::ROUTE_PATH),
            wg_quick_bin: PathBuf::from("wg-quick"),
            install_bin: PathBuf::from("install"),
            ufw_bin: PathBuf::from("ufw"),
            ufw_conf: PathBuf::from(UFW_CONF),
            auto_accept_delay: AUTO_ACCEPT_DELAY,
            rx_bytes_path: PathBuf::from(RX_BYTES_PATH),
            tx_bytes_path: PathBuf::from(TX_BYTES_PATH),
            proc_wireless_path: PathBuf::from(PROC_WIRELESS_PATH),
            station: None,
            logger: None,
            marker_path,
            polkit_rules_path,
            installed_conf_path,
            teardown_iface_path,
            sessions_path,
            session_epoch_ms: None,
            diag_path: None,
            // Beside the app log; a test process never writes the real one.
            connection_log_path: (!test)
                .then(|| crate::applog::default_path().with_file_name("connection.log")),
            session_history_owner: false,
        }
    }

    /// Where the connection log is kept across runs; None keeps it in memory.
    pub fn connection_log_path(mut self, path: Option<PathBuf>) -> Self {
        self.connection_log_path = path;
        self
    }

    /// Where the diagnostic journal is written; None disables it.
    pub fn diag_path(mut self, path: Option<PathBuf>) -> Self {
        self.diag_path = path;
        self
    }

    pub fn logger(mut self, logger: Option<crate::applog::Logger>) -> Self {
        self.logger = logger;
        self
    }

    pub fn marker_path(mut self, path: PathBuf) -> Self {
        self.marker_path = path;
        self
    }

    pub fn polkit_rules_path(mut self, path: PathBuf) -> Self {
        self.polkit_rules_path = path;
        self
    }

    pub fn installed_conf_path(mut self, path: PathBuf) -> Self {
        self.installed_conf_path = path;
        self
    }

    pub fn teardown_iface_path(mut self, path: PathBuf) -> Self {
        self.teardown_iface_path = path;
        self
    }

    /// Where completed sessions are stored; tests inject a temp file.
    pub fn sessions_path(mut self, path: PathBuf) -> Self {
        self.sessions_path = path;
        self
    }

    /// Let this App write the session history. Only the tray daemon does; a
    /// CLI App must leave the daemon's live record alone.
    pub fn session_history_owner(mut self, owner: bool) -> Self {
        self.session_history_owner = owner;
        self
    }

    /// Fixed epoch-ms base for session timestamps. Tests use it so
    /// `sample_session`/`tick_at` produce deterministic wall-clock values.
    pub fn session_epoch_ms(mut self, epoch_ms: i64) -> Self {
        self.session_epoch_ms = Some(epoch_ms);
        self
    }

    pub fn proc_wireless_path(mut self, path: PathBuf) -> Self {
        self.proc_wireless_path = path;
        self
    }

    pub fn station_source(mut self, source: Box<dyn crate::station::StationSource>) -> Self {
        self.station = Some(source);
        self
    }

    pub fn rx_bytes_path(mut self, path: PathBuf) -> Self {
        self.rx_bytes_path = path;
        self
    }

    pub fn tx_bytes_path(mut self, path: PathBuf) -> Self {
        self.tx_bytes_path = path;
        self
    }

    pub fn auto_accept_delay(mut self, delay: Duration) -> Self {
        self.auto_accept_delay = delay;
        self
    }

    pub fn wg_bin(mut self, wg_bin: PathBuf) -> Self {
        self.wg_bin = wg_bin;
        self
    }

    pub fn iface_pin(mut self, iface: std::net::Ipv4Addr) -> Self {
        self.iface_pin = Some(iface);
        self
    }

    pub fn ip_bin(mut self, ip_bin: PathBuf) -> Self {
        self.ip_bin = ip_bin;
        self
    }

    /// Test seam: the `nmcli` asked to scan while there is no network.
    pub fn nmcli_bin(mut self, nmcli_bin: PathBuf) -> Self {
        self.nmcli_bin = nmcli_bin;
        self
    }

    /// Test seam: the uplink interface name, skipping the route lookup.
    pub fn link_iface(mut self, iface: Option<String>) -> Self {
        self.link_iface = iface;
        self
    }

    /// Test seam: uplink state, replacing the carrier + address read.
    pub fn link_state(mut self, state: LinkStateFn) -> Self {
        self.link_state = Some(state);
        self
    }

    /// Where the carrier file lives; production is `/sys/class/net`.
    pub fn uplink_sysfs(mut self, path: PathBuf) -> Self {
        self.uplink_sysfs = path;
        self
    }

    pub fn pkexec_bin(mut self, pkexec_bin: PathBuf) -> Self {
        self.pkexec_bin = pkexec_bin;
        self
    }

    /// The polkit authorization check (production: `pkcheck`).
    pub fn pkcheck_bin(mut self, pkcheck_bin: PathBuf) -> Self {
        self.pkcheck_bin = pkcheck_bin;
        self
    }

    pub fn helper_bin(mut self, helper_bin: PathBuf) -> Self {
        self.helper_bin = helper_bin;
        self
    }

    pub fn route_bin(mut self, route_bin: PathBuf) -> Self {
        self.route_bin = route_bin;
        self
    }

    pub fn wg_quick_bin(mut self, wg_quick_bin: PathBuf) -> Self {
        self.wg_quick_bin = wg_quick_bin;
        self
    }

    pub fn install_bin(mut self, install_bin: PathBuf) -> Self {
        self.install_bin = install_bin;
        self
    }

    pub fn ufw_bin(mut self, ufw_bin: PathBuf) -> Self {
        self.ufw_bin = ufw_bin;
        self
    }

    /// Where ufw's on/off switch is read from (tests point it at a temp file).
    pub fn ufw_conf(mut self, ufw_conf: PathBuf) -> Self {
        self.ufw_conf = ufw_conf;
        self
    }

    pub fn build(mut self) -> App {
        let state = crate::settings::load_state_or_preserve(&self.config_dir.join("state.toml"));
        let session_epoch = Instant::now();
        let session_epoch_ms_base = self.session_epoch_ms.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0)
        });
        let session = SessionTracker::new(role_name(state.settings.role), session_epoch_ms_base);
        let session_history = SessionHistory::load(&self.sessions_path);
        let diag = self
            .diag_path
            .as_ref()
            .and_then(|path| match Diag::open(path) {
                Ok(diag) => Some(diag),
                Err(e) => {
                    eprintln!("warning: cannot write the diagnostic journal: {e}");
                    None
                }
            });
        let connection_log = match self.connection_log_path.clone() {
            Some(path) => ConnectionLog::persistent(path),
            None => ConnectionLog::new(),
        };
        App {
            orchestrator: Orchestrator::new(),
            discovery: None,
            handshake: None,
            pake: None,
            known: None,
            adopting: false,
            marker_reuse_blocked: false,
            peer_wg_fresh: false,
            tunnel_reused: false,
            no_network_since: None,
            known_send_fault: None,
            no_network_refresh_at: None,
            wifi_scan: crate::wifi_scan::WifiScanNudge::new(self.nmcli_bin.clone()),
            owns_tunnel: false,
            foreign_tunnel: false,
            live_peer_key: None,
            live_peer_key_checked: false,
            host_dns_override: crate::host_dns::find_on_host().map(|o| o.line()),
            resolved_present: resolved_present(
                &tool_present,
                resolved_running(std::path::Path::new("/run/systemd/resolve")),
            ),
            use_known: false,
            keypair: None,
            local_code: None,
            settings: state.settings.clone(),
            state,
            config_dir: self.config_dir.clone(),
            root: RootCommands {
                pkexec_bin: self.pkexec_bin.clone(),
                wg_quick_bin: self.wg_quick_bin.clone(),
                wg_bin: self.wg_bin.clone(),
                install_bin: self.install_bin.clone(),
                ufw_bin: self.ufw_bin.clone(),
                helper_bin: self.helper_bin.clone(),
                route_bin: self.route_bin.clone(),
                interface: IFACE.to_string(),
                teardown_conf: self.installed_conf_path.clone(),
                teardown_iface: self.teardown_iface_path.clone(),
                teardown_rules: self.polkit_rules_path.clone(),
            },
            backend: WgBackend {
                wg_bin: self.wg_bin.clone(),
            },
            iface_pin: self.iface_pin,
            ip_bin: self.ip_bin.clone(),
            pkcheck_bin: self.pkcheck_bin.clone(),
            peer_public_key: None,
            peer_endpoint: None,
            last_announce: None,
            last_error: None,
            auto_accept_deadline: None,
            auto_accept_delay: self.auto_accept_delay,
            rx_bytes_path: self.rx_bytes_path.clone(),
            station: match self.station.take() {
                Some(station) => station,
                None => Box::new(crate::station::NlStation::new(
                    self.proc_wireless_path.clone(),
                )),
            },
            station_ring: jafnify_core::wifi_range::StationRing::new(),
            last_link_log_ms: None,
            range_trend: jafnify_core::tiles::TrendBuffer::new(),
            last_range_sample: None,
            tx_bytes_path: self.tx_bytes_path.clone(),
            rx_watch: None,
            link_watch: None,
            link_lost: false,
            last_link_sample: None,
            fallback_at: None,
            routing_down: false,
            restore_at: None,
            live_watch: None,
            last_probe: None,
            probe_no_ping: false,
            last_route_toggle: None,
            answered_since_toggle: false,
            link_back_at: None,
            fallback_rule_checked: false,
            link_iface: self.link_iface.clone(),
            link_state: self.link_state.clone(),
            uplink_sysfs: self.uplink_sysfs.clone(),
            peer_reset_hint: false,
            reset_deadline: None,
            reset_neutral: false,
            reset_neutral_at: None,
            reset_probe_sent: false,
            search_ifaces: Vec::new(),
            search_recheck_at: None,
            connecting_since: None,
            pake_wrong_code_logged: false,
            denied: false,
            connected_since: None,
            conn_clock: jafnify_core::connclock::ConnectedClock::default(),
            clock_origin: Instant::now(),
            last_peer_proof: None,
            polkit_prompt_expected: false,
            session,
            connection_log,
            last_traffic: None,
            session_history,
            sessions_path: self.sessions_path.clone(),
            session_history_owner: self.session_history_owner,
            last_live_save: None,
            resume_candidate: None,
            usage_on: false,
            usage_sampler: UsageSampler::new(),
            usage_store: UsageStore::load(&self.config_dir.join("usage.tsv")),
            drop_history_on: false,
            drop_store: DropStore::load(&self.config_dir.join("drops.tsv")),
            phone_context: None,
            drop_open: None,
            recovery: None,
            peer_stopped: false,
            outage_reason: None,
            away_quiet: false,
            tile_scopes: crate::tile_scopes::load(
                &self.config_dir.join(crate::tile_scopes::FILE_NAME),
            ),
            home_layout: crate::home_layout::load(
                &self.config_dir.join(jafnify_core::home_layout::FILE_NAME),
            ),
            logged_drop: None,
            best_spot: BestSpotState::default(),
            good_hours_on: false,
            good_hours_ready: false,
            good_hours_sampler: good_hours::Sampler::new(),
            good_hours_store: GoodHoursStore::load(&self.config_dir.join("good_hours.tsv")),
            session_epoch,
            session_epoch_ms_base,
            firewall: None,
            firewall_verdict: Verdict::Unknown,
            firewall_iface: None,
            ufw_conf: self.ufw_conf.clone(),
            logger: self.logger.take(),
            disconnected_at: None,
            marker_path: self.marker_path.clone(),
            installed_conf_path: self.installed_conf_path.clone(),
            reconnect_rx: None,
            suppress_job: None,
            last_suppress_check: None,
            suppress_retry_at: None,
            suppress_restore_pending: false,
            diag,
            last_peer_rx: None,
            tunnel_dead: false,
            last_diag_stats: None,
            last_diag_rx: None,
            handshake_age: HandshakeAge::Unknown,
            handshake_job: None,
            last_handshake_query: None,
            viewer_open: false,
            diag_egress: EgressSnapshot::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jafnify_core::snapshot::PeerPath;

    #[test]
    fn an_expired_code_track_is_reported_until_a_pairing_completes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        let code = PairingCode::parse("1234").unwrap();
        app.orchestrator_mut()
            .start_search_at(code, now.checked_sub(Duration::from_secs(601)).unwrap());
        assert_eq!(app.state(), ConnectionState::Searching);
        assert!(app.code_expired_at(now));

        app.orchestrator_mut().confirm_connected();
        assert!(
            !app.code_expired_at(now),
            "a completed pairing is never told the code expired"
        );
    }

    #[test]
    fn health_uses_the_handshake_age_then_the_last_data_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.orchestrator_mut().confirm_connected();

        app.handshake_age = HandshakeAge::Seconds(10);
        assert_eq!(app.health_at(now).path, PeerPath::DirectHotspot);
        assert_eq!(
            app.health_at(now).line().as_deref(),
            Some("Last handshake 10 s ago - Direct over hotspot")
        );

        app.handshake_age = HandshakeAge::Seconds(181);
        assert_eq!(app.health_at(now).path, PeerPath::Unreachable);
        app.handshake_age = HandshakeAge::Never;
        assert_eq!(
            app.health_at(now).path,
            PeerPath::WaitingForHandshake,
            "a tunnel with no handshake yet is not unreachable"
        );

        app.handshake_age = HandshakeAge::Unknown;
        app.last_peer_rx = Some(now.checked_sub(Duration::from_secs(5)).unwrap());
        assert_eq!(
            app.health_at(now).path,
            PeerPath::DirectHotspot,
            "unknown handshake falls back to the last received data"
        );
        app.last_peer_rx = None;
        assert_eq!(app.health_at(now).path, PeerPath::Unknown);
        assert_eq!(app.health_at(now).line(), None);
    }

    #[test]
    fn the_handshake_age_is_asked_only_while_a_window_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.orchestrator_mut().confirm_connected();

        app.handshake_poll(now);
        assert!(app.handshake_job.is_none(), "no window: no root call");
        assert!(app.last_handshake_query.is_none());

        app.set_viewer_open(true);
        app.handshake_poll(now);
        assert!(app.handshake_job.is_some(), "an open window asks at once");

        app.handshake_age = HandshakeAge::Seconds(10);
        app.set_viewer_open(false);
        app.handshake_poll(now + HANDSHAKE_POLL_INTERVAL);
        assert!(app.handshake_job.is_none());
        assert!(app.last_handshake_query.is_none());
        assert_eq!(
            app.handshake_age,
            HandshakeAge::Unknown,
            "a closed window drops the age instead of showing a stale one later"
        );
    }

    #[test]
    fn a_pairing_save_error_survives_a_connected_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        app.last_error = Some("could not save the pairing: disk full".to_string());
        app.orchestrator_mut().confirm_connected();
        app.track_connected(Instant::now());
        assert_eq!(
            app.last_error.as_deref(),
            Some("could not save the pairing: disk full")
        );
    }

    #[test]
    fn a_root_error_is_cleared_by_a_connected_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        app.last_error = Some("The kill switch needs iptables or nft".to_string());
        app.orchestrator_mut().confirm_connected();
        app.track_connected(Instant::now());
        assert_eq!(app.last_error, None);
    }

    #[test]
    fn the_polkit_hint_is_armed_only_around_an_uncovered_tunnel_call() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        assert!(!app.polkit_prompt_expected());

        app.arm_polkit_hint("tunnel up", false);
        assert!(
            app.polkit_prompt_expected(),
            "uncovered up expects a prompt"
        );
        app.clear_polkit_hint();
        assert!(!app.polkit_prompt_expected(), "the call clears the hint");

        app.arm_polkit_hint("tunnel up", true);
        assert!(
            !app.polkit_prompt_expected(),
            "a covering rule means no prompt"
        );

        app.arm_polkit_hint("tunnel up", false);
        assert!(app.polkit_prompt_expected());
        app.arm_polkit_hint("ufw allow", false);
        assert!(
            app.polkit_prompt_expected(),
            "an unrelated ufw call leaves the attempt note alone"
        );
        app.arm_polkit_hint("kill switch down", false);
        assert!(app.polkit_prompt_expected());

        app.arm_polkit_hint("tunnel inspect", false);
        assert!(
            !app.polkit_prompt_expected(),
            "inspect is not a tunnel prompt"
        );
        app.arm_polkit_hint("tunnel endpoint", false);
        assert!(
            !app.polkit_prompt_expected(),
            "endpoint is not a tunnel prompt"
        );
        app.arm_polkit_hint("teardown", false);
        assert!(app.polkit_prompt_expected());

        app.reset_session(Instant::now());
        assert!(!app.polkit_prompt_expected(), "reset clears a stale hint");
    }

    #[test]
    fn the_drop_history_is_only_written_while_the_flag_is_on() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        let path = dir.path().join(crate::drop_store::FILE_NAME);

        app.set_drop_history_enabled(false);
        app.record_drop("no data from peer for 20 s", 1_712_326_800_000, now);
        assert!(app.drop_records().is_empty());
        assert!(!path.exists(), "the flag is off: no file is written");

        app.set_drop_history_enabled(true);
        app.record_drop("no data from peer for 20 s", 1_712_326_800_000, now);
        app.record_back(1_712_326_812_000, 12_000);
        let records = app.drop_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reason, "no-data");
        assert_eq!(records[0].down_ms, Some(12_000));
        assert!(path.exists());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file stays private");
    }

    #[test]
    fn a_drop_keeps_the_phone_context_of_the_last_moment() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);

        app.record_drop("peer disconnected", 1_712_326_800_000, now);
        assert!(app.drop_records()[0].phone.is_none());

        let sample = |age_secs: u64| PhoneContextSample {
            context: PhoneContext {
                net: Some(jafnify_core::phone_stats::NetworkKind::Mobile5gAdv),
                signal: Some(1),
                temp_c_x10: Some(395),
                battery: Some(80),
                rtt_ms: Some(120),
                thr: None,
            },
            age_secs,
            at: now,
            wifi: None,
            wg_loss_pct: Some(9),
            wg_rtt_ms: Some(52),
        };
        app.set_phone_context(Some(sample(2)));
        app.record_drop("peer disconnected", 1_712_326_801_000, now);
        let records = app.drop_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].phone.unwrap().signal, Some(1));
        assert!(records[1].detail(0).contains("likely weak signal"));
        // The phone's WireGuard numbers of that moment are evidence too.
        assert_eq!(records[1].link.phone_loss_pct, Some(9));
        assert_eq!(records[1].link.phone_rtt_ms, Some(52));

        // An old summary is not used: the drop says that there is no phone data.
        app.set_phone_context(Some(sample(40)));
        app.record_drop("peer disconnected", 1_712_326_802_000, now);
        let records = app.drop_records();
        assert!(records[2].phone.is_none());
        assert_eq!(records[2].link.phone_loss_pct, None);
        assert!(records[2].detail(0).contains("phone data: none"));
    }

    #[test]
    fn a_stale_phone_summary_keeps_the_wifi_leg() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);

        // The phone stopped answering, so its summary is old. The Wi-Fi leg
        // is measured on its own and still says how the link behaved.
        app.set_phone_context(Some(PhoneContextSample {
            context: PhoneContext::default(),
            age_secs: 40,
            at: now,
            wifi: Some(WifiLeg::new(80, 25, 30)),
            wg_loss_pct: None,
            wg_rtt_ms: None,
        }));
        app.record_drop("peer disconnected", 1_712_326_800_000, now);
        let records = app.drop_records();
        assert_eq!(records.len(), 1);
        assert!(records[0].phone.is_none(), "the old phone state is dropped");
        assert_eq!(
            records[0].wifi.map(|leg| leg.median_ms),
            Some(80),
            "the Wi-Fi leg of that moment stays"
        );
    }

    #[test]
    fn a_drop_that_cannot_be_written_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        // The config dir is a file: the drop file cannot be written.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "not a dir").unwrap();
        let journal = dir.path().join("journal.log");
        let logger = crate::applog::Logger::open(journal.clone()).unwrap();
        let mut app = AppBuilder::new(blocked).logger(Some(logger)).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);

        app.record_drop("peer disconnected", 1_712_326_800_000, now);
        let text = std::fs::read_to_string(&journal).unwrap();
        assert!(
            text.contains("could not write the drop history"),
            "the failed write must be logged: {text}"
        );
    }

    #[test]
    fn the_peer_that_says_goodbye_is_a_stop_on_the_phone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        // A sharer hands no routing back: no root call in a test.
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();
        assert_eq!(app.state(), ConnectionState::Connected);

        app.peer_said_bye(now);
        let events = app.drop_events();
        assert_eq!(
            events.len(),
            1,
            "one event for the peer leaving: {events:?}"
        );
        assert_eq!(
            events[0].kind(),
            jafnify_core::drop_history::EventKind::Stopped
        );
        assert!(
            app.drop_records().is_empty(),
            "a stop on the phone is not a drop"
        );
        assert_eq!(events[0].down_ms(), None, "the link is still down");

        // The peer is back: the event closes with the down time.
        app.known_connected(now + Duration::from_secs(60));
        let events = app.drop_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].kind(),
            jafnify_core::drop_history::EventKind::Stopped
        );
        assert_eq!(events[0].down_ms(), Some(60_000));
        assert!(
            app.drop_records().is_empty(),
            "a stop on the phone is never a drop"
        );
    }

    #[test]
    fn a_cleared_history_forgets_the_open_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();
        app.peer_said_bye(now);
        assert_eq!(app.drop_events().len(), 1);

        app.clear_drop_history();
        app.known_connected(now + Duration::from_secs(60));
        assert!(
            app.drop_events().is_empty(),
            "a cleared history gets no back line"
        );
        assert!(
            !dir.path().join(crate::drop_store::FILE_NAME).exists(),
            "a cleared history stays empty"
        );
    }

    #[test]
    fn a_new_session_closes_the_open_drop_at_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();
        app.peer_said_bye(now);
        assert_eq!(app.drop_events().len(), 1);

        // A stop, a forget or a restart ends the session and the outage with
        // it: the record is closed at that moment (never "not back yet" for
        // ever) and the next session neither records a drop nor stretches the
        // old one.
        app.reset_session(now + Duration::from_secs(5));
        assert_eq!(app.drop_events().len(), 1, "a new session records no drop");
        app.known_connected(now + Duration::from_secs(60));
        let events = app.drop_events();
        assert_eq!(events.len(), 1);
        let down = events[0].down_ms().expect("the old outage is closed");
        assert!(
            (4_000..=6_000).contains(&down),
            "the outage ends with the session, not at the next connect: {down}"
        );
    }

    #[test]
    fn a_reset_closed_by_a_stop_still_takes_the_word_of_the_phone() {
        // The phone app was updated, the link saw a reset,
        // a stop and a new session followed. The open drop is closed at the
        // stop and the "updated" word of the phone that comes after the new
        // connect still reclassifies it.
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();
        app.open_drop("peer session reset; no data for 8 s", now);
        app.reset_session(now + Duration::from_secs(2));
        app.known_connected(now + Duration::from_secs(5));
        assert!(
            app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(8)).used(),
            "the word of the phone is still used"
        );
        let events = app.drop_events();
        assert_eq!(events.len(), 1);
        assert!(
            !events[0].is_drop(),
            "an app update is not a drop: {events:?}"
        );
        assert!(
            events[0].down_ms().is_some(),
            "and it is closed: {events:?}"
        );
    }

    #[test]
    fn an_absence_longer_than_the_edge_is_not_a_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        // The phone cannot be reached: the hotspot of the phone went away.
        app.open_drop("no network", now);
        // Two minutes later the outage is an absence, not a fault.
        app.update_away(now + Duration::from_secs(121));
        let events = app.drop_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            events[0].kind(),
            jafnify_core::drop_history::EventKind::Away
        );
        assert!(app.drop_records().is_empty(), "an absence is not a drop");

        // The phone comes back: the absence is closed with its length.
        app.close_drop(now + Duration::from_secs(660));
        let events = app.drop_events();
        assert_eq!(events.len(), 1, "the absence is one record");
        assert_eq!(events[0].down_ms(), Some(660_000));
        let detail = events[0].detail(0);
        assert!(
            detail.contains(jafnify_core::drop_history::AWAY_LABEL),
            "{detail}"
        );
        assert!(
            detail.contains(jafnify_core::drop_history::NOT_A_DROP),
            "{detail}"
        );
    }

    #[test]
    fn a_short_absence_stays_a_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("no network", now);
        // One second short of the edge: the loss is a fault.
        app.update_away(now + Duration::from_secs(119));
        let records = app.drop_records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].reason, "no-network");
        assert_eq!(records[0].hint(), "likely the phone's hotspot went away");
    }

    #[test]
    fn a_loss_of_data_stays_a_drop_however_long_it_lasted() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("no data from peer for 35 s", now);
        app.update_away(now + Duration::from_secs(600));
        let records = app.drop_records();
        assert_eq!(records.len(), 1, "a fault of the link is a drop");
        assert_eq!(records[0].reason, "no-data");
    }

    #[test]
    fn a_quick_reset_is_neutral_and_an_update_writes_one_neutral_line() {
        // A real install gap: the peer session reset, the
        // phone came back after ten seconds, and its first summary said the
        // app was updated. The outage must never become a drop.
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.note_peer_reset(now);
        assert_eq!(app.status_text_at(now), RECONNECTING);
        assert_eq!(app.state(), ConnectionState::Connected);

        // The peer is back with a new session inside the window; its word
        // arrives with the first summary shortly after.
        app.peer_restarted(now + Duration::from_secs(10));
        assert_eq!(
            app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(11)),
            NoteExit::Updated
        );
        assert!(
            app.drop_records().is_empty(),
            "an update is not a drop: {:?}",
            app.drop_records()
        );
        let events = app.drop_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            events[0].kind(),
            jafnify_core::drop_history::EventKind::Updated
        );
        assert!(
            events[0].detail(0).contains(drop_history::UPDATED_LABEL),
            "{}",
            events[0].detail(0)
        );
    }

    #[test]
    fn a_quick_reset_with_a_crash_word_is_a_drop_and_needs_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.note_peer_reset(now);
        app.peer_restarted(now + Duration::from_secs(10));
        assert_eq!(
            app.note_phone_exit(ExitReason::Crash, now + Duration::from_secs(11)),
            NoteExit::FaultNow,
            "a quick crash is recorded now and the desktop must hear it"
        );
        let records = app.drop_records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].reason, drop_history::REASON_RESET);
        assert_eq!(records[0].hint(), "likely the phone app stopped");
        assert_eq!(records[0].down_ms, Some(10_000));
        assert!(
            app.connection_log
                .tail(50)
                .iter()
                .any(|line| line.contains("disconnected at")),
            "the drop is in the connection log: {:?}",
            app.connection_log.tail(50)
        );
    }

    #[test]
    fn a_quick_reset_without_any_word_stays_neutral() {
        // An old app that does not report why it ended: the neutral window
        // must not blame the link for what may have been an update.
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.note_peer_reset(now);
        app.peer_restarted(now + Duration::from_secs(10));
        assert_eq!(
            app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(60) * 2),
            NoteExit::Unused
        );
        assert!(
            app.drop_records().is_empty(),
            "no word, no drop: {:?}",
            app.drop_records()
        );
    }

    #[test]
    fn a_reset_that_never_returns_is_a_drop_from_the_reset_moment() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        let start_ms = app.session_epoch_ms_at(now);
        app.note_peer_reset(now);
        // The window closes with the peer still away: a real loss.
        app.known_lost_after_reset_at(
            "peer session reset; no data for 12 s".to_string(),
            Some(start_ms),
            now + RESET_NEUTRAL_GRACE,
        );
        let records = app.drop_records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].reason, drop_history::REASON_RESET);
        assert_eq!(records[0].at_ms, start_ms, "the outage began at the reset");

        // The phone returns later and says it was an update: the long outage
        // is still reclassified and not counted.
        app.close_drop(now + RESET_NEUTRAL_GRACE + Duration::from_secs(2));
        assert_eq!(
            app.note_phone_exit(
                ExitReason::Updated,
                now + RESET_NEUTRAL_GRACE + Duration::from_secs(3)
            ),
            NoteExit::Updated
        );
        assert!(
            app.drop_records().is_empty(),
            "a slow update is still an update: {:?}",
            app.drop_records()
        );
    }

    #[test]
    fn an_app_update_reclassifies_the_session_reset_after_the_reconnect() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("peer session reset; no data for 8 s", now);
        app.close_drop(now + Duration::from_secs(3));
        assert_eq!(app.drop_records().len(), 1, "the drop is recorded first");

        // The phone's first summary of the new session says it was updated.
        assert!(app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(4)).used());
        let events = app.drop_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            events[0].kind(),
            jafnify_core::drop_history::EventKind::Updated
        );
        assert_eq!(events[0].down_ms(), Some(3_000));
        assert!(
            app.drop_records().is_empty(),
            "an update is not counted as a drop"
        );
    }

    #[test]
    fn a_crash_keeps_the_drop_and_carries_the_phone_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("no data from peer for 35 s", now);
        app.close_drop(now + Duration::from_secs(12));
        assert!(app.note_phone_exit(ExitReason::Crash, now + Duration::from_secs(13)).used());
        let records = app.drop_records();
        assert_eq!(records.len(), 1, "a crash stays a drop");
        assert_eq!(records[0].hint(), "likely the phone app stopped");
    }

    #[test]
    fn an_update_does_not_explain_a_peer_left_outage() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("peer disconnected", now);
        app.close_drop(now + Duration::from_secs(3));
        assert!(!app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(4)).used());
        assert_eq!(app.drop_records().len(), 1, "a peer-left drop stays a drop");
    }

    #[test]
    fn an_exit_word_that_arrives_too_late_explains_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("peer session reset; no data for 8 s", now);
        app.close_drop(now + Duration::from_secs(3));
        assert!(!app
            .note_phone_exit(
                ExitReason::Updated,
                now + Duration::from_secs(3) + EXIT_GRACE + Duration::from_secs(1)
            )
            .used());
        assert_eq!(
            app.drop_records().len(),
            1,
            "an old reason must never rewrite a fault"
        );
    }

    #[test]
    fn the_exit_word_is_handled_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        let now = Instant::now();
        app.set_drop_history_enabled(true);
        app.set_role(Role::Sharer);
        app.orchestrator_mut().confirm_connected();

        app.open_drop("peer session reset; no data for 8 s", now);
        app.close_drop(now + Duration::from_secs(3));
        assert!(app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(4)).used());
        assert!(!app.note_phone_exit(ExitReason::Updated, now + Duration::from_secs(5)).used());
        assert_eq!(app.drop_events().len(), 1, "one outage, one record");
    }

    fn app_with_a_pair(dir: &std::path::Path) -> (App, String) {
        let mut app = AppBuilder::new(dir.to_path_buf())
            .sessions_path(dir.join("sessions.toml"))
            .build();
        let peer = jafnify_core::KeyPair::generate();
        let rec = pair_record(
            &jafnify_core::KeyPair::generate(),
            &peer.public_key_b64(),
            Role::Joiner,
            None,
            1,
        )
        .unwrap();
        app.state.peers = vec![rec];
        (app, peer.public_key_b64())
    }

    #[test]
    fn a_connect_inside_the_grace_continues_the_stopped_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = app_with_a_pair(dir.path());
        let now = Instant::now();
        let id = app.current_session().id;
        app.end_session(now);
        let candidate = app.resume_candidate_at(now);
        app.reset_session(now);
        app.resume_candidate = candidate;
        let at = now + Duration::from_millis(jafnify_core::scope::RESUME_GRACE_MS as u64);
        assert!(app.try_resume(at), "the edge of the grace still continues");
        assert_eq!(app.current_session().id, id);
        assert!(app.resume_candidate.is_none(), "a candidate is used once");
    }

    #[test]
    fn a_connect_after_the_grace_starts_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = app_with_a_pair(dir.path());
        let now = Instant::now();
        let id = app.current_session().id;
        let candidate = app.resume_candidate_at(now);
        app.reset_session(now);
        app.resume_candidate = candidate;
        let late = now + Duration::from_millis(jafnify_core::scope::RESUME_GRACE_MS as u64 + 1);
        assert!(!app.try_resume(late));
        assert_ne!(app.current_session().id, id);
    }

    #[test]
    fn another_device_or_a_forget_starts_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = app_with_a_pair(dir.path());
        let now = Instant::now();
        let id = app.current_session().id;
        let mut candidate = app.resume_candidate_at(now).unwrap();
        candidate.peer_key = "another device".to_string();
        app.reset_session(now);
        app.resume_candidate = Some(candidate);
        assert!(!app.try_resume(now + Duration::from_secs(3)));
        assert_ne!(app.current_session().id, id);

        // Every reset that is not the stop itself (forget, a restart) drops
        // the candidate, so the next Connect is a new session.
        let candidate = app.resume_candidate_at(now);
        app.resume_candidate = candidate;
        app.reset_session(now);
        assert!(app.resume_candidate.is_none());
    }

    fn station_app(
        dir: &std::path::Path,
        iface: &str,
    ) -> (
        App,
        std::sync::Arc<std::sync::Mutex<Option<jafnify_core::wifi_range::StationSample>>>,
    ) {
        let next = std::sync::Arc::new(std::sync::Mutex::new(None));
        let app = AppBuilder::new(dir.to_path_buf())
            .station_source(Box::new(crate::station::FakeStation {
                iface: iface.to_string(),
                next: next.clone(),
            }))
            .build();
        (app, next)
    }

    #[test]
    fn the_wifi_link_is_read_for_the_uplink_and_kept_with_a_drop() {
        use jafnify_core::wifi_range::StationSample;
        let dir = tempfile::tempdir().unwrap();
        let (mut app, next) = station_app(dir.path(), "wlo1");
        app.set_drop_history_enabled(true);
        app.set_role(Role::Joiner);
        app.orchestrator_mut().confirm_connected();
        app.link_watch = Some(crate::uplink::LinkWatch::new("wlo1".to_string()));
        let now = Instant::now();

        // The laptop walks away: the level and the rate fall, retries and
        // missed beacons grow, once a second.
        let walk = [
            (-62, 8667, 100, 0),
            (-70, 5850, 140, 0),
            (-76, 1300, 260, 2),
            (-81, 65, 420, 7),
        ];
        for (i, (avg, rate, retries, bloss)) in walk.iter().enumerate() {
            *next.lock().unwrap() = Some(StationSample {
                signal_dbm: Some(avg + 2),
                signal_avg_dbm: Some(*avg),
                tx_rate_100k: Some(*rate),
                tx_retries: Some(*retries),
                tx_failed: Some(0),
                beacon_loss: Some(*bloss),
            });
            app.range_poll(now + Duration::from_secs(i as u64));
        }
        assert_eq!(app.range_now(now + Duration::from_secs(3)), Some(-81));
        assert_eq!(app.range_trend().len(), 4);
        // A reading twice in the same second is one sample.
        app.range_poll(now + Duration::from_millis(3_500));
        assert_eq!(app.range_trend().len(), 4);
        // An old level is no level.
        assert_eq!(app.range_now(now + Duration::from_secs(10)), None);

        app.open_drop("no network", now + Duration::from_secs(5));
        let records = app.drop_records();
        assert_eq!(records.len(), 1);
        let link = records[0].link;
        assert_eq!(link.weakest_dbm, Some(-81));
        assert_eq!(link.lowest_rate_100k, Some(65));
        assert_eq!(link.retries, Some(320));
        assert_eq!(link.beacon_loss, Some(7));
        assert_eq!(records[0].hint(), drop_history::HINT_OUT_OF_RANGE);

        // The link log holds numbers only: the first reading and one ten
        // seconds later at most, never an address.
        let log = std::fs::read_to_string(dir.path().join("link.tsv")).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.starts_with("JFL1 1 at="), "{log}");
        assert!(!log.contains(':'), "no MAC address in the log: {log}");
    }

    #[test]
    fn an_open_drop_at_a_weak_level_returns_the_range_hint() {
        use jafnify_core::wifi_range::StationSample;
        let dir = tempfile::tempdir().unwrap();
        let (mut app, next) = station_app(dir.path(), "wlo1");
        app.set_drop_history_enabled(true);
        app.set_role(Role::Joiner);
        app.orchestrator_mut().confirm_connected();
        app.link_watch = Some(crate::uplink::LinkWatch::new("wlo1".to_string()));
        let now = Instant::now();
        *next.lock().unwrap() = Some(StationSample {
            signal_avg_dbm: Some(-81),
            ..Default::default()
        });
        app.range_poll(now);
        let hint = app.open_drop("no network", now + Duration::from_secs(1));
        assert_eq!(hint, Some(drop_history::HINT_OUT_OF_RANGE));
        assert_eq!(app.drop_records()[0].link.weakest_dbm, Some(-81));
    }

    #[test]
    fn an_open_drop_without_a_weak_level_names_the_hotspot() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _next) = station_app(dir.path(), "wlo1");
        app.set_drop_history_enabled(true);
        app.set_role(Role::Joiner);
        app.orchestrator_mut().confirm_connected();
        let now = Instant::now();
        let hint = app.open_drop("no network", now + Duration::from_secs(1));
        assert_eq!(hint, Some(drop_history::HINT_HOTSPOT_GONE));
    }

    #[test]
    fn no_wifi_uplink_is_no_level() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, next) = station_app(dir.path(), "wlo1");
        *next.lock().unwrap() = Some(jafnify_core::wifi_range::StationSample::level_only(-50));
        app.orchestrator_mut().confirm_connected();
        // A cable or a lab veth: not a Wi-Fi link.
        app.link_watch = Some(crate::uplink::LinkWatch::new("veth1".to_string()));
        app.range_poll(Instant::now());
        assert!(app.range_trend().is_empty());
        assert!(!dir.path().join("link.tsv").exists());
        // Not connected: nothing is read.
        let (mut idle, _) = station_app(dir.path(), "wlo1");
        idle.range_poll(Instant::now());
        assert!(idle.range_trend().is_empty());
    }

    /// A Connected app with fresh authenticated peer activity at `now`.
    fn fresh_connected_app(now: Instant) -> App {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf()).build();
        app.orchestrator_mut().confirm_connected();
        app.last_peer_proof = Some(now);
        app.last_peer_rx = Some(now);
        app.track_connected(now);
        app
    }

    #[test]
    fn the_connected_clock_starts_with_the_first_fresh_tick() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        // Off the second boundary: the clock counts whole milliseconds.
        let later = now + Duration::from_millis(5_200);
        app.last_peer_proof = Some(later);
        app.track_connected(later);
        assert_eq!(app.connected_secs_at(later), Some(5));
    }

    #[test]
    fn a_short_gap_shows_reconnecting_and_restarts_the_clock() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        let gone = now + FRESH_GAP + Duration::from_secs(1);
        assert_eq!(app.status_text_at(gone), RECONNECTING);
        app.track_connected(gone);
        assert_eq!(app.connected_secs_at(gone), None);
        let back = gone + Duration::from_secs(1);
        app.last_peer_proof = Some(back);
        app.track_connected(back);
        assert_eq!(
            app.connected_secs_at(back + Duration::from_millis(2_200)),
            Some(2)
        );
    }

    #[test]
    fn a_carrier_loss_is_a_neutral_display_and_no_drop() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        app.link_lost = true;
        assert_eq!(app.status_text_at(now), LINK_LOST);
        app.track_connected(now + Duration::from_secs(1));
        assert_eq!(app.connected_secs_at(now + Duration::from_secs(1)), None);
        assert_eq!(app.state(), ConnectionState::Connected);
        assert_eq!(app.current_session().drops, 0);
    }

    #[test]
    fn the_phone_value_wins_over_the_local_clock() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        app.observe_phone_connected_ms(Some(30_000), now);
        let later = now + Duration::from_millis(2_200);
        app.last_peer_proof = Some(later);
        assert_eq!(app.connected_secs_at(later), Some(32));
    }

    #[test]
    fn a_phone_session_rewind_restarts_the_session_bytes_with_the_up_time() {
        let dir = tempfile::tempdir().unwrap();
        let rx = dir.path().join("rx");
        let tx = dir.path().join("tx");
        std::fs::write(&rx, "0").unwrap();
        std::fs::write(&tx, "0").unwrap();
        let mut app = AppBuilder::new(dir.path().to_path_buf())
            .rx_bytes_path(rx.clone())
            .tx_bytes_path(tx.clone())
            .build();
        app.set_usage_enabled(true);
        let now = Instant::now();
        app.orchestrator_mut().confirm_connected();
        app.last_peer_proof = Some(now);
        app.last_peer_rx = Some(now);
        app.track_connected(now);
        app.sample_session(now);
        std::fs::write(&rx, "300").unwrap();
        std::fs::write(&tx, "120").unwrap();
        app.sample_session(now + Duration::from_secs(10));
        assert_eq!(app.current_session().down_bytes, 300);
        assert_eq!(
            app.usage_summaries(now + Duration::from_secs(10), 0)[0].down_bytes,
            300,
            "today counted the old session before the update"
        );
        let old_id = app.current_session().id.clone();

        app.observe_phone_connected_ms(Some(10_000), now + Duration::from_secs(10));
        app.observe_phone_connected_ms(Some(1_000), now + Duration::from_secs(12));

        let fresh = app.current_session();
        assert_ne!(fresh.id, old_id, "the session follows the phone session");
        assert_eq!(fresh.down_bytes, 0);
        assert_eq!(fresh.up_bytes, 0);
        assert_eq!(app.connected_secs_at(now + Duration::from_secs(12)), Some(1));
        assert_eq!(
            app.usage_summaries(now + Duration::from_secs(12), 0)[0].down_bytes,
            300,
            "the session reset leaves today alone"
        );
        assert_eq!(app.session_history()[0].down_bytes, 300, "the old session is kept");

        app.sample_session(now + Duration::from_secs(12));
        std::fs::write(&rx, "400").unwrap();
        std::fs::write(&tx, "150").unwrap();
        app.sample_session(now + Duration::from_secs(13));
        assert_eq!(app.current_session().down_bytes, 100);
        assert_eq!(app.current_session().up_bytes, 30);
        assert_eq!(
            app.usage_summaries(now + Duration::from_secs(13), 0)[0].down_bytes,
            400,
            "today keeps counting the new bytes"
        );
    }

    /// An app with real counter files and its own session-history path, in
    /// the connected state, so `sample_session` sees traffic.
    fn connected_app(
        dir: &std::path::Path,
    ) -> (App, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let rx = dir.join("rx");
        let tx = dir.join("tx");
        std::fs::write(&rx, "0").unwrap();
        std::fs::write(&tx, "0").unwrap();
        let sessions = dir.join("sessions.toml");
        let mut app = AppBuilder::new(dir.to_path_buf())
            .rx_bytes_path(rx.clone())
            .tx_bytes_path(tx.clone())
            .sessions_path(sessions.clone())
            .session_history_owner(true)
            .build();
        let now = Instant::now();
        app.orchestrator_mut().confirm_connected();
        app.last_peer_proof = Some(now);
        app.last_peer_rx = Some(now);
        app.track_connected(now);
        (app, rx, tx, sessions)
    }

    /// Write counters and take one sample, so the live session has figures.
    fn sample_traffic(
        app: &mut App,
        rx: &std::path::Path,
        tx: &std::path::Path,
        at: Instant,
        down: u64,
        up: u64,
    ) {
        std::fs::write(rx, down.to_string()).unwrap();
        std::fs::write(tx, up.to_string()).unwrap();
        app.sample_session(at);
    }

    #[test]
    fn the_live_session_is_saved_while_running_and_closed_on_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, rx, tx, sessions) = connected_app(dir.path());
        let now = Instant::now();
        app.sample_session(now);
        sample_traffic(&mut app, &rx, &tx, now + Duration::from_secs(10), 300, 120);
        assert!(
            !sessions.exists(),
            "the first sample only starts the interval"
        );

        sample_traffic(&mut app, &rx, &tx, now + Duration::from_secs(70), 400, 150);
        let text = std::fs::read_to_string(&sessions).unwrap();
        assert!(text.contains("live"), "a live record is on disk: {text}");
        assert!(
            text.contains("down_bytes = 400"),
            "the saved live figures: {text}"
        );

        app.close_session_on_exit();
        let kept = SessionHistory::load(&sessions).recent(10);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].end_reason, "App closed");
        assert_eq!(kept[0].down_bytes, 400);
    }

    #[test]
    fn a_stop_closes_the_live_session_once_as_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, rx, tx, sessions) = connected_app(dir.path());
        let now = Instant::now();
        app.sample_session(now);
        sample_traffic(&mut app, &rx, &tx, now + Duration::from_secs(10), 300, 120);
        app.end_session(now + Duration::from_secs(20));

        let kept = SessionHistory::load(&sessions).recent(10);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].end_reason, "stopped");
    }

    #[test]
    fn a_resumed_session_is_written_once_on_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, rx, tx, sessions) = connected_app(dir.path());
        let peer = KeyPair::generate();
        let record = pair_record(
            &KeyPair::generate(),
            &peer.public_key_b64(),
            Role::Joiner,
            None,
            1,
        )
        .unwrap();
        app.state.peers = vec![record];
        let now = Instant::now();
        app.sample_session(now);
        sample_traffic(&mut app, &rx, &tx, now + Duration::from_secs(10), 300, 120);
        app.end_session(now + Duration::from_secs(20));
        let candidate = app.resume_candidate_at(now + Duration::from_secs(20));
        app.reset_session(now + Duration::from_secs(20));
        app.resume_candidate = candidate;
        assert!(app.try_resume(now + Duration::from_secs(21)));

        app.close_session_on_exit();
        let kept = SessionHistory::load(&sessions).recent(10);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].end_reason, "App closed");
    }

    #[test]
    fn a_daemon_start_recovers_the_previous_live_session_once() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions.toml");
        let mut history = SessionHistory::default();
        history.set_live(SessionStats {
            id: "live".to_string(),
            role: "joiner".to_string(),
            started_at: 1_000,
            last_activity_at: 2_000,
            connected_ms: 2_000,
            up_bytes: 80,
            down_bytes: 300,
            ..SessionStats::default()
        });
        history.save(&sessions).unwrap();

        assert!(SessionHistory::recover_unfinished(&sessions).unwrap());
        let first = AppBuilder::new(dir.path().to_path_buf())
            .sessions_path(sessions.clone())
            .session_history_owner(true)
            .build();
        let kept = first.session_history();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].end_reason, "App closed");

        let second = AppBuilder::new(dir.path().to_path_buf())
            .sessions_path(sessions.clone())
            .session_history_owner(true)
            .build();
        assert_eq!(second.session_history().len(), 1);
        assert!(!SessionHistory::recover_unfinished(&sessions).unwrap());
    }

    #[test]
    fn a_cli_style_build_leaves_the_live_record_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions.toml");
        let mut history = SessionHistory::default();
        history.set_live(SessionStats {
            id: "live".to_string(),
            role: "joiner".to_string(),
            started_at: 1_000,
            last_activity_at: 2_000,
            connected_ms: 2_000,
            up_bytes: 80,
            down_bytes: 300,
            ..SessionStats::default()
        });
        history.save(&sessions).unwrap();
        let before = std::fs::read_to_string(&sessions).unwrap();

        let app = AppBuilder::new(dir.path().to_path_buf())
            .sessions_path(sessions.clone())
            .session_history_owner(false)
            .build();
        assert!(app.session_history().is_empty());
        assert_eq!(std::fs::read_to_string(&sessions).unwrap(), before);
    }

    #[test]
    fn an_old_phone_keeps_the_local_clock() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        app.observe_phone_connected_ms(None, now);
        let later = now + Duration::from_millis(3_200);
        app.last_peer_proof = Some(later);
        assert_eq!(app.connected_secs_at(later), Some(3));
    }

    #[test]
    fn the_phone_value_survives_a_short_neutral_gap() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        app.observe_phone_connected_ms(Some(20_000), now);
        let gone = now + FRESH_GAP + Duration::from_secs(1);
        app.track_connected(gone);
        assert_eq!(app.connected_secs_at(gone), None);
        let back = gone + Duration::from_secs(1);
        app.last_peer_proof = Some(back);
        app.track_connected(back);
        // The phone never reset, so its clock kept running through the gap.
        assert_eq!(
            app.connected_secs_at(back + Duration::from_millis(200)),
            Some(34)
        );
    }

    #[test]
    fn an_adopted_tunnel_uses_the_watchdog_verdict_for_the_neutral() {
        let now = Instant::now();
        let mut app = fresh_connected_app(now);
        // A code/adopted tunnel has no known-track heartbeat.
        app.last_peer_proof = None;
        app.last_peer_rx = None;
        app.tunnel_dead = true;
        app.track_connected(now + Duration::from_secs(1));
        assert_eq!(app.status_text_at(now), RECONNECTING);
        assert_eq!(app.state(), ConnectionState::Connected);
        assert_eq!(app.connected_secs_at(now + Duration::from_secs(1)), None);
    }
}
