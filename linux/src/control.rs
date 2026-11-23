//! Local control channel between the window process and the tray daemon.
//!
//! The daemon owns the tunnel and the tray. The window only asks for a state
//! snapshot and sends user commands over a Unix socket in `$XDG_RUNTIME_DIR`
//! (0600, same user). No private key or config is part of the protocol.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use jafnify_core::phone_stats::PhoneStats;
use serde::{Deserialize, Serialize};

use crate::session::{SessionRecord, SessionStats};
use crate::settings::ensure_private_dir;

pub const RUNTIME_SUBDIR: &str = crate::flavor::NAME;
pub const SOCKET_NAME: &str = "control.sock";
pub const LOCK_NAME: &str = "tray.lock";
pub const WINDOW_LOCK_NAME: &str = "window.lock";
/// The window sends a state request at least this often while it is open. A
/// crashed window stops refreshing and the tray comes back after this long.
pub const WINDOW_PRESENCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("control protocol error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the tray process did not answer")]
    Timeout,
    #[error("XDG_RUNTIME_DIR is not set")]
    NoRuntimeDir,
    #[error("the tray process is not installed next to this window")]
    NoDaemon,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    GetState,
    /// Read-only status poll: the daemon answers with a snapshot and does not
    /// mark the window as open.
    Status,
    WindowClosed,
    /// Take the page a tray entry asked for, if any, and clear the hint.
    TakeOpenPage,
    Connect,
    TryAgain,
    Disconnect,
    Pair {
        code: String,
    },
    Forget,
    AllowFirewall,
    AcceptTerms,
    SetSettings {
        role: String,
        autostart: bool,
        kill_switch: bool,
        tray_only_when_closed: bool,
        /// `system`/`dark`/`light`; older windows omit it and keep the saved
        /// theme instead of overwriting it with a default.
        #[serde(default)]
        theme: Option<String>,
        /// Flag values changed by this request. Older windows omit the map,
        /// which means no flag changes.
        #[serde(default)]
        flags: BTreeMap<String, bool>,
        /// The name this computer calls itself; local only.
        #[serde(default)]
        nickname: Option<String>,
    },
    /// Theme-only update: does not touch role, autostart or the kill switch.
    SetTheme {
        theme: String,
    },
    /// One flag switch: writes `flags.toml` only. The same refusal rule as
    /// `SetSettings.flags`; unknown, unwired or unsupported ids are ignored.
    SetFlag {
        id: String,
        value: bool,
    },
    /// The scope of one tile that sums over time: the tile id and the scope
    /// word. The core owns the words and the order; the window only stores
    /// the choice and passes it in.
    SetTileScope {
        id: String,
        scope: String,
    },
    /// The whole home layout, as the saved text. The core validates it and
    /// the daemon writes it; the choice applies at once.
    SetHomeLayout {
        layout: String,
    },
    /// One-shot link measurement. Only while Connected; never background.
    LinkTest,
    SetViewState {
        details_expanded: bool,
        advanced_expanded: bool,
    },
    /// Persist the GUI window's size and, when known, its position.
    SetWindowGeometry {
        window: crate::settings::WindowGeometry,
    },
    /// The redacted diagnostics export: the connection log, the journal tail
    /// and the application log tail.
    Diagnostics,
    /// The drop history of the last day. Read-only: the daemon owns the file.
    GetDrops,
    /// Delete the drop history file. The daemon does the work, so the file
    /// cannot race the daemon's own appends.
    ClearDrops,
    /// Start the best-spot session: the phone is measured for two minutes.
    BestSpotStart,
    /// Mark the spot the phone sits in right now.
    BestSpotMark,
    /// Stop the best-spot session.
    BestSpotStop,
    /// The good hours: the hours the phone's network was usually quick or
    /// usually busy. Read-only: the daemon owns the file.
    GetGoodHours,
    /// Delete the good-hours file and the hour in the making. The daemon does
    /// the work, so the file cannot race the daemon's own appends.
    ClearGoodHours,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    State(Box<Snapshot>),
    Ok {
        note: Option<String>,
    },
    Err {
        message: String,
    },
    /// The page a tray entry asked for, if one is pending. `None` when not.
    Page {
        #[serde(default)]
        page: Option<String>,
    },
    /// The redacted diagnostics export, ready to be pasted into a report.
    Diagnostics {
        text: String,
    },
    /// The one line from a link test, in core wording. `sent` is 0 when no
    /// probe ran (not connected or ICMP denied); older daemons omit the
    /// structured fields, which reads as "no result yet".
    LinkTest {
        line: String,
        #[serde(default)]
        sent: u32,
        #[serde(default)]
        avg_ms: Option<u64>,
        #[serde(default)]
        loss_pct: u32,
    },
    /// The drop history: the summary of the last day and the newest drops,
    /// newest first. Older daemons omit it.
    Drops {
        summary: String,
        #[serde(default)]
        rows: Vec<String>,
    },
    /// The best-spot session: whether it runs, whether it shows its marks
    /// and the lines to show. Older daemons omit it.
    BestSpot {
        running: bool,
        /// A session shows its marks and its verdict, also after its two
        /// minutes are over.
        #[serde(default)]
        active: bool,
        #[serde(default)]
        lines: Vec<String>,
    },
    /// The good hours, in the core wording. Older daemons omit it.
    GoodHours {
        #[serde(default)]
        lines: Vec<String>,
    },
}

/// The laptop's own usage in core wording, one list per window. Older
/// daemons omit it; it appears only while `usage_statistics` is on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageView {
    pub today: Vec<String>,
    pub week: Vec<String>,
    pub month: Vec<String>,
}

/// One read-only host hint: what was detected and what to do about it. The
/// text never carries a network name or an address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHintRow {
    pub title: String,
    pub help: String,
}

/// Everything the window draws. It deliberately has no private key, config
/// text or full public key: the window only shows the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: String,
    pub status_text: String,
    pub paired_name: Option<String>,
    pub paired: bool,
    pub denied: bool,
    pub retry_reason: Option<String>,
    pub no_peer: bool,
    pub connected_secs: Option<u64>,
    pub traffic: Option<(u64, u64)>,
    pub last_connected_ago: Option<u64>,
    pub retry_secs_left: Option<u64>,
    /// The main-screen verdict from jafnify-core, or none.
    #[serde(default)]
    pub health_line: Option<String>,
    /// The handshake age, shown in Details.
    #[serde(default)]
    pub health_detail: Option<String>,
    /// Another Jafnify instance owns jafn0.
    #[serde(default)]
    pub foreign_tunnel: bool,
    /// The local code track expired before pairing completed.
    #[serde(default)]
    pub code_expired: bool,
    pub firewall: Option<String>,
    /// The LAN interface the rule is scoped to; older daemons omit it.
    #[serde(default)]
    pub firewall_iface: Option<String>,
    /// A host-wide resolved override, shown in Details; older daemons omit it.
    #[serde(default)]
    pub dns_override: Option<String>,
    /// The name this computer calls itself; local only, never sent to the peer.
    #[serde(default)]
    pub nickname: String,
    /// How the peer is reached, in the core's words; older daemons omit it.
    #[serde(default)]
    pub path: Option<String>,
    /// True when name lookups on this computer go through the tunnel.
    #[serde(default)]
    pub dns_through_tunnel: bool,
    /// The live peer endpoint; older daemons omit it.
    #[serde(default)]
    pub peer_endpoint: Option<String>,
    pub last_error: Option<String>,
    pub role: String,
    pub autostart: bool,
    pub kill_switch: bool,
    pub tray_only_when_closed: bool,
    #[serde(default)]
    pub details_expanded: bool,
    #[serde(default)]
    pub advanced_expanded: bool,
    pub terms_ok: bool,
    pub ufw_active: bool,
    pub pair_id: Option<String>,
    pub peer_key_prefix: Option<String>,
    #[serde(default)]
    pub current_session: SessionStats,
    #[serde(default)]
    pub session_history: Vec<SessionRecord>,
    /// The real drops of the live session, from the same events the drops
    /// tile counts. Older daemons omit it.
    #[serde(default)]
    pub session_drops: u32,
    #[serde(default)]
    pub connection_log: Vec<String>,
    /// A password prompt is expected for the next privileged tunnel call.
    #[serde(default)]
    pub polkit_note: bool,
    /// While Connected, the uplink carrying the peer route is down.
    #[serde(default)]
    pub link_lost: bool,
    /// While Searching, this computer has no usable network.
    #[serde(default)]
    pub no_network: bool,
    /// `system`/`dark`/`light`; older daemons omit it.
    #[serde(default = "default_theme_name")]
    pub theme: String,
    /// The last saved window geometry; zero when never saved.
    #[serde(default)]
    pub window: crate::settings::WindowGeometry,
    /// Every declared flag with its effective value. Older daemons omit the
    /// map, which reads as every row at its core default.
    #[serde(default)]
    pub flags: BTreeMap<String, bool>,
    /// The scope of each tile that sums over time: tile id to scope word.
    /// Older daemons omit the map, which reads as every tile at its default.
    #[serde(default)]
    pub tile_scopes: BTreeMap<String, String>,
    /// The saved home layout text. Older daemons omit it, which reads as the
    /// platform default.
    #[serde(default)]
    pub home_layout: String,
    /// The last phone summary, if one arrived; older daemons omit it.
    #[serde(default)]
    pub phone_stats: Option<PhoneStats>,
    /// Age of the last successful phone summary, in seconds.
    #[serde(default)]
    pub phone_stats_age_secs: Option<u64>,
    /// Repeated phone timeouts with no value; the phone is probably older.
    #[serde(default)]
    pub phone_stats_unavailable: bool,
    /// The laptop's own summaries while usage statistics are on.
    #[serde(default)]
    pub usage: Option<UsageView>,
    /// The host hints that were detected on this computer; older daemons omit
    /// them.
    #[serde(default)]
    pub host_hints: Vec<HostHintRow>,
    /// The median round trip to the phone, in milliseconds. Older daemons omit
    /// it.
    #[serde(default)]
    pub wifi_ms: Option<u64>,
    /// How many percent of the measured requests were lost.
    #[serde(default)]
    pub wifi_loss_pct: Option<u64>,
    /// How many requests the measurement holds.
    #[serde(default)]
    pub wifi_n: Option<u64>,
    /// The temperature that made the phone hot, while it is hot. Older
    /// daemons omit it.
    #[serde(default)]
    pub phone_hot_temp_c_x10: Option<i16>,
    /// The link is good for a big download right now. Older daemons omit it.
    #[serde(default)]
    pub phone_good_time: bool,
    /// The best-spot session still measures the phone. Older daemons omit it.
    #[serde(default)]
    pub best_spot_running: bool,
    /// A best-spot session shows its marks and its verdict. Older daemons
    /// omit it.
    #[serde(default)]
    pub best_spot_active: bool,
    /// The lines of the best-spot block. Older daemons omit them.
    #[serde(default)]
    pub best_spot_lines: Vec<String>,
    /// The lines of the good-hours block. Older daemons omit them.
    #[serde(default)]
    pub good_hours_lines: Vec<String>,
    /// The home screen of the window: the status card, the tiles and the hour
    /// strip. Older daemons omit it.
    #[serde(default)]
    pub home: Option<jafnify_core::tiles::HomeModel>,
}

impl Snapshot {
    /// True while the best-spot session shows its marks and its verdict.
    /// The session lives on after its two minutes: Stop has to stay
    /// available, so the marks and the verdict can be cleared.
    pub fn best_spot_showing(&self) -> bool {
        self.best_spot_active
    }
}

fn default_theme_name() -> String {
    crate::theme::ThemeMode::System.as_str().to_string()
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: String::new(),
            status_text: String::new(),
            paired_name: None,
            paired: false,
            denied: false,
            retry_reason: None,
            no_peer: false,
            connected_secs: None,
            traffic: None,
            last_connected_ago: None,
            retry_secs_left: None,
            health_line: None,
            health_detail: None,
            foreign_tunnel: false,
            code_expired: false,
            firewall: None,
            firewall_iface: None,
            dns_override: None,
            nickname: String::new(),
            path: None,
            dns_through_tunnel: false,
            peer_endpoint: None,
            last_error: None,
            role: String::new(),
            autostart: false,
            kill_switch: false,
            tray_only_when_closed: false,
            details_expanded: false,
            advanced_expanded: false,
            terms_ok: false,
            ufw_active: false,
            pair_id: None,
            peer_key_prefix: None,
            current_session: SessionStats::default(),
            session_history: Vec::new(),
            session_drops: 0,
            connection_log: Vec::new(),
            polkit_note: false,
            link_lost: false,
            no_network: false,
            theme: default_theme_name(),
            window: crate::settings::WindowGeometry::default(),
            flags: BTreeMap::new(),
            tile_scopes: BTreeMap::new(),
            home_layout: String::new(),
            phone_stats: None,
            phone_stats_age_secs: None,
            phone_stats_unavailable: false,
            usage: None,
            host_hints: Vec::new(),
            wifi_ms: None,
            wifi_loss_pct: None,
            wifi_n: None,
            phone_hot_temp_c_x10: None,
            phone_good_time: false,
            best_spot_running: false,
            best_spot_active: false,
            best_spot_lines: Vec::new(),
            good_hours_lines: Vec::new(),
            home: None,
        }
    }
}

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Try to become the one owner of `path`. `None` means another process
    /// holds it; the file lock is released automatically when that process
    /// exits, including after a crash.
    pub fn acquire(path: &Path) -> io::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Ok(Some(Self { _file: file }))
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

/// A heartbeat, not a connection count: the window is a short-lived process
/// and may poll over a new connection each time.
pub struct WindowPresence {
    last_seen: Option<Instant>,
    timeout: Duration,
}

impl WindowPresence {
    pub fn new(timeout: Duration) -> Self {
        Self {
            last_seen: None,
            timeout,
        }
    }

    pub fn seen(&mut self, now: Instant) {
        self.last_seen = Some(now);
    }

    pub fn closed(&mut self) {
        self.last_seen = None;
    }

    pub fn is_open(&self, now: Instant) -> bool {
        self.last_seen
            .is_some_and(|seen| now.saturating_duration_since(seen) < self.timeout)
    }
}

pub fn runtime_dir() -> Result<PathBuf, ControlError> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").ok_or(ControlError::NoRuntimeDir)?;
    let dir = PathBuf::from(base).join(RUNTIME_SUBDIR);
    ensure_private_dir(&dir)?;
    Ok(dir)
}

pub fn socket_path() -> Result<PathBuf, ControlError> {
    Ok(runtime_dir()?.join(SOCKET_NAME))
}

pub fn lock_path() -> Result<PathBuf, ControlError> {
    Ok(runtime_dir()?.join(LOCK_NAME))
}

pub fn window_lock_path() -> Result<PathBuf, ControlError> {
    Ok(runtime_dir()?.join(WINDOW_LOCK_NAME))
}

/// Bind the daemon socket, replacing a stale one left by a crashed process.
/// The lock is acquired before this is called, so a live daemon cannot be
/// replaced.
pub fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // Create the socket with 0600 from the first instant, not only after bind.
    let old_umask = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(path);
    unsafe {
        libc::umask(old_umask);
    }
    let listener = bound?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn connect() -> Result<UnixStream, ControlError> {
    Ok(UnixStream::connect(socket_path()?)?)
}

/// Connect with a deadline. A blocking `connect` can wait in the listen
/// backlog while the daemon is inside a password prompt; a nonblocking socket
/// plus poll keeps the window responsive.
pub fn connect_at(path: &Path) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control socket path is too long",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes.iter()) {
        *dst = *src as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let connected = unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) };
    if connected != 0 {
        let err = io::Error::last_os_error();
        let in_progress = err
            .raw_os_error()
            .is_some_and(|code| code == libc::EINPROGRESS || code == libc::EAGAIN);
        if !in_progress {
            unsafe {
                libc::close(fd);
            }
            return Err(err);
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let millis = 750_i32;
        let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
        if ready == 0 {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control connect timed out",
            ));
        }
        if ready < 0 {
            let err = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(err);
        }
        let mut so_error: libc::c_int = 0;
        let mut error_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let got = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut so_error as *mut _ as *mut libc::c_void,
                &mut error_len,
            )
        };
        if got != 0 {
            let err = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(err);
        }
        if so_error != 0 {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::from_raw_os_error(so_error));
        }
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        let err = io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(err);
    }
    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}

pub fn request(req: &Request) -> Result<Response, ControlError> {
    request_at(req, &socket_path()?)
}

/// A request whose answer may take longer than the usual 750 ms, such as the
/// one-shot link test. The caller still owns the deadline.
pub fn request_timeout(req: &Request, timeout: Duration) -> Result<Response, ControlError> {
    request_at_timeout(req, &socket_path()?, timeout)
}

pub fn request_at(req: &Request, path: &Path) -> Result<Response, ControlError> {
    request_at_timeout(req, path, Duration::from_millis(750))
}

pub fn request_at_timeout(
    req: &Request,
    path: &Path,
    read_timeout: Duration,
) -> Result<Response, ControlError> {
    let mut stream = connect_at(path)?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(Duration::from_millis(750)))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut reader = BufReader::new(stream);
    let mut answer = String::new();
    match reader.read_line(&mut answer) {
        Ok(0) => Err(ControlError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the tray process closed the control socket",
        ))),
        Ok(_) => Ok(serde_json::from_str(&answer)?),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
            Err(ControlError::Timeout)
        }
        Err(e) => Err(ControlError::Io(e)),
    }
}

/// Start the windowless daemon when no socket answers, then wait for it.
pub fn ensure_daemon() -> Result<(), ControlError> {
    if connect().is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let daemon = std::env::var_os("JAFNIFY_DAEMON_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| exe.with_file_name(crate::flavor::NAME));
    if !daemon.is_file() {
        return Err(ControlError::NoDaemon);
    }
    // A test (JAFNIFY_DAEMON_BIN) or a system without a user manager gets
    // the plain child; a desktop session gets the supervised unit.
    let supervised = std::env::var_os("JAFNIFY_DAEMON_BIN").is_none()
        && std::env::var_os("XDG_RUNTIME_DIR").is_some()
        && spawn_supervised(&daemon);
    if !supervised {
        Command::new(&daemon)
            .arg("tray")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    wait_for_socket(Duration::from_secs(5))
}

/// The transient user unit the window starts the tray in.
pub fn tray_unit_name() -> String {
    format!("{}-tray", crate::flavor::NAME)
}

/// Desktop variables the tray needs (tray icon, notifications), copied from
/// the window's environment when set.
const TRAY_ENV: [&str; 5] = [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_CURRENT_DESKTOP",
    "LANG",
];

/// `systemd-run --user` arguments for the supervised tray. A crash or kill
/// (not only a clean quit) runs `fallback` after the process is gone, so
/// with the kill switch off the machine goes back to its normal network
/// instead of staying routed into a dead tunnel; the unit then restarts the
/// tray, which adopts the tunnel again. With the kill switch on, `fallback`
/// refuses and the block stays, as designed. Same contract as the autostart
/// unit (autostart.rs).
pub fn supervised_tray_args(daemon: &Path, env: &[(String, String)]) -> Vec<String> {
    let mut args = vec![
        "--user".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--unit={}", tray_unit_name()),
        format!("--description={} tray", crate::flavor::DISPLAY),
        "-p".to_string(),
        format!("ExecStopPost={} fallback", daemon.display()),
        "-p".to_string(),
        "Restart=on-failure".to_string(),
        "-p".to_string(),
        "RestartSec=2".to_string(),
    ];
    for (name, value) in env {
        args.push(format!("--setenv={name}={value}"));
    }
    args.push("--".to_string());
    args.push(daemon.display().to_string());
    args.push("tray".to_string());
    args
}

fn spawn_supervised(daemon: &Path) -> bool {
    let env: Vec<(String, String)> = TRAY_ENV
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| (name.to_string(), v))
        })
        .collect();
    let mut cmd = Command::new("systemd-run");
    cmd.args(supervised_tray_args(daemon, &env))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if matches!(cmd.status(), Ok(status) if status.success()) {
        return true;
    }
    // The unit may already exist (restarting after a crash): never start a
    // second, unsupervised tray beside it; wait for its socket instead.
    let state = Command::new("systemctl")
        .args(["--user", "show", "-p", "ActiveState", "--value"])
        .arg(format!("{}.service", tray_unit_name()))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match state {
        Ok(out) => unit_is_running(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => false,
    }
}

/// `ActiveState` values for which the supervised tray is (re)starting.
pub fn unit_is_running(active_state: &str) -> bool {
    matches!(
        active_state.trim(),
        "active" | "activating" | "reloading" | "deactivating"
    )
}

/// Start another window; the window lock makes a second launch a no-op.
/// The caller owns the returned child and must reap it.
pub fn spawn_window() -> io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let gui = std::env::var_os("JAFNIFY_GUI_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| exe.with_file_name(crate::flavor::flavored("jafnify-gui")));
    Command::new(gui)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

pub fn wait_for_socket(timeout: Duration) -> Result<(), ControlError> {
    let path = socket_path()?;
    let deadline = Instant::now() + timeout;
    loop {
        if connect_at(&path).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ControlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
