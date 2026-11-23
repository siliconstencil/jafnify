// Android sharer runtime: mutual-code discovery + handshake + boringtun
// tunnel. Sharer mode runs the WireGuard server + router (gateway) that
// shares the phone's internet with the paired Linux peer. Status is the
// single source of truth consumed by the JNI surface (lib.rs) and rendered by
// the services on the Java side.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jafnify_core::discovery::{Discovery, DiscoveryConfig};
use jafnify_core::handshake::Handshake;
use jafnify_core::keyexchange::KeyPair;
use jafnify_core::known::{pair_record, stored_pair, KnownTrack, PairSecret};
use jafnify_core::link::RxWatch;
use jafnify_core::names::display_name;
use jafnify_core::orchestrator::{ConnectionState, Event, Orchestrator};
use jafnify_core::pairing::{CodeSession, PairingCode};
use jafnify_core::pake::PakeLink;
use jafnify_core::phone_exit::{with_summary_key, ExitReason};
use jafnify_core::phone_stats::{self, PhoneStats};
use jafnify_core::snapshot::{classify_path, CodeView, Freshness, Health, RetryView, Snapshot};
use jafnify_core::state::{Role, StateStore, TomlStateStore};
use jafnify_core::{
    ANNOUNCE_INTERVAL, CONNECT_TIMEOUT, GATEWAY_DNS_IP, LISTEN_PORT, OLD_PEER_MESSAGE,
    RX_STALL_TIMEOUT,
};
use libwgslirpy::wg::{parsebase64_32, Opts as WgOpts, PublicKey, StaticSecret, WgStats};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Retry delay after the peer said Bye: it may come straight back.
const RETRY_AFTER_BYE: Duration = Duration::from_secs(2);
/// The retry reason after the peer said Bye.
const PEER_BYE_MESSAGE: &str = "Peer disconnected";
const AUTO_ACCEPT_DELAY: Duration = Duration::from_secs(3);
const KEEPALIVE: u16 = 25;
const ZERO: Duration = Duration::ZERO;
/// Diagnostic events kept for the next `diag()` snapshot. Oldest are dropped
/// first, so the ring can never grow without bound.
const MAX_EVENTS: usize = 200;

/// Epoch milliseconds, or -1 if the system clock is before the epoch.
fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(-1)
}

/// One whitespace-free token: journal values must not split into fields.
fn token(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_whitespace() || c.is_control() || c == '=' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// One `EVENT` line: `EVENT ts=<ms> event=<name> key=value ...`.
fn event_line(name: &str, fields: &[(&str, &str)]) -> String {
    let mut line = format!("EVENT ts={} event={}", epoch_ms(), token(name));
    for (key, value) in fields {
        line.push(' ');
        line.push_str(&token(key));
        line.push('=');
        line.push_str(&token(value));
    }
    line
}

/// Append one diagnostic event to the bounded ring. Never called while the
/// state lock is held.
fn record_event(state: &Arc<Mutex<JoinerState>>, name: &str, fields: &[(&str, &str)]) {
    let line = event_line(name, fields);
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    if s.events.len() >= MAX_EVENTS {
        s.events.pop_front();
    }
    s.events.push_back(line);
}

/// Injectable time source: the loop never calls `Instant::now()` itself, so
/// tests move time by hand.
pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// A running WireGuard tunnel: its task, and a reader of the bytes received
/// from the peer (> 0 means the peer's WireGuard traffic arrived).
pub(crate) struct Tunnel {
    pub(crate) handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    pub(crate) rx: Box<dyn Fn() -> u64 + Send + Sync>,
}

pub(crate) struct TunnelRequest<'a> {
    pub(crate) keypair: &'a KeyPair,
    pub(crate) peer_pk: [u8; 32],
    /// The UDP port a sharer's gateway binds (chosen at sharer start).
    pub(crate) listen_port: u16,
    pub(crate) state: &'a Arc<Mutex<JoinerState>>,
}

/// Starts the tunnel for a paired peer. Called from inside the loop's tokio
/// runtime. Tests inject a fake.
pub(crate) trait TunnelStarter: Send + Sync {
    fn start(&self, req: TunnelRequest<'_>) -> anyhow::Result<Tunnel>;
}

#[derive(Clone)]
pub(crate) struct Env {
    pub(crate) starter: Arc<dyn TunnelStarter>,
    pub(crate) clock: Clock,
    /// UDP port the sharer's gateway listens on; probed at sharer start.
    pub(crate) listen_port: u16,
    /// Test seam: while set, the known-peer sends fail with this kind (no
    /// network). Always None in production.
    pub(crate) send_fault: Arc<Mutex<Option<std::io::ErrorKind>>>,
}

impl Env {
    fn real() -> Self {
        Self {
            starter: Arc::new(RealStarter),
            clock: Arc::new(Instant::now),
            listen_port: LISTEN_PORT,
            send_fault: Arc::default(),
        }
    }
}

struct RealStarter;

impl TunnelStarter for RealStarter {
    fn start(&self, r: TunnelRequest<'_>) -> anyhow::Result<Tunnel> {
        setup_gateway(r.keypair, r.peer_pk, r.listen_port, r.state)
    }
}

/// What a search runs on: a pairing code (0x01/0x02), or the stored pair
/// (0x03 Hello/Proof, no code).
pub(crate) enum Track {
    /// The code and the instant it was issued: the acceptance window follows
    /// the issue instant, not the moment a retry re-arms the track.
    Code(CodeSession),
    Known(Box<KnownStart>),
}

/// The known track's starting point: the stored pair, plus (when the code
/// track hands over) the track that already proved the peer and the tunnel
/// that is already up.
pub(crate) struct KnownStart {
    stored: Stored,
    /// The track that found the peer: adopted, not opened again. Its session
    /// (own nonce, the peer's nonces) is the one the peer already answered.
    track: Option<KnownTrack>,
    /// The tunnel that is already up: adopted, not started again. It runs
    /// with the stored keys, so the peer's new session meets the same peer.
    tunnel: Option<Tunnel>,
}

impl KnownStart {
    /// A known track that starts from scratch: the stored pair, a new track
    /// and a new gateway.
    fn fresh(stored: Stored) -> Self {
        Self {
            stored,
            track: None,
            tunnel: None,
        }
    }
}

/// The persisted pair: this device's private key for it and the peer's key.
/// Secret: holds a private key.
#[derive(Clone)]
pub(crate) struct Stored {
    private_key: String,
    peer_pk: [u8; 32],
}

impl Stored {
    fn peer_pk_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.peer_pk)
    }
}

/// The stored pair, if the state file holds one (v1: one known peer).
fn load_stored(path: &Path) -> Result<Option<Stored>, String> {
    let peers = TomlStateStore::new(path.to_path_buf())
        .load()
        .map_err(|e| e.to_string())?;
    let Some(p) = stored_pair(&peers) else {
        return Ok(None);
    };
    Ok(Some(Stored {
        private_key: p.private_key.clone().unwrap_or_default(),
        peer_pk: parsebase64_32(&p.public_key).map_err(|e| e.to_string())?,
    }))
}

/// Remember a code pairing (replacing any earlier one) after its first rx > 0.
fn persist_pair(
    path: &Path,
    own: &KeyPair,
    peer_pk: [u8; 32],
    peer: SocketAddr,
) -> Result<Stored, String> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let peer_b64 = base64::engine::general_purpose::STANDARD.encode(peer_pk);
    let endpoint = peer.to_string();
    let rec = pair_record(own, &peer_b64, Role::Sharer, Some(endpoint), unix)
        .map_err(|e| e.to_string())?;
    TomlStateStore::new(path.to_path_buf())
        .save(&[rec])
        .map_err(|e| e.to_string())?;
    Ok(Stored {
        private_key: own.private_key_b64(),
        peer_pk,
    })
}

/// The IPv4 addresses in a comma-separated list ("10.20.89.13,10.77.0.1"), as
/// the app passes them: the interfaces discovery runs on. Junk entries are
/// skipped.
pub fn parse_ifaces(list: &str) -> Vec<Ipv4Addr> {
    list.split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

/// Discovery on exactly the listed interfaces. An empty list keeps the old
/// behaviour (the kernel's default route): better than not searching at all.
pub fn discovery_config(ifaces: &str) -> DiscoveryConfig {
    DiscoveryConfig {
        ifaces: parse_ifaces(ifaces),
        ..Default::default()
    }
}

#[derive(Default)]
pub(crate) struct JoinerState {
    status: String,
    peer_id: String,
    peer_name: String,
    /// The peer's WireGuard endpoint; empty for the sharer, whose gateway
    /// learns the peer address dynamically.
    peer_endpoint: String,
    /// The code track changes this once its connection has stored a pair, so
    /// a later restart resumes on the known track like the running loop does.
    paired: bool,
    /// The code this run is searching with and the instant it was issued, so
    /// the JNI can count down without knowing the orchestrator. The window
    /// follows the issue instant; re-arming never extends it.
    code_session: Option<CodeSession>,
    /// The retry deadline while the orchestrator is Retry; None otherwise.
    retry_deadline: Option<Instant>,
    /// The reason of the last disconnect, for the retry line.
    retry_reason: Option<String>,
    /// A pre-v2 peer was seen during this search.
    old_peer: bool,
    /// The orchestrator state, for the health line's reachability verdict.
    orchestrator_state: Option<ConnectionState>,
    /// Live transfer counters (set once the gateway tunnel is up).
    stats: Option<Arc<Mutex<libwgslirpy::wg::WgStats>>>,
    /// Bounded diagnostic events; `diag()` drains them oldest first.
    events: VecDeque<String>,
    /// Worker liveness: iterations of the known loop, the code loop and of the
    /// tunnel supervision inside either loop. Additive diagnostics only.
    worker_known: AtomicU64,
    worker_code: AtomicU64,
    worker_tunnel: AtomicU64,
    /// Bumped when the peer announces a new session while this side is still
    /// Connected; the frontend restarts its connected clock on a change.
    session_epoch: u64,
    /// Monotonic instant this side's displayed status became Connected;
    /// None while it is not. The summary answer carries `now - this`, and the
    /// laptop counts from it. A display-only neutral clears it, so the laptop
    /// learns the restart at once.
    connected_since: Option<Instant>,
    /// Connected but no authenticated peer activity in this tick: the
    /// display is neutral. Additive to the five status lines, which stay
    /// semantic for the session tracker.
    reconnecting: bool,
}

pub struct Joiner {
    state: Arc<Mutex<JoinerState>>,
    /// The pairing code and its issue instant while running the code track;
    /// None on the known track. A restart reuses this session unchanged, so it
    /// cannot extend the code's window.
    active_code: Mutex<Option<CodeSession>>,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The running loop, joined by stop() so it cannot write stale state
    /// after the next run has started.
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The state file holding the paired device (None = nothing is remembered).
    state_path: Mutex<Option<PathBuf>>,
    env: Env,
}

impl Default for Joiner {
    fn default() -> Self {
        Self::new()
    }
}

impl Joiner {
    pub fn new() -> Self {
        Self::with_env(Env::real())
    }

    pub(crate) fn with_env(env: Env) -> Self {
        Self {
            state: Arc::new(Mutex::new(JoinerState::default())),
            active_code: Mutex::new(None),
            shutdown: Mutex::new(None),
            worker: Mutex::new(None),
            state_path: Mutex::new(None),
            env,
        }
    }

    /// Where the paired device is kept (a `TomlStateStore` file, 0600).
    pub fn set_state_path(&self, path: PathBuf) {
        *self.state_path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    }

    /// A device is paired: `sharer_start_known` can run.
    pub fn is_paired(&self) -> bool {
        matches!(self.stored(), Ok(Some(_)))
    }

    /// Short display name of the paired device, if one is stored.
    pub fn paired_name(&self) -> Option<String> {
        let path = self
            .state_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let peers = TomlStateStore::new(path).load().ok()?;
        stored_pair(&peers).map(|p| p.name.clone())
    }

    /// Forget the paired device: stop, then delete the record including the
    /// private key. Pairing by code works again afterwards.
    pub fn forget(&self) -> Result<(), String> {
        self.stop();
        let Some(path) = self
            .state_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        TomlStateStore::new(path)
            .save(&[])
            .map_err(|e| e.to_string())
    }

    fn stored(&self) -> Result<Option<Stored>, String> {
        match &*self.state_path.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(path) => load_stored(path),
            None => Ok(None),
        }
    }

    /// Starts the sharer (gateway) thread and returns the generated pairing
    /// code the Linux peer must enter. The peer's internet access is served
    /// over the WireGuard tunnel once pairing completes.
    pub fn sharer_start(&self, cfg: DiscoveryConfig) -> Result<String, String> {
        if self
            .shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Err("Already running".to_string());
        }
        let code = PairingCode::generate();
        let code_str = code.as_str().to_string();
        self.sharer_start_session(CodeSession::issue_at(code, (self.env.clock)()), cfg)?;
        Ok(code_str)
    }

    /// Sharer with a caller-chosen code and discovery config (multicast
    /// interface). Used by `sharer_start` and by the host-runnable harness.
    pub fn sharer_start_with(&self, code_str: String, cfg: DiscoveryConfig) -> Result<(), String> {
        if self
            .shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Err("Already running".to_string());
        }
        let code = PairingCode::parse(&code_str).ok_or_else(|| "Invalid code".to_string())?;
        self.sharer_start_session(CodeSession::issue_at(code, (self.env.clock)()), cfg)
    }

    /// Starts the sharer with an already-issued code session, so a restart can
    /// hand over the original issue instant instead of minting a new window.
    fn sharer_start_session(
        &self,
        session: CodeSession,
        cfg: DiscoveryConfig,
    ) -> Result<(), String> {
        if self
            .shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Err("Already running".to_string());
        }
        self.spawn(Track::Code(session), cfg, self.sharer_env())
    }

    /// Sharer for the paired device: no code, no 0x01/0x02. The gateway runs
    /// with the stored keys from the start; the peer is found by 0x03.
    pub fn sharer_start_known(&self, cfg: DiscoveryConfig) -> Result<(), String> {
        if self
            .shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Err("Already running".to_string());
        }
        let stored = self
            .stored()?
            .ok_or_else(|| "no paired device".to_string())?;
        self.spawn(
            Track::Known(Box::new(KnownStart::fresh(stored))),
            cfg,
            self.sharer_env(),
        )
    }

    /// Restart the sharer on a new interface list, staying on the current
    /// track: the same code while pairing, or the stored pair once the code
    /// track has persisted one.
    pub fn sharer_restart(&self, cfg: DiscoveryConfig) -> Result<(), String> {
        self.stop();
        let session = self
            .active_code
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let paired = self.state.lock().unwrap_or_else(|e| e.into_inner()).paired;
        if paired {
            self.sharer_start_known(cfg)
        } else if let Some(session) = session {
            // Same code, same issue instant: a restart never extends the
            // acceptance window.
            self.sharer_start_session(session, cfg)
        } else {
            Err("no sharer running".to_string())
        }
    }

    /// The sharer's environment: the gateway port is 9797 when free, else
    /// another free port (a taken port is not an error; the port is announced
    /// in the MAC-covered handshake, so the joiner follows it).
    fn sharer_env(&self) -> Env {
        let listen_port = choose_listen_port(self.env.listen_port);
        if listen_port != self.env.listen_port {
            tracing::info!(
                "UDP {} is taken: the gateway uses UDP {listen_port}",
                self.env.listen_port
            );
        }
        Env {
            listen_port,
            ..self.env.clone()
        }
    }

    fn spawn(&self, track: Track, cfg: DiscoveryConfig, env: Env) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.shutdown.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        *self.active_code.lock().unwrap_or_else(|e| e.into_inner()) = match &track {
            Track::Code(session) => Some(session.clone()),
            Track::Known(_) => None,
        };
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.paired = matches!(&track, Track::Known(_));
            // Expose the code session immediately: the first loop iteration
            // may be a moment away, and the screen asks at once.
            st.code_session = match &track {
                Track::Code(session) => Some(session.clone()),
                Track::Known(_) => None,
            };
        }
        let state = self.state.clone();
        let track_name = match &track {
            Track::Code(_) => "code",
            Track::Known(_) => "known",
        };
        record_event(
            &state,
            "session_start",
            &[("role", "sharer"), ("track", track_name)],
        );
        let store = self
            .state_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let guard_state = state.clone();
        let handle = std::thread::spawn(move || {
            guard_worker(&guard_state, || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("current-thread runtime build failed");
                rt.block_on(join_loop(track, cfg, state, rx, env, store));
            });
        });
        *self.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    pub fn stop(&self) {
        record_event(&self.state, "session_stop", &[]);
        if let Some(tx) = self
            .shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = tx.send(());
        }
        if let Some(handle) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = handle.join();
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.status.clear();
        s.peer_id.clear();
        s.peer_name.clear();
        s.peer_endpoint.clear();
        s.stats = None;
        s.code_session = None;
        s.retry_deadline = None;
        s.retry_reason = None;
        s.old_peer = false;
        s.orchestrator_state = None;
    }

    /// The code line under the pairing code: "" when no code is on screen,
    /// "Code expired" once its lifetime is over, else the remaining whole
    /// minutes ("Expires in 10 minutes"), and `m:ss` only in the last minute.
    pub fn code_expiry(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        code_expiry_line(s.code_session.as_ref(), (self.env.clock)())
    }

    /// Whether the code on screen has outlived its lifetime. The UI decides
    /// what to offer from this fact, not from the wording of the line above.
    pub fn code_expired(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.code_session
            .as_ref()
            .is_some_and(|session| session.is_expired((self.env.clock)()))
    }

    /// The health verdict under the status, or "" when nothing is known.
    pub fn health_line(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let handshake_age_ms = s
            .stats
            .as_ref()
            .map(|stats| stats.lock().map(|g| g.handshake_age_ms).unwrap_or(-1));
        health_line(
            s.old_peer,
            handshake_age_ms,
            tunnel_expected(s.orchestrator_state),
        )
    }

    /// The handshake age for the Details view, or "" when there is none.
    pub fn health_detail(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let handshake_age_ms = s
            .stats
            .as_ref()
            .map(|stats| stats.lock().map(|g| g.handshake_age_ms).unwrap_or(-1));
        health_detail(handshake_age_ms, tunnel_expected(s.orchestrator_state))
    }

    /// The retry line while the orchestrator is retrying, else "".
    pub fn retry_line(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        retry_line(
            s.retry_deadline,
            s.retry_reason.as_deref(),
            (self.env.clock)(),
        )
    }

    /// Five lines: status, peer id, peer name, rx bytes, tx bytes.
    /// Peer fields are empty until paired; counters are zero until the
    /// gateway tunnel is up. The host CLI keeps this shape.
    pub fn status(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (rx, tx) = counters(&s);
        format!(
            "{}\n{}\n{}\n{}\n{}",
            s.status, s.peer_id, s.peer_name, rx, tx
        )
    }

    /// The sharer's connected time in milliseconds, or None while it is not
    /// Connected. The laptop adopts this value.
    pub fn connected_ms(&self) -> Option<u64> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.connected_since
            .map(|since| Instant::now().duration_since(since).as_millis() as u64)
    }

    /// Connected but no authenticated peer activity: the display is neutral.
    pub fn reconnecting(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.reconnecting
    }

    /// Nine lines for the Android JNI: the five of [`status`](Self::status),
    /// the peer endpoint (empty for a sharer), the session epoch, the
    /// connected milliseconds (0 when not connected) and the neutral flag
    /// (1 while no authenticated peer activity arrives). Android reads the
    /// first seven; the clock and flag are additive.
    pub fn status_with_endpoint(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (rx, tx) = counters(&s);
        let connected_ms = s
            .connected_since
            .map(|since| Instant::now().duration_since(since).as_millis() as u64)
            .unwrap_or(0);
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            s.status,
            s.peer_id,
            s.peer_name,
            rx,
            tx,
            live_endpoint(&s),
            s.session_epoch,
            connected_ms,
            u8::from(s.reconnecting)
        )
    }

    /// Additive diagnostic snapshot: a `STATS` first line with the common
    /// counters/addresses, followed by the drained `EVENT` lines. Every value
    /// is one whitespace-free token; no code, key or packet payload is ever
    /// included. `status()` and `status_with_endpoint()` are unaffected.
    pub fn diag(&self) -> String {
        let (stats, peer_endpoint, events, workers) = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let stats = s.stats.clone();
            let peer_endpoint = s.peer_endpoint.clone();
            let events: Vec<String> = s.events.drain(..).collect();
            let workers = (
                s.worker_known.load(Ordering::Relaxed),
                s.worker_code.load(Ordering::Relaxed),
                s.worker_tunnel.load(Ordering::Relaxed),
            );
            (stats, peer_endpoint, events, workers)
        };
        let unavailable = || {
            (
                0,
                0,
                0,
                0,
                0,
                0,
                -1,
                -1,
                -1,
                String::new(),
                String::new(),
                -1,
                0,
                0,
            )
        };
        let (
            udp_rx_packets,
            udp_rx_bytes,
            udp_tx_packets,
            udp_tx_bytes,
            udp_send_errors,
            udp_recv_errors,
            udp_last_errno,
            last_rx_ms,
            last_tx_ms,
            peer_addr,
            local_addr,
            wg_handshake_ms,
            wg_rx_bytes,
            wg_tx_bytes,
        ) = match &stats {
            Some(stats) => match stats.lock() {
                Ok(g) => (
                    g.udp_rx_packets,
                    g.udp_rx_bytes,
                    g.udp_tx_packets,
                    g.udp_tx_bytes,
                    g.send_errors,
                    g.recv_errors,
                    g.last_errno as i64,
                    g.last_rx_ms,
                    g.last_tx_ms,
                    g.peer_addr.clone(),
                    g.local_addr.clone(),
                    g.handshake_age_ms,
                    g.rx_bytes,
                    g.tx_bytes,
                ),
                Err(_) => unavailable(),
            },
            None => unavailable(),
        };
        let peer = if !peer_addr.is_empty() {
            peer_addr
        } else if !peer_endpoint.is_empty() {
            peer_endpoint
        } else {
            "-1".to_string()
        };
        let local = if local_addr.is_empty() {
            "-1".to_string()
        } else {
            local_addr
        };
        let (worker_known, worker_code, worker_tunnel) = workers;
        let fields: [(&str, String); 17] = [
            ("udp_rx_packets", udp_rx_packets.to_string()),
            ("udp_rx_bytes", udp_rx_bytes.to_string()),
            ("udp_tx_packets", udp_tx_packets.to_string()),
            ("udp_tx_bytes", udp_tx_bytes.to_string()),
            ("udp_send_errors", udp_send_errors.to_string()),
            ("udp_recv_errors", udp_recv_errors.to_string()),
            ("udp_last_errno", udp_last_errno.to_string()),
            ("last_rx_ms", last_rx_ms.to_string()),
            ("last_tx_ms", last_tx_ms.to_string()),
            ("peer", peer),
            ("local", local),
            ("wg_handshake_ms", wg_handshake_ms.to_string()),
            ("wg_rx_bytes", wg_rx_bytes.to_string()),
            ("wg_tx_bytes", wg_tx_bytes.to_string()),
            ("worker_known", worker_known.to_string()),
            ("worker_code", worker_code.to_string()),
            ("worker_tunnel", worker_tunnel.to_string()),
        ];
        let mut out = String::from("STATS");
        for (key, value) in fields {
            out.push(' ');
            out.push_str(key);
            out.push('=');
            out.push_str(&token(&value));
        }
        for event in events {
            out.push('\n');
            out.push_str(&event);
        }
        out
    }
}

/// The live rx/tx counters, or zeros while no tunnel is up.
/// The peer's current socket address: the gateway learns it from the first
/// authenticated packet (WgStats::peer_addr), so a sharer has no fixed
/// endpoint. A stored endpoint (joiner side) is the fallback.
fn live_endpoint(s: &JoinerState) -> String {
    if let Some(stats) = &s.stats {
        let g = stats.lock().unwrap_or_else(|e| e.into_inner());
        if !g.peer_addr.is_empty() {
            return g.peer_addr.clone();
        }
    }
    s.peer_endpoint.clone()
}

fn counters(s: &JoinerState) -> (u64, u64) {
    match &s.stats {
        Some(stats) => {
            let g = stats.lock().unwrap_or_else(|e| e.into_inner());
            (g.rx_bytes, g.tx_bytes)
        }
        None => (0, 0),
    }
}

/// A code pairing that has been persisted hands over to the known track at
/// its next retry (the pair's key is then the one to reconnect with); a
/// search that starts on the known track stays on it.
async fn join_loop(
    track: Track,
    cfg: DiscoveryConfig,
    state: Arc<Mutex<JoinerState>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    env: Env,
    store: Option<PathBuf>,
) {
    let mut track = track;
    loop {
        match track {
            Track::Code(session) => {
                let args = (cfg.clone(), state.clone(), env.clone());
                match code_loop(session, args, &mut shutdown_rx, store.as_deref()).await {
                    Some(start) => track = Track::Known(Box::new(start)),
                    None => return,
                }
            }
            Track::Known(start) => {
                known_loop(*start, &cfg, &state, &mut shutdown_rx, &env).await;
                return;
            }
        }
    }
}

/// The pairing track. Returns the known track's starting point when a retry
/// is due after a pairing was stored, None when shut down.
async fn code_loop(
    session: CodeSession,
    (cfg, state, env): (DiscoveryConfig, Arc<Mutex<JoinerState>>, Env),
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
    store: Option<&Path>,
) -> Option<KnownStart> {
    let code = session.code().clone();
    let issued_at = session.issued_at();
    let mut persisted: Option<Stored> = None;
    // The watch for the paired device's known-peer session. It answers that
    // device's Hellos at once, so a device that came back (a restart) finds
    // this side without waiting for a retry.
    let mut watcher: Option<KnownTrack> = None;
    let mut link = match Link::open(&cfg) {
        Ok(l) => l,
        Err(e) => {
            record_event(&state, "socket_error", &[("phase", "open"), ("reason", &e)]);
            tracing::error!("{}", e);
            return None;
        }
    };
    let mut orch = Orchestrator::new();
    orch.start_search_at(code.clone(), issued_at);
    let mut s = Session::default();
    // Last endpoint announced by the peer, for endpoint-change events.
    let mut last_peer_addr: Option<SocketAddr> = None;
    // The PAKE session of this attempt (0x05/0x06). Created when the attempt
    // reaches Connecting: a retry makes a new keypair and a new SPAKE2 state.
    let mut pake: Option<PakeLink> = None;

    loop {
        {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            st.worker_code.fetch_add(1, Ordering::Relaxed);
            if s.tunnel.is_some() {
                st.worker_tunnel.fetch_add(1, Ordering::Relaxed);
            }
        }
        if shutdown_rx.try_recv().is_ok() {
            if let Some(t) = s.tunnel.take() {
                t.handle.abort();
            }
            return None;
        }
        let now = (env.clock)();
        let was_retry = orch.state() == ConnectionState::Retry;
        if let Some(ev) = orch.tick(now) {
            match ev {
                Event::NoPeerFound => tracing::debug!("search window elapsed, still searching"),
            }
        }
        if was_retry && orch.state() == ConnectionState::Searching && persisted.is_some() {
            return persisted.map(KnownStart::fresh);
        }
        if was_retry && orch.state() == ConnectionState::Searching {
            // The retry wait is over: search from scratch. New sockets (no
            // frame queued from the old peer can be taken as live), new
            // keypair and a new discovery nonce, exactly like a first start.
            match Link::open(&cfg) {
                Ok(l) => {
                    record_event(&state, "socket_rebind", &[("phase", "search")]);
                    link = l;
                    pake = None;
                    // Same code, same issue instant: the retry does not extend
                    // the acceptance window.
                    orch.start_search_at(code.clone(), issued_at);
                    s = Session::default();
                }
                Err(e) => {
                    record_event(
                        &state,
                        "socket_error",
                        &[("phase", "rebind"), ("reason", &e)],
                    );
                    record_event(&state, "reset", &[("reason", &e)]);
                    orch.on_disconnect_at(e, now);
                }
            }
        }
        // Both ends announce while Searching and Pairing so the peer can
        // find us whichever app started first.
        if matches!(
            orch.state(),
            ConnectionState::Searching | ConnectionState::Pairing
        ) {
            announce_discovery(&link.discovery, &orch, &mut s.last_discovery_announce, now);
        }
        // A version-1 handshake from the peer: it can never pair with this build.
        if (link.handshake.old_peer_seen(&code) || pake.as_ref().is_some_and(|p| p.old_peer_seen()))
            && matches!(
                orch.state(),
                ConnectionState::Searching | ConnectionState::Pairing | ConnectionState::Connecting
            )
        {
            lose(&state, &mut orch, &mut s, OLD_PEER_MESSAGE.to_string(), now);
        }
        match orch.state() {
            ConnectionState::Searching => {
                if let Ok(Some((from, token))) = link.discovery.try_receive(ZERO) {
                    if orch.on_discovery_event(&token) {
                        link.discovery.learn(from);
                        s.auto_accept_deadline = Some(now + AUTO_ACCEPT_DELAY);
                    }
                }
            }
            ConnectionState::Pairing => {
                if let Some(deadline) = s.auto_accept_deadline {
                    if now >= deadline {
                        s.auto_accept_deadline = None;
                        if orch.try_accept_code(code.as_str()).is_err() {
                            tracing::error!("auto-accept failed for own code");
                        }
                    }
                }
            }
            ConnectionState::Connecting => {
                // Connecting lasts until the peer's first WireGuard data
                // arrives, not until its 0x02 frame does. Until then our own
                // 0x02 keeps going out: a single frame can be lost.
                let since = *s.connecting_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= CONNECT_TIMEOUT {
                    let reason =
                        format!("No data from peer within {} s", CONNECT_TIMEOUT.as_secs());
                    record_event(
                        &state,
                        "no_data",
                        &[("phase", "connecting"), ("reason", &reason)],
                    );
                    lose(&state, &mut orch, &mut s, reason, now);
                } else {
                    // The PAKE session of this attempt: a new keypair, a new
                    // nonce and a new SPAKE2 state for every attempt.
                    if pake.is_none() {
                        match PakeLink::open(
                            &cfg,
                            Role::Sharer,
                            &code,
                            &link.keypair,
                            env.listen_port,
                        ) {
                            Ok(p) => pake = Some(p),
                            Err(e) => {
                                record_event(
                                    &state,
                                    "socket_error",
                                    &[("phase", "pake"), ("reason", &e.to_string())],
                                );
                                lose(&state, &mut orch, &mut s, format!("Pake error: {e}"), now);
                                continue;
                            }
                        }
                    }
                    let poll = match pake.as_mut() {
                        Some(p) => match p.poll(now) {
                            Ok(poll) => poll,
                            Err(e) => {
                                lose(&state, &mut orch, &mut s, format!("Pake error: {e}"), now);
                                continue;
                            }
                        },
                        None => continue,
                    };
                    if poll.wrong_code && !s.wrong_code_recorded {
                        // A wrong code is a diagnostic only: it never stops the
                        // attempt, which ends at the connect timeout.
                        s.wrong_code_recorded = true;
                        record_event(&state, "wrong_code", &[("phase", "connecting")]);
                    }
                    let probe = s
                        .tunnel
                        .as_ref()
                        .map(|t| (t.handle.is_finished(), (t.rx)()));
                    if let Some((false, rx)) = probe {
                        s.note_rx(now, rx);
                    }
                    match probe {
                        None => {
                            if let Some(peer) = poll.peer {
                                let peer_addr = peer.endpoint;
                                if last_peer_addr != Some(peer_addr) {
                                    record_event(
                                        &state,
                                        "peer_endpoint",
                                        &[("peer", &peer_addr.to_string())],
                                    );
                                    last_peer_addr = Some(peer_addr);
                                }
                                let started = env.starter.start(TunnelRequest {
                                    keypair: &link.keypair,
                                    peer_pk: peer.public_key,
                                    listen_port: env.listen_port,
                                    state: &state,
                                });
                                match started {
                                    Ok(t) => {
                                        record_event(
                                            &state,
                                            "tunnel_start",
                                            &[("peer", &peer_addr.to_string())],
                                        );
                                        s.tunnel = Some(t);
                                        s.peer = Some((peer.public_key, peer_addr));
                                        // Fresh window for the WG handshake.
                                        s.connecting_since = None;
                                    }
                                    Err(e) => {
                                        record_event(
                                            &state,
                                            "tunnel_error",
                                            &[("reason", &e.to_string())],
                                        );
                                        tracing::error!("tunnel setup failed: {}", e);
                                        lose(
                                            &state,
                                            &mut orch,
                                            &mut s,
                                            format!("Tunnel error: {e}"),
                                            now,
                                        );
                                    }
                                }
                            }
                        }
                        Some((true, _)) => lose(
                            &state,
                            &mut orch,
                            &mut s,
                            "Connection lost".to_string(),
                            now,
                        ),
                        Some((false, rx)) if rx > 0 => {
                            orch.confirm_connected();
                            s.connecting_since = None;
                            s.rx_watch = None;
                            // Traffic proves the tunnel: only now remember the pair.
                            if let (Some(path), Some((pk, peer))) = (store, s.peer) {
                                match persist_pair(path, &link.keypair, pk, peer) {
                                    Ok(stored) => {
                                        state.lock().unwrap_or_else(|e| e.into_inner()).paired =
                                            true;
                                        persisted = Some(stored.clone());
                                        // From now on the paired device can come
                                        // back on the known track (a restart);
                                        // this side answers it at once.
                                        watcher = open_watcher(&stored, &cfg, &env, now);
                                    }
                                    Err(e) => tracing::error!("could not save the pairing: {}", e),
                                }
                            }
                        }
                        Some(_) => {}
                    }
                }
            }
            ConnectionState::Connected => {
                let probe = s
                    .tunnel
                    .as_ref()
                    .map(|t| (t.handle.is_finished(), (t.rx)()));
                if let Some((false, rx)) = probe {
                    s.note_rx(now, rx);
                }
                match probe {
                    None | Some((true, _)) => lose(
                        &state,
                        &mut orch,
                        &mut s,
                        "Connection lost".to_string(),
                        now,
                    ),
                    Some((false, rx)) => {
                        let watch = s
                            .rx_watch
                            .get_or_insert_with(|| RxWatch::new(now, Some(rx)));
                        if watch.stalled(now, Some(rx)) {
                            let reason =
                                format!("No data from peer for {} s", RX_STALL_TIMEOUT.as_secs());
                            record_event(
                                &state,
                                "no_data",
                                &[("phase", "connected"), ("reason", &reason)],
                            );
                            lose(&state, &mut orch, &mut s, reason, now);
                        }
                    }
                }
            }
            _ => {}
        }
        // The paired device may come back on the known track at any time (a
        // restart while this side holds the code session). Its Proof proves
        // it holds the paired key and is live now, so its new session
        // supersedes the stale one at once.
        if orch.state() == ConnectionState::Connected {
            if let Some(start) = take_over(&mut watcher, &state, &mut s, &persisted, now) {
                return Some(start);
            }
        }
        record_view(&state, &orch, now, s.fresh(now));
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The watch for the paired device's known-peer session: it answers that
/// device's Hellos at once, so a device that came back (a restart) finds this
/// side without waiting for a retry.
fn open_watcher(
    stored: &Stored,
    cfg: &DiscoveryConfig,
    env: &Env,
    now: Instant,
) -> Option<KnownTrack> {
    match open_known(stored, cfg, now) {
        Ok((mut track, _keypair)) => {
            track.set_own_port(env.listen_port);
            track.watch_new_session();
            // This side keeps its WireGuard state: the peer must not set up
            // its tunnel again because of a Fresh from here.
            track.suppress_fresh();
            Some(track)
        }
        Err(e) => {
            tracing::error!("could not watch for the paired device: {}", e);
            None
        }
    }
}

/// Take the paired device's new session over when it proved itself: the
/// tunnel and the track that found it are kept, so nothing is torn down.
/// Returns the known track's starting point, or None while the device has not
/// proved itself.
fn take_over(
    watcher: &mut Option<KnownTrack>,
    state: &Arc<Mutex<JoinerState>>,
    s: &mut Session,
    persisted: &Option<Stored>,
    now: Instant,
) -> Option<KnownStart> {
    let poll = {
        let watcher = watcher.as_mut()?;
        match watcher.poll_detailed(now) {
            Ok(poll) => poll,
            Err(e) => {
                record_event(
                    state,
                    "socket_error",
                    &[("phase", "watch"), ("reason", &e.to_string())],
                );
                return None;
            }
        }
    };
    if poll.endpoint.is_some() {
        s.note_peer(now);
    }
    if !poll.peer_new_session {
        return None;
    }
    record_event(
        state,
        "session_takeover",
        &[("reason", "new session of the paired device")],
    );
    // The peer proved itself on a new session: its clock starts again.
    state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .session_epoch += 1;
    Some(KnownStart {
        stored: persisted.clone()?,
        track: watcher.take(),
        tunnel: s.tunnel.take(),
    })
}

/// The known track: the stored pair reconnects with no code. The sharer serves
/// from the start of every search (its gateway uses the stored keys).
/// The connect timeout, stall and retry rules are the code track's.
async fn known_loop(
    start: KnownStart,
    cfg: &DiscoveryConfig,
    state: &Arc<Mutex<JoinerState>>,
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
    env: &Env,
) {
    let stored = start.stored;
    let mut orch = Orchestrator::new();
    orch.start_known_search();
    let mut keypair = match stored_keypair(&stored) {
        Ok(k) => k,
        Err(e) => {
            record_event(state, "socket_error", &[("phase", "open"), ("reason", &e)]);
            tracing::error!("{}", e);
            return;
        }
    };
    // The track that found the peer is adopted when the code track hands
    // over: its session is the one the peer already answered.
    let mut track = match start.track {
        Some(t) => t,
        None => match KnownTrack::open(
            cfg,
            &keypair,
            &stored.peer_pk_b64(),
            Role::Sharer,
            (env.clock)(),
        ) {
            Ok(mut t) => {
                t.set_own_port(env.listen_port);
                t
            }
            Err(e) => {
                record_event(
                    state,
                    "socket_error",
                    &[("phase", "open"), ("reason", &e.to_string())],
                );
                tracing::error!("{}", e);
                return;
            }
        },
    };
    // The tunnel that is already up is adopted: the peer's new session meets
    // the same gateway, with the stored keys.
    let mut s = Session {
        tunnel: start.tunnel,
        ..Session::default()
    };
    // Last endpoint learned from the tracked peer, for endpoint-change events.
    let mut last_peer_addr: Option<SocketAddr> = None;
    // Since when the sends fail because this phone has no usable network.
    let mut no_network_since: Option<Instant> = None;
    if s.tunnel.is_some() {
        // The session continues: the peer proved itself and the tunnel never
        // went down.
        orch.confirm_connected();
    } else {
        start_known_gateway(
            env,
            &keypair,
            &stored,
            state,
            &mut orch,
            &mut s,
            (env.clock)(),
        );
    }

    loop {
        {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            st.worker_known.fetch_add(1, Ordering::Relaxed);
            if s.tunnel.is_some() {
                st.worker_tunnel.fetch_add(1, Ordering::Relaxed);
            }
        }
        if shutdown_rx.try_recv().is_ok() {
            // Stopping: tell a connected peer, so it ends its session now.
            if matches!(
                orch.state(),
                ConnectionState::Connected | ConnectionState::Connecting
            ) {
                match track.send_bye() {
                    Ok(()) => record_event(state, "bye_sent", &[]),
                    Err(e) => record_event(
                        state,
                        "socket_error",
                        &[("phase", "bye"), ("reason", &e.to_string())],
                    ),
                }
            }
            if let Some(t) = s.tunnel.take() {
                t.handle.abort();
            }
            return;
        }
        let now = (env.clock)();
        let was_retry = orch.state() == ConnectionState::Retry;
        if let Some(ev) = orch.tick(now) {
            match ev {
                Event::NoPeerFound => tracing::debug!("search window elapsed, still searching"),
            }
        }
        if was_retry && orch.state() == ConnectionState::Searching {
            // Fresh sockets and nonce, like the code track; the key is the
            // stored one, so the sharer's gateway restarts with it.
            match open_known(&stored, cfg, now) {
                Ok((mut t, k)) => {
                    record_event(state, "socket_rebind", &[("phase", "known")]);
                    t.set_own_port(env.listen_port);
                    track = t;
                    keypair = k;
                    s = Session::default();
                    start_known_gateway(env, &keypair, &stored, state, &mut orch, &mut s, now);
                }
                Err(e) => {
                    record_event(
                        state,
                        "socket_error",
                        &[("phase", "rebind"), ("reason", &e)],
                    );
                    record_event(state, "reset", &[("reason", &e)]);
                    orch.on_disconnect_at(e, now);
                }
            }
        }
        if track.old_peer_seen()
            && matches!(
                orch.state(),
                ConnectionState::Searching | ConnectionState::Connecting
            )
        {
            lose(state, &mut orch, &mut s, OLD_PEER_MESSAGE.to_string(), now);
        }
        if matches!(
            orch.state(),
            ConnectionState::Searching | ConnectionState::Connecting | ConnectionState::Connected
        ) {
            track.set_connected(orch.state() == ConnectionState::Connected);
            track.fail_sends(*env.send_fault.lock().unwrap_or_else(|e| e.into_inner()));
            let polled = track.poll_detailed(now);
            if let Ok(p) = &polled {
                if p.endpoint.is_some() {
                    s.note_peer(now);
                }
                if orch.state() == ConnectionState::Connected {
                    if p.peer_bye {
                        // The peer stopped: end the session now, retry soon.
                        record_event(state, "peer_bye", &[]);
                        lose_with_retry(
                            state,
                            &mut orch,
                            &mut s,
                            PEER_BYE_MESSAGE.to_string(),
                            now,
                            RETRY_AFTER_BYE,
                        );
                    } else if p.peer_restarted {
                        // The peer started a new session while data kept
                        // flowing: the frontend restarts its clock.
                        record_event(state, "peer_restart", &[]);
                        state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .session_epoch += 1;
                    }
                }
            }
            match polled.map(|p| p.endpoint) {
                Ok(Some(from)) if orch.state() == ConnectionState::Searching => {
                    if last_peer_addr != Some(from) {
                        record_event(state, "peer_endpoint", &[("peer", &from.to_string())]);
                        last_peer_addr = Some(from);
                    }
                    if s.tunnel.is_none() {
                        // The gateway (re)starts when the stored peer is found
                        // and no tunnel is up.
                        let started = env.starter.start(TunnelRequest {
                            keypair: &keypair,
                            peer_pk: stored.peer_pk,
                            listen_port: env.listen_port,
                            state,
                        });
                        match started {
                            Ok(t) => {
                                record_event(state, "tunnel_start", &[("peer", &from.to_string())]);
                                s.tunnel = Some(t);
                            }
                            Err(e) => {
                                record_event(state, "tunnel_error", &[("reason", &e.to_string())]);
                                tracing::error!("tunnel setup failed: {}", e);
                                lose(state, &mut orch, &mut s, format!("Tunnel error: {e}"), now);
                            }
                        }
                    }
                    if orch.state() == ConnectionState::Searching {
                        orch.known_peer_found();
                        s.connecting_since = None;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    record_event(
                        state,
                        "socket_error",
                        &[("phase", "poll"), ("reason", &e.to_string())],
                    );
                    lose(state, &mut orch, &mut s, e.to_string(), now)
                }
            }
            match track.no_network().map(str::to_string) {
                // A live session with no network is lost, as before.
                Some(text)
                    if matches!(
                        orch.state(),
                        ConnectionState::Connecting | ConnectionState::Connected
                    ) =>
                {
                    let reason = format!("known-peer socket error: {text}");
                    record_event(
                        state,
                        "socket_error",
                        &[("phase", "poll"), ("reason", &reason)],
                    );
                    lose(state, &mut orch, &mut s, reason, now);
                }
                // Searching: wait for the network quietly, one event, instead
                // of a Retry and a rebind every few seconds.
                Some(text) if orch.state() == ConnectionState::Searching => {
                    if no_network_since.is_none() {
                        no_network_since = Some(now);
                        record_event(state, "no_network", &[("reason", &text)]);
                    }
                }
                Some(_) => {}
                None => {
                    if let Some(since) = no_network_since.take() {
                        let secs = now.saturating_duration_since(since).as_secs().to_string();
                        record_event(state, "network_back", &[("after_s", &secs)]);
                    }
                }
            }
        }
        let probe = s
            .tunnel
            .as_ref()
            .map(|t| (t.handle.is_finished(), (t.rx)()));
        if let Some((false, rx)) = probe {
            s.note_rx(now, rx);
        }
        match (orch.state(), probe) {
            (ConnectionState::Searching, Some((true, _))) => {
                lose(state, &mut orch, &mut s, "Connection lost".to_string(), now)
            }
            // WireGuard data from the stored peer needs no Proof.
            (ConnectionState::Searching, Some((false, rx))) if rx > 0 => {
                orch.confirm_connected();
                s.rx_watch = None;
            }
            (ConnectionState::Connecting, probe) => {
                let since = *s.connecting_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= CONNECT_TIMEOUT {
                    let reason =
                        format!("No data from peer within {} s", CONNECT_TIMEOUT.as_secs());
                    record_event(
                        state,
                        "no_data",
                        &[("phase", "connecting"), ("reason", &reason)],
                    );
                    lose(state, &mut orch, &mut s, reason, now);
                } else {
                    match probe {
                        None | Some((true, _)) => {
                            lose(state, &mut orch, &mut s, "Connection lost".to_string(), now)
                        }
                        Some((false, rx)) if rx > 0 => {
                            orch.confirm_connected();
                            s.connecting_since = None;
                            s.rx_watch = None;
                        }
                        Some(_) => {}
                    }
                }
            }
            (ConnectionState::Connected, probe) => match probe {
                None | Some((true, _)) => {
                    lose(state, &mut orch, &mut s, "Connection lost".to_string(), now)
                }
                Some((false, rx)) => {
                    let watch = s
                        .rx_watch
                        .get_or_insert_with(|| RxWatch::new(now, Some(rx)));
                    if watch.stalled(now, Some(rx)) {
                        let reason =
                            format!("No data from peer for {} s", RX_STALL_TIMEOUT.as_secs());
                        record_event(
                            state,
                            "no_data",
                            &[("phase", "connected"), ("reason", &reason)],
                        );
                        lose(state, &mut orch, &mut s, reason, now);
                    }
                }
            },
            _ => {}
        }
        record_view(state, &orch, now, s.fresh(now));
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Sockets and key of one known search.
fn open_known(
    stored: &Stored,
    cfg: &DiscoveryConfig,
    now: Instant,
) -> Result<(KnownTrack, KeyPair), String> {
    let keypair = stored_keypair(stored)?;
    let track = KnownTrack::open(cfg, &keypair, &stored.peer_pk_b64(), Role::Sharer, now)
        .map_err(|e| e.to_string())?;
    Ok((track, keypair))
}

/// The stored pair's own key: the one the tunnel and the known session run on.
fn stored_keypair(stored: &Stored) -> Result<KeyPair, String> {
    KeyPair::from_private_b64(&stored.private_key).map_err(|e| format!("stored key unusable: {e}"))
}

/// A sharer serves from the start of every known search; its endpoint is
/// learned from the first authenticated WireGuard packet.
#[allow(clippy::too_many_arguments)]
fn start_known_gateway(
    env: &Env,
    keypair: &KeyPair,
    stored: &Stored,
    state: &Arc<Mutex<JoinerState>>,
    orch: &mut Orchestrator,
    s: &mut Session,
    now: Instant,
) {
    let started = env.starter.start(TunnelRequest {
        keypair,
        peer_pk: stored.peer_pk,
        listen_port: env.listen_port,
        state,
    });
    match started {
        Ok(t) => {
            record_event(state, "tunnel_start", &[("role", "sharer")]);
            s.tunnel = Some(t);
        }
        Err(e) => {
            record_event(state, "tunnel_error", &[("reason", &e.to_string())]);
            tracing::error!("gateway setup failed: {}", e);
            lose(state, orch, s, format!("Tunnel error: {e}"), now);
        }
    }
}

/// Everything that belongs to ONE search: sockets and this device's keypair.
struct Link {
    discovery: Discovery,
    handshake: Handshake,
    keypair: KeyPair,
}

impl Link {
    fn open(cfg: &DiscoveryConfig) -> Result<Self, String> {
        Ok(Self {
            discovery: Discovery::bind(cfg).map_err(|e| format!("discovery bind failed: {e}"))?,
            handshake: Handshake::bind(cfg).map_err(|e| format!("handshake bind failed: {e}"))?,
            keypair: KeyPair::generate(),
        })
    }
}

/// Timers and the tunnel of the current search; reset whenever a search
/// starts over.
#[derive(Default)]
struct Session {
    auto_accept_deadline: Option<Instant>,
    last_discovery_announce: Option<Instant>,
    /// A wrong-code Confirm was already recorded for this attempt.
    wrong_code_recorded: bool,
    tunnel: Option<Tunnel>,
    connecting_since: Option<Instant>,
    rx_watch: Option<RxWatch>,
    /// The peer's key and address once a tunnel was started toward it.
    peer: Option<([u8; 32], SocketAddr)>,
    /// The last authenticated frame from the peer (a verified Proof) and the
    /// last tunnel rx value with the instant it changed.
    last_peer: Option<Instant>,
    last_rx: Option<(Instant, u64)>,
}

impl Session {
    /// The peer proved itself just now (a verified Proof or Hello).
    fn note_peer(&mut self, now: Instant) {
        self.last_peer = Some(now);
    }

    /// A fresh tunnel rx value; only a change counts as data.
    fn note_rx(&mut self, now: Instant, rx: u64) {
        match self.last_rx {
            Some((_, last)) if last == rx => {}
            _ => self.last_rx = Some((now, rx)),
        }
    }

    /// Whether authenticated peer activity arrived within [`FRESH_GAP`]. A
    /// track that never saw a verified peer frame (the code track) is never
    /// declared stale here; the 35 s stall covers it.
    fn fresh(&self, now: Instant) -> bool {
        let Some(last_peer) = self.last_peer else {
            return true;
        };
        now.saturating_duration_since(last_peer) <= FRESH_GAP
            || self
                .last_rx
                .is_some_and(|(at, _)| now.saturating_duration_since(at) <= FRESH_GAP)
    }
}

/// The tunnel is dead or never got data: stop it and hand over to the
/// orchestrator (Retry, 10 s, fresh search). The reset (and its reason) is
/// also recorded for the diagnostic snapshot; behavior is unchanged.
/// Like `lose`, with an explicit retry delay.
fn lose_with_retry(
    state: &Arc<Mutex<JoinerState>>,
    orch: &mut Orchestrator,
    s: &mut Session,
    reason: String,
    now: Instant,
    retry: Duration,
) {
    record_event(state, "reset", &[("reason", &reason)]);
    if let Some(t) = s.tunnel.take() {
        t.handle.abort();
    }
    *s = Session::default();
    orch.on_disconnect_at_with_retry(reason, now, retry);
}

fn lose(
    state: &Arc<Mutex<JoinerState>>,
    orch: &mut Orchestrator,
    s: &mut Session,
    reason: String,
    now: Instant,
) {
    record_event(state, "reset", &[("reason", &reason)]);
    if let Some(t) = s.tunnel.take() {
        t.handle.abort();
    }
    *s = Session::default();
    orch.on_disconnect_at(reason, now);
}

/// Announce our discovery token (frame 0x01) once per ANNOUNCE_INTERVAL.
fn announce_discovery(
    discovery: &Discovery,
    orch: &Orchestrator,
    last: &mut Option<Instant>,
    now: Instant,
) {
    if last.is_some_and(|t| now.duration_since(t) < ANNOUNCE_INTERVAL) {
        return;
    }
    if let Some(token) = orch.own_token() {
        if discovery.announce(&token).is_err() {
            tracing::error!("discovery announce failed");
        }
        *last = Some(now);
    }
}

/// While Connected, no authenticated frame from the peer for this long means
/// the tunnel is not carrying anything: the display goes neutral without
/// touching the orchestrator state (the 35 s stall, drops and away rules stay
/// as they are).
pub(crate) const FRESH_GAP: Duration = Duration::from_secs(12);

/// The clock start the display implies: entering Connected stamps now,
/// staying Connected keeps the stamp, leaving it (a real loss or a neutral
/// gap) clears the stamp. The next Connected tick starts a new count.
fn next_connected_since(
    current: Option<Instant>,
    display_connected: bool,
    now: Instant,
) -> Option<Instant> {
    if !display_connected {
        None
    } else {
        Some(current.unwrap_or(now))
    }
}

/// Copy the orchestrator facts the JNI accessors render. Called once per loop
/// tick; the accessors never need the loop's own `Orchestrator`.
fn record_view(state: &Arc<Mutex<JoinerState>>, orch: &Orchestrator, now: Instant, fresh: bool) {
    let connected = orch.state() == ConnectionState::Connected;
    let display_connected = connected && fresh;
    let since = {
        let st = state.lock().unwrap_or_else(|e| e.into_inner());
        next_connected_since(st.connected_since, display_connected, now)
    };
    phone_stats_provider().set_connected_since(since);
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    // Line 0 keeps its orchestrator meaning so the phone's session tracker
    // never reads a display-only neutral as a drop; the flag rides beside it.
    st.status = status_text(orch);
    st.reconnecting = connected && !fresh;
    st.connected_since = since;
    st.code_session = orch.code_session().cloned();
    st.retry_deadline = orch.retry_remaining_at(now).map(|d| now + d);
    st.retry_reason = orch.last_disconnect_reason().map(str::to_string);
    st.old_peer = orch.old_peer_seen();
    st.orchestrator_state = Some(orch.state());
}

/// The state a dead worker leaves behind: the card says sharing stopped so the
/// user starts again, and no stale "Connected" survives. A worker panic must
/// not leave the phone looking like it still shares.
fn record_worker_panic(state: &Arc<Mutex<JoinerState>>) {
    record_event(state, "worker_panic", &[]);
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.status = jafnify_core::share_notice::WORKER_STOPPED.to_string();
    st.peer_id.clear();
    st.peer_name.clear();
    st.peer_endpoint.clear();
    st.stats = None;
    st.retry_deadline = None;
    st.retry_reason = None;
    st.old_peer = false;
    st.orchestrator_state = Some(ConnectionState::Idle);
    st.connected_since = None;
    st.reconnecting = false;
    drop(st);
    phone_stats_provider().set_connected_since(None);
}

/// Run one worker body. A panic is caught, logged, and turned into the stopped
/// state, so the service keeps running and its restart policy can apply.
/// Returns whether the body completed without a panic.
fn guard_worker(state: &Arc<Mutex<JoinerState>>, body: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(()) => true,
        Err(_) => {
            tracing::error!("sharer loop panicked; sharing stopped");
            record_worker_panic(state);
            false
        }
    }
}

/// The expiry line under the pairing code. Pure, so the wording stays the
/// core's `CodeView::expiry_line` and the window follows the issue instant.
pub(crate) fn code_expiry_line(session: Option<&CodeSession>, now: Instant) -> String {
    let Some(session) = session else {
        return String::new();
    };
    CodeView {
        digits: session.code().as_str().to_string(),
        expires_in_secs: Some(session.remaining_at(now).as_secs()),
        expired: session.is_expired(now),
    }
    .expiry_line()
}

/// The health verdict from the platform facts. `handshake_age_ms` is the
/// boringtun handshake age (-1 = never, None = no stats). The moving age is
/// not part of the verdict; [`health_detail`] carries it for Details.
pub(crate) fn health_line(
    old_peer: bool,
    handshake_age_ms: Option<i64>,
    tunnel_expected: bool,
) -> String {
    let mut health = health_facts(handshake_age_ms, tunnel_expected);
    health.old_peer = old_peer;
    health.verdict().unwrap_or_default()
}

/// The handshake age for the Details view. No age is shown for a handshake
/// that never happened or is unknown.
pub(crate) fn health_detail(handshake_age_ms: Option<i64>, tunnel_expected: bool) -> String {
    health_facts(handshake_age_ms, tunnel_expected)
        .handshake_detail()
        .unwrap_or_default()
}

fn health_facts(handshake_age_ms: Option<i64>, tunnel_expected: bool) -> Health {
    let handshake = match handshake_age_ms {
        Some(ms) if ms >= 0 => Freshness::Age(ms as u64 / 1000),
        Some(_) if tunnel_expected => Freshness::Never,
        _ => Freshness::Unknown,
    };
    let path = classify_path(handshake, None);
    Health {
        handshake,
        path,
        old_peer: false,
    }
}

/// The retry line with the remaining wait rounded up to whole seconds.
pub(crate) fn retry_line(deadline: Option<Instant>, reason: Option<&str>, now: Instant) -> String {
    let Some(deadline) = deadline else {
        return String::new();
    };
    let remaining = deadline.saturating_duration_since(now);
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    RetryView {
        remaining_secs: secs,
        reason: reason.map(str::to_string),
    }
    .line()
}

/// A tunnel exists (or is being raised) in these states, so a handshake that
/// never happened means the peer is not reachable.
fn tunnel_expected(state: Option<ConnectionState>) -> bool {
    matches!(
        state,
        Some(ConnectionState::Connecting | ConnectionState::Connected)
    )
}

fn status_text(orch: &Orchestrator) -> String {
    let state = orch.state();
    // The card and the details must both show the real wait left and the
    // reason, not the retry interval. Every other state uses the core words.
    let retry = (state == ConnectionState::Retry).then(|| {
        let remaining = orch
            .retry_remaining_at(Instant::now())
            .unwrap_or_else(|| orch.retry_interval());
        let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        RetryView {
            remaining_secs: secs,
            reason: orch.last_disconnect_reason().map(str::to_string),
        }
    });
    Snapshot {
        state,
        code: None,
        retry,
        health: Health::default(),
    }
    .status_line()
}

/// The peer's display name for the live status ("Laptop - Blue Falcon"), the
/// same one the store keeps. `peer_role` is the PEER's role. Falls back to the
/// key prefix if the pair id cannot be derived (unusable peer key).
fn live_peer_name(own: &KeyPair, peer_pk_b64: &str, peer_role: Role) -> String {
    match PairSecret::derive(own, peer_pk_b64) {
        Ok(secret) => display_name(peer_role, &secret.pair_id()),
        Err(_) => peer_pk_b64.chars().take(12).collect(),
    }
}

/// The gateway's UDP port: `preferred` when it is free (9797 for WgServer's
/// clients), else another free port. The probe socket is closed again before
/// the gateway binds, so a port freed in between is also taken as is.
fn choose_listen_port(preferred: u16) -> u16 {
    let free_port = |port: u16| {
        std::net::UdpSocket::bind(("0.0.0.0", port))
            .ok()
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.port())
    };
    free_port(preferred)
        .or_else(|| free_port(0))
        .unwrap_or(preferred)
}

/// Reader of the bytes received so far (0 if the stats lock is poisoned).
fn rx_reader(stats: Arc<Mutex<WgStats>>) -> Box<dyn Fn() -> u64 + Send + Sync> {
    Box::new(move || stats.lock().map(|s| s.rx_bytes).unwrap_or(0))
}

/// Sharer side: WireGuard server + router. The peer endpoint is left as
/// `None` so wg.rs dynamic mode learns the Linux peer's address from the
/// first authenticated packet (hotspot subnet changes are picked up
/// automatically). The router (smoltcp NAT + DNS) shares this phone's
/// internet with the peer over the tunnel.
/// The gateway's router: DNS and ping on the gateway address, outbound only.
/// No inbound port forwards: a laptop reaching the phone's own ports is not
/// something the tunnel offers.
/// Holds the summary line the phone side can serve. Empty means the switch is
/// off or no summary is ready; the responder then stays silent.
pub struct PhoneStatsProvider {
    line: Mutex<Option<String>>,
    /// The exit reason of the previous process, served on the next answers.
    /// It must outlive one sampler tick: the local sampler rewrites the line
    /// every second while the laptop asks for a summary every five, so a
    /// single line would almost always miss it.
    exit: Mutex<Option<StickyExit>>,
    /// The counters of the live tunnel: the WireGuard loss and handshake
    /// round trip go into each answer.
    wg: Mutex<Option<Arc<Mutex<WgStats>>>>,
    /// The laptop's newest link report, when it came, and the line of its
    /// levels: the range tile of the phone.
    range: Mutex<RangeSeen>,
    /// The sharer's displayed Connected start (wall-clock ms, 0 = not
    /// connected). Each answer carries the elapsed time from it.
    connected_since: Mutex<Option<Instant>>,
}

/// How many answers carry an exit word before it is forgotten. Three cover
/// one lost datagram and one answer that reached the phone before its laptop
/// noticed the new session; the laptop asks every five seconds, so the word
/// is still fresh while they go out.
pub const EXIT_ANSWERS: u32 = 3;
/// A stale word must never describe a later outage: it is dropped after a
/// minute even when no laptop asked for a summary.
pub const EXIT_MAX_MS: u64 = 60_000;

/// The exit word of the previous process, kept while the laptop polls.
struct StickyExit {
    reason: ExitReason,
    set_at: Instant,
    served: u32,
}

#[derive(Default)]
struct RangeSeen {
    newest: Option<(jafnify_core::wifi_range::RangeReport, i64)>,
    trend: jafnify_core::tiles::TrendBuffer,
}

/// Wall-clock epoch milliseconds.
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A link report older than this is no report: the laptop stopped sending.
pub const RANGE_FRESH_MS: i64 = 10_000;

/// Below this many datagrams from the laptop the loss estimate says nothing:
/// an idle tunnel only carries keepalives.
pub const LOSS_MIN_PACKETS: u64 = 200;

/// The WireGuard loss and handshake round trip of the counters, when they
/// say something: `(loss %, rtt ms)`.
pub fn wg_link_numbers(stats: &WgStats) -> (Option<u8>, Option<u32>) {
    let loss = (stats.est_loss_permille >= 0 && stats.udp_rx_packets >= LOSS_MIN_PACKETS)
        .then(|| ((stats.est_loss_permille + 5) / 10).clamp(0, 100) as u8);
    let rtt = u32::try_from(stats.handshake_rtt_ms).ok();
    (loss, rtt)
}

impl PhoneStatsProvider {
    pub fn new() -> Self {
        Self {
            line: Mutex::new(None),
            exit: Mutex::new(None),
            wg: Mutex::new(None),
            range: Mutex::new(RangeSeen::default()),
            connected_since: Mutex::new(None),
        }
    }

    /// Keep the exit word of the previous process for the next answers. The
    /// phone calls this once, when sharing starts after the update or crash.
    pub fn set_exit(&self, reason: ExitReason) {
        if let Ok(mut slot) = self.exit.lock() {
            *slot = Some(StickyExit {
                reason,
                set_at: Instant::now(),
                served: 0,
            });
        }
    }

    /// One exit word for one answer, while it is still worth sending.
    fn exit_for_answer(&self) -> Option<ExitReason> {
        let mut slot = self.exit.lock().ok()?;
        let sticky = slot.as_mut()?;
        if sticky.set_at.elapsed().as_millis() as u64 > EXIT_MAX_MS {
            *slot = None;
            return None;
        }
        sticky.served = sticky.served.saturating_add(1);
        let reason = sticky.reason;
        if sticky.served >= EXIT_ANSWERS {
            *slot = None;
        }
        Some(reason)
    }

    /// The displayed Connected start the summary answer counts from.
    pub fn set_connected_since(&self, since: Option<Instant>) {
        if let Ok(mut slot) = self.connected_since.lock() {
            *slot = since;
        }
    }

    fn connected_ms(&self) -> Option<u64> {
        self.connected_since
            .lock()
            .ok()?
            .map(|since| Instant::now().duration_since(since).as_millis() as u64)
    }

    /// Replace the summary line; `None` clears it.
    pub fn set_line(&self, line: Option<String>) {
        if let Ok(mut slot) = self.line.lock() {
            *slot = line;
        }
    }

    /// Keep a link report of the laptop that arrived at `at_ms`.
    pub fn note_range(&self, report: jafnify_core::wifi_range::RangeReport, at_ms: i64) {
        if let Ok(mut seen) = self.range.lock() {
            seen.newest = Some((report, at_ms));
            seen.trend.push(at_ms, i64::from(report.level_dbm));
        }
    }

    /// The laptop's link report and its line, while it is fresh at `now_ms`.
    pub fn range(
        &self,
        now_ms: i64,
    ) -> Option<(
        jafnify_core::wifi_range::RangeReport,
        jafnify_core::tiles::TrendBuffer,
    )> {
        let seen = self.range.lock().ok()?;
        let (report, at_ms) = seen.newest?;
        (now_ms - at_ms <= RANGE_FRESH_MS).then(|| (report, seen.trend.clone()))
    }

    /// The counters of the tunnel that is up; `None` when it is gone.
    pub fn set_wg(&self, stats: Option<Arc<Mutex<WgStats>>>) {
        if let Ok(mut slot) = self.wg.lock() {
            *slot = stats;
        }
    }
}

impl Default for PhoneStatsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl libwgslirpy::router::StatsProvider for PhoneStatsProvider {
    fn respond(&self, request: &[u8]) -> Option<Vec<u8>> {
        match phone_stats::parse_request(request).ok()? {
            phone_stats::PhoneStatsRequest::Summary => {
                let line = self.line.lock().ok()?.clone()?;
                let mut stats = PhoneStats::parse_response(line.as_bytes()).ok()?;
                if let Some(report) = phone_stats::request_range(request) {
                    self.note_range(report, now_epoch_ms());
                }
                // This phone takes the laptop's link report.
                stats.caps_link = true;
                stats.sconn_ms = self.connected_ms();
                let wg = self.wg.lock().ok().and_then(|slot| slot.clone());
                if let Some(wg) = wg {
                    if let Ok(counters) = wg.lock() {
                        let (loss, rtt) = wg_link_numbers(&counters);
                        stats.wg_loss_pct = loss;
                        stats.wg_rtt_ms = rtt;
                    }
                }
                let line = with_summary_key(&stats.to_response(), self.exit_for_answer());
                Some(line.into_bytes())
            }
            phone_stats::PhoneStatsRequest::History => {
                Some(phone_stats::UNSUPPORTED_LINE.as_bytes().to_vec())
            }
        }
    }
}

static PHONE_STATS_PROVIDER: OnceLock<Arc<PhoneStatsProvider>> = OnceLock::new();

/// The process-wide provider the gateway router reads. It starts empty, so a
/// build with no phone feed answers nothing.
pub fn phone_stats_provider() -> Arc<PhoneStatsProvider> {
    PHONE_STATS_PROVIDER
        .get_or_init(|| Arc::new(PhoneStatsProvider::new()))
        .clone()
}

fn gateway_router_opts() -> libwgslirpy::router::Opts {
    libwgslirpy::router::Opts {
        dns_addr: Some(format!("{GATEWAY_DNS_IP}:53").parse().unwrap()),
        // The stats responder lives in the same smoltcp router as DNS. It has
        // no host socket, and it answers only packets that arrived through the
        // authenticated WireGuard tunnel.
        stats_addr: Some(
            format!("{GATEWAY_DNS_IP}:{}", phone_stats::PORT)
                .parse()
                .unwrap(),
        ),
        stats_provider: Some(phone_stats_provider()),
        pingable: Some(GATEWAY_DNS_IP.parse().unwrap()),
        mtu: 1420,
        // Values proven on the WgServer gateway (2026-07, 30+ min under load, no outage).
        tcp_buffer_size: 262144,
        incoming_udp: vec![],
        incoming_tcp: vec![],
    }
}

fn setup_gateway(
    keypair: &KeyPair,
    peer_pk: [u8; 32],
    listen_port: u16,
    state: &Arc<Mutex<JoinerState>>,
) -> anyhow::Result<Tunnel> {
    let peer_pk_b64 = base64::engine::general_purpose::STANDARD.encode(peer_pk);
    let private_secret = StaticSecret::from(parsebase64_32(&keypair.private_key_b64())?);
    let peer_pk = PublicKey::from(parsebase64_32(&peer_pk_b64)?);
    let stats = Arc::new(Mutex::new(WgStats::default()));
    phone_stats_provider().set_wg(Some(stats.clone()));
    let wg_opts = WgOpts {
        private_key: private_secret,
        peer_key: peer_pk,
        peer_endpoint: None,
        keepalive_interval: Some(KEEPALIVE),
        bind_ip_port: format!("0.0.0.0:{listen_port}").parse().unwrap(),
        stats: Some(stats.clone()),
    };
    let router_opts = gateway_router_opts();
    {
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.peer_id = peer_pk_b64.clone();
        s.peer_name = live_peer_name(keypair, &peer_pk_b64, Role::Joiner);
        // A sharer learns the peer endpoint from the first authenticated
        // packet, so there is no fixed endpoint to report.
        s.peer_endpoint.clear();
        s.stats = Some(stats.clone());
    }
    let handle = tokio::spawn(async move { libwgslirpy::run(wg_opts, router_opts, 1024).await });
    Ok(Tunnel {
        handle,
        rx: rx_reader(stats),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jafnify_core::{CODE_TTL, OLD_PEER_CARD_REASON, RETRY_INTERVAL};
    use std::net::Ipv4Addr;

    // Tests here share the multicast group; run them one at a time.
    static MULTICAST: Mutex<()> = Mutex::new(());

    /// A field of the first `STATS` line of a `diag()` snapshot.
    fn stat_value(diag: &str, key: &str) -> Option<String> {
        let stats = diag.lines().next()?;
        stats
            .split_whitespace()
            .find_map(|token| token.strip_prefix(&format!("{key}=")).map(str::to_string))
    }

    fn stat_u64(diag: &str, key: &str) -> u64 {
        stat_value(diag, key)
            .unwrap_or_else(|| panic!("{key} missing: {diag}"))
            .parse()
            .unwrap_or_else(|e| panic!("{key} is not a number: {e}"))
    }

    fn code() -> PairingCode {
        PairingCode::parse("1234").unwrap()
    }

    // F3: a panic in the outer join_loop thread must not leave a stale
    // "Connected" on the card while nothing runs.
    #[test]
    fn a_panicking_worker_reports_stopped_and_leaves_a_restart_possible() {
        let state = Arc::new(Mutex::new(JoinerState::default()));
        {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            s.status = "Connected".to_string();
            s.orchestrator_state = Some(ConnectionState::Connected);
            s.retry_reason = Some("a reason".to_string());
            s.peer_id = "peer".to_string();
        }
        let completed = guard_worker(&state, || panic!("injected worker panic"));
        assert!(!completed);
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(s.status, jafnify_core::share_notice::WORKER_STOPPED);
        assert_eq!(s.orchestrator_state, Some(ConnectionState::Idle));
        assert_eq!(s.retry_reason, None);
        assert!(s.peer_id.is_empty());
        assert!(s.stats.is_none());
    }

    #[test]
    fn the_worker_guard_lets_a_normal_return_through() {
        let state = Arc::new(Mutex::new(JoinerState::default()));
        assert!(guard_worker(&state, || {}));
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(s.status, "");
    }

    /// Both retry wordings are a retry state: the old "Retrying (Ns)" line and
    /// the core's "Connection lost - retrying in N s" line.
    fn is_retry(status: &str) -> bool {
        status.starts_with("Retrying") || status.starts_with("Connection lost")
    }

    // Go item 3: the window follows the issue instant. Re-arming at T+300 s
    // keeps the same window; 599 s is still good, 600 s is over.
    #[test]
    fn code_expiry_counts_down_from_the_issue_instant() {
        let issued = Instant::now();
        let session = CodeSession::issue_at(code(), issued);
        assert_eq!(
            code_expiry_line(Some(&session), issued),
            "Expires in 10 minutes"
        );
        assert_eq!(
            code_expiry_line(Some(&session), issued + Duration::from_secs(300)),
            "Expires in 5 minutes"
        );
        assert_eq!(
            code_expiry_line(Some(&session), issued + Duration::from_secs(599)),
            "Expires in 0:01"
        );
        assert_eq!(
            code_expiry_line(Some(&session), issued + CODE_TTL),
            "Code expired"
        );
        assert_eq!(code_expiry_line(None, issued), "");
    }

    #[test]
    fn health_line_maps_handshake_age_and_the_old_peer() {
        assert_eq!(health_line(false, Some(5_000), true), "Direct over hotspot");
        assert_eq!(health_line(false, Some(181_000), true), "Not reachable");
        // Never while the tunnel should be up: waiting for the first
        // handshake, not unreachable.
        assert_eq!(
            health_line(false, Some(-1), true),
            "Waiting for the first handshake"
        );
        // Never while no tunnel is expected: nothing to say.
        assert_eq!(health_line(false, Some(-1), false), "");
        // No stats at all: nothing to say.
        assert_eq!(health_line(false, None, true), "");
        assert_eq!(health_line(true, Some(5_000), true), OLD_PEER_CARD_REASON);
    }

    #[test]
    fn health_detail_carries_only_the_handshake_age() {
        assert_eq!(health_detail(Some(5_000), true), "Last handshake 5 s ago");
        assert_eq!(
            health_detail(Some(181_000), true),
            "Last handshake 181 s ago"
        );
        assert_eq!(health_detail(Some(-1), true), "");
        assert_eq!(health_detail(Some(-1), false), "");
        assert_eq!(health_detail(None, true), "");
    }

    #[test]
    fn retry_line_rounds_the_wait_up_and_carries_the_reason() {
        let now = Instant::now();
        assert_eq!(
            retry_line(
                Some(now + Duration::from_millis(9_500)),
                Some("Connection lost"),
                now
            ),
            "Connection lost - retrying in 10 s - Connection lost"
        );
        assert_eq!(
            retry_line(Some(now), None, now),
            "Connection lost - retrying in 0 s"
        );
        assert_eq!(retry_line(None, Some("x"), now), "");
    }

    #[test]
    fn the_status_line_is_the_core_line() {
        let idle = Orchestrator::new();
        assert_eq!(
            status_text(&idle),
            Snapshot::default().status_line(),
            "the JNI status line is rendered by the core"
        );

        let mut searching = Orchestrator::new();
        searching.start_search(code());
        let expected = Snapshot {
            state: ConnectionState::Searching,
            ..Snapshot::default()
        };
        assert_eq!(status_text(&searching), expected.status_line());
    }

    #[test]
    fn the_retry_status_line_has_the_seconds_left_and_the_reason() {
        let now = Instant::now();
        let mut orch = Orchestrator::new();
        orch.start_search(code());
        orch.on_disconnect_at_with_retry(
            "no data from peer for 35 s".to_string(),
            now,
            Duration::from_secs(10),
        );

        let line = status_text(&orch);
        assert!(line.starts_with("Connection lost - retrying in "), "{line}");
        assert!(line.ends_with(" - no data from peer for 35 s"), "{line}");
        assert!(!line.contains("(10s)"), "{line}");
    }

    // A restart must not mint a fresh code window: it reuses the session (and
    // its issue instant) the run was started with.
    #[test]
    fn a_code_restart_keeps_the_issue_instant() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        j.sharer_start_with("4321".into(), loopback_cfg())
            .expect("start");
        let issued = j
            .active_code
            .lock()
            .unwrap()
            .as_ref()
            .expect("code session")
            .issued_at();
        j.sharer_restart(loopback_cfg()).expect("code restart");
        let after = j
            .active_code
            .lock()
            .unwrap()
            .as_ref()
            .expect("code session")
            .issued_at();
        j.stop();
        assert_eq!(issued, after, "a restart extended the code window");
    }

    // Loopback only: an unpinned multicast socket would follow the host's
    // default route (a live wg0 tunnel on the dev machine). A test port other
    // than the product's 9799 keeps a running app on this host out of the test.
    fn loopback_cfg() -> DiscoveryConfig {
        DiscoveryConfig {
            iface: Some(Ipv4Addr::LOCALHOST),
            addr: "239.255.42.99:49801".parse().unwrap(),
            ..Default::default()
        }
    }

    // Lab report, finding 4: nothing ever announced the 0x01
    // discovery token, so the sharer stayed in Searching forever.
    #[test]
    fn announce_discovery_sends_own_token_once_per_interval() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let listener = Discovery::bind(&loopback_cfg()).unwrap();
        let sender = Discovery::bind(&loopback_cfg()).unwrap();
        let mut orch = Orchestrator::new();
        orch.start_search(code());
        let mut last = None;
        let now = Instant::now();

        announce_discovery(&sender, &orch, &mut last, now);
        let (_, token) = listener
            .try_receive(Duration::from_millis(500))
            .unwrap()
            .expect("no 0x01 frame announced");
        assert!(token.verify());
        assert_eq!(Some(token), orch.own_token());

        // Within the interval: nothing more is sent.
        announce_discovery(&sender, &orch, &mut last, now + Duration::from_millis(200));
        assert!(listener
            .try_receive(Duration::from_millis(150))
            .unwrap()
            .is_none());

        // After the interval: announced again.
        announce_discovery(&sender, &orch, &mut last, now + ANNOUNCE_INTERVAL);
        assert!(listener
            .try_receive(Duration::from_millis(500))
            .unwrap()
            .is_some());
    }

    // Discovery fix: the app lists the interfaces to use (hotspot
    // included, cellular excluded) instead of trusting the default route.
    #[test]
    fn iface_list_parses_commas_and_skips_junk() {
        assert_eq!(
            parse_ifaces("10.20.89.13, 192.168.43.1,,not-an-ip,10.0.0.256"),
            vec![
                "10.20.89.13".parse::<Ipv4Addr>().unwrap(),
                "192.168.43.1".parse().unwrap()
            ]
        );
        assert!(parse_ifaces("").is_empty());
    }

    #[test]
    fn discovery_config_uses_the_list_and_an_empty_list_keeps_the_default_route() {
        let cfg = discovery_config("10.20.89.13,10.77.0.1");
        assert_eq!(
            cfg.interfaces(),
            vec![
                "10.20.89.13".parse::<Ipv4Addr>().unwrap(),
                "10.77.0.1".parse().unwrap()
            ]
        );
        assert!(discovery_config("").interfaces().is_empty());
    }

    // Finding 2: multicast loop returns our own 0x02 frame; it must not be
    // taken for the peer.
    #[test]
    fn sharer_start_with_rejects_invalid_code() {
        let joiner = Joiner::new();
        assert_eq!(
            joiner.sharer_start_with("12ab".into(), loopback_cfg()),
            Err("Invalid code".to_string())
        );
    }

    // Host harness entry point: the real sharer loop, with a caller-chosen
    // code and multicast interface, announces its 0x01 token on that interface.
    #[test]
    fn sharer_start_with_searches_and_announces_on_given_iface() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let listener = Discovery::bind(&loopback_cfg()).unwrap();
        let joiner = Joiner::new();
        joiner
            .sharer_start_with("1234".into(), loopback_cfg())
            .unwrap();
        assert_eq!(
            joiner.sharer_start_with("1234".into(), loopback_cfg()),
            Err("Already running".to_string())
        );
        let end = Instant::now() + Duration::from_secs(3);
        let mut heard = false;
        while Instant::now() < end && !heard {
            if let Some((_, token)) = listener.try_receive(Duration::from_millis(100)).unwrap() {
                // Ignore stray frames: only a token for our code counts.
                heard = token.verify();
            }
        }
        let end = Instant::now() + Duration::from_secs(1);
        let mut status = joiner.status();
        while !status.starts_with("Searching for the other device") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            status = joiner.status();
        }
        joiner.stop();
        assert!(heard, "sharer announced no 0x01 token");
        assert!(
            status.starts_with("Searching for the other device"),
            "status was {status:?}"
        );
    }

    // Android's JNI reads seven lines; the clock is the additive eighth. The
    // host CLI keeps the five-line shape.
    #[test]
    fn status_with_endpoint_adds_the_endpoint_and_status_stays_five_lines() {
        let joiner = Joiner::new();
        {
            let mut s = joiner.state.lock().unwrap_or_else(|e| e.into_inner());
            s.status = "Connected".to_string();
            s.peer_id = "peerid".to_string();
            s.peer_name = "Laptop - Blue Falcon".to_string();
            s.peer_endpoint = "10.1.0.2:9797".to_string();
            s.stats = Some(Arc::new(Mutex::new(libwgslirpy::wg::WgStats {
                rx_bytes: 2048,
                tx_bytes: 4096,
                ..libwgslirpy::wg::WgStats::default()
            })));
        }

        assert_eq!(
            joiner.status_with_endpoint(),
            "Connected\npeerid\nLaptop - Blue Falcon\n2048\n4096\n10.1.0.2:9797\n0\n0\n0"
        );
        assert_eq!(joiner.status_with_endpoint().lines().count(), 9);
        assert_eq!(
            joiner.status(),
            "Connected\npeerid\nLaptop - Blue Falcon\n2048\n4096"
        );
        assert_eq!(joiner.status().lines().count(), 5);
    }

    #[test]
    fn status_with_endpoint_reports_the_address_the_gateway_learned() {
        // Live: the phone's Details read "Laptop address unknown"
        // while connected, because a sharer never stores a fixed endpoint.
        let joiner = Joiner::new();
        {
            let mut s = joiner.state.lock().unwrap_or_else(|e| e.into_inner());
            s.status = "Connected".to_string();
            s.stats = Some(Arc::new(Mutex::new(libwgslirpy::wg::WgStats {
                peer_addr: "10.174.190.70:9796".to_string(),
                ..libwgslirpy::wg::WgStats::default()
            })));
        }
        assert_eq!(
            joiner.status_with_endpoint().lines().nth(5),
            Some("10.174.190.70:9796")
        );
    }

    // Task 4: the diagnostic snapshot is additive. Its first line carries
    // every common key, each value is one whitespace-free token, and unknown
    // values are -1 rather than a fabricated zero. Status stays unchanged.
    #[test]
    fn diag_first_line_has_every_common_key_and_no_spaces_in_values() {
        let joiner = Joiner::new();
        let status_before = joiner.status();
        let endpoint_before = joiner.status_with_endpoint();
        let diag = joiner.diag();
        let stats = diag.lines().next().expect("no STATS line");
        assert!(stats.starts_with("STATS "), "{stats}");
        for key in [
            "udp_rx_packets",
            "udp_rx_bytes",
            "udp_tx_packets",
            "udp_tx_bytes",
            "udp_send_errors",
            "udp_recv_errors",
            "udp_last_errno",
            "last_rx_ms",
            "last_tx_ms",
            "peer",
            "local",
            "wg_handshake_ms",
            "wg_rx_bytes",
            "wg_tx_bytes",
            "worker_known",
            "worker_code",
            "worker_tunnel",
        ] {
            assert!(stats.contains(&format!("{key}=")), "{key} missing: {stats}");
        }
        for token in stats.split_whitespace().skip(1) {
            let (key, value) = token
                .split_once('=')
                .unwrap_or_else(|| panic!("not key=value: {token}"));
            assert!(!key.is_empty(), "{token}");
            assert!(!value.is_empty(), "{token}");
            assert!(!value.chars().any(char::is_whitespace), "space in {token}");
            // No key material: a 43/44-char base64 token would be key-shaped.
            let key_shaped = value.len() >= 43
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
            assert!(!key_shaped, "key-shaped value: {token}");
        }
        assert_eq!(stat_value(&diag, "peer").as_deref(), Some("-1"));
        assert_eq!(stat_value(&diag, "local").as_deref(), Some("-1"));
        assert_eq!(stat_value(&diag, "udp_last_errno").as_deref(), Some("-1"));
        assert_eq!(stat_value(&diag, "last_rx_ms").as_deref(), Some("-1"));
        assert_eq!(stat_value(&diag, "last_tx_ms").as_deref(), Some("-1"));
        assert_eq!(stat_value(&diag, "wg_handshake_ms").as_deref(), Some("-1"));

        // The existing five/six line status is untouched by diag().
        assert_eq!(joiner.status(), status_before);
        assert_eq!(joiner.status_with_endpoint(), endpoint_before);
        assert_eq!(joiner.status().lines().count(), 5);
    }

    // Task 4 rig test: the worker liveness counters advance, an endpoint and a
    // socket-rebind event are recorded, and every reset carries a reason.
    #[test]
    fn diag_records_worker_progress_endpoint_rebind_and_reset_reason() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");

        let first = rig.joiner.diag();
        let code1 = stat_u64(&first, "worker_code");
        assert!(code1 > 0, "code worker never ran: {first}");
        let tunnel1 = stat_u64(&first, "worker_tunnel");
        assert!(tunnel1 > 0, "tunnel supervision never counted: {first}");
        assert!(first.contains("event=peer_endpoint "), "{first}");

        std::thread::sleep(Duration::from_millis(150));
        let second = rig.joiner.diag();
        let code2 = stat_u64(&second, "worker_code");
        assert!(
            code2 > code1,
            "worker_code did not increase: {code1} -> {code2}"
        );

        // Stall the tunnel: a no-data decision and a reset, both with a reason.
        rig.advance(RX_STALL_TIMEOUT + Duration::from_secs(1));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.starter.rx.store(0, SeqCst);
        rig.advance(RETRY_INTERVAL);
        rig.wait("searching", |r| {
            r.status() == "Searching for the other device"
        });

        let diag = rig.joiner.diag();
        assert!(diag.contains("event=no_data "), "{diag}");
        assert!(diag.contains("event=reset "), "{diag}");
        assert!(diag.contains("reason=No_data_from_peer"), "{diag}");
        assert!(diag.contains("event=socket_rebind "), "{diag}");
    }

    // The known track has its own worker counter, separate from the code track.
    #[test]
    fn diag_counts_the_known_worker_without_the_code_worker() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.store_pair(&KeyPair::generate());
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);
        std::thread::sleep(Duration::from_millis(100));
        let diag = rig.joiner.diag();
        assert!(stat_u64(&diag, "worker_known") > 0, "{diag}");
        assert_eq!(stat_u64(&diag, "worker_code"), 0, "{diag}");
    }

    // The PAKE peer of the same code is accepted; a peer of another code
    // never is.
    #[test]
    fn a_peer_with_the_same_code_is_accepted_and_a_wrong_code_is_not() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let me = KeyPair::generate();
        let peer = KeyPair::generate();
        let now = Instant::now();
        {
            let mut mine = PakeLink::open(&loopback_cfg(), Role::Joiner, &code(), &me, 0).unwrap();
            let mut theirs =
                PakeLink::open(&loopback_cfg(), Role::Sharer, &code(), &peer, 9797).unwrap();
            let mut accepted = false;
            for _ in 0..40 {
                let _ = mine.poll(now);
                let _ = theirs.poll(now);
                if mine.poll(now).unwrap().peer.is_some() {
                    accepted = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(accepted, "the peer with the same code was never accepted");
        }

        // A peer of another code never becomes a peer.
        let other = PairingCode::parse("9999").unwrap();
        let stranger = KeyPair::generate();
        let mut mine = PakeLink::open(&loopback_cfg(), Role::Joiner, &code(), &me, 0).unwrap();
        let mut theirs =
            PakeLink::open(&loopback_cfg(), Role::Sharer, &other, &stranger, 9797).unwrap();
        for _ in 0..40 {
            let _ = mine.poll(now);
            let _ = theirs.poll(now);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            mine.poll(now).unwrap().peer.is_none(),
            "a wrong code was accepted"
        );
    }

    // The gateway opens no inbound port forwards into the phone.
    #[test]
    fn a_sharer_gateway_opens_no_port_forwards() {
        let opts = gateway_router_opts();
        assert!(opts.incoming_udp.is_empty());
        assert!(opts.incoming_tcp.is_empty());
    }

    // ---- Connecting and loss rules (lab restart deadlock) ----
    // The sharer under test runs the real join_loop on loopback multicast with
    // an injected clock and a fake tunnel; the peer is a "ghost" driven by hand.

    use jafnify_core::known::{pair_record, Frame, KnownSocket, KnownTrack};
    use jafnify_core::pairing::DiscoveryToken;
    use jafnify_core::state::{Peer, Role, StateStore, TomlStateStore};
    use std::cell::{Cell, RefCell, RefMut};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};

    const GHOST_NONCE: [u8; 16] = [9; 16];

    struct Alive(Arc<AtomicUsize>);
    impl Drop for Alive {
        fn drop(&mut self) {
            self.0.fetch_sub(1, SeqCst);
        }
    }

    /// Records (own pk, peer pk) per start; the "tunnel" is a task that lives
    /// until aborted, whose rx counter the test sets.
    struct FakeStarter {
        rx: Arc<AtomicU64>,
        alive: Arc<AtomicUsize>,
        started: Mutex<Vec<([u8; 32], [u8; 32])>>,
        /// Own private key of each start. Secret: compared, never printed.
        privs: Mutex<Vec<String>>,
    }

    impl TunnelStarter for FakeStarter {
        fn start(&self, req: TunnelRequest<'_>) -> anyhow::Result<Tunnel> {
            self.started
                .lock()
                .unwrap()
                .push((req.keypair.public_key_bytes(), req.peer_pk));
            self.privs
                .lock()
                .unwrap()
                .push(req.keypair.private_key_b64());
            self.alive.fetch_add(1, SeqCst);
            let guard = Alive(self.alive.clone());
            let handle = tokio::spawn(async move {
                let _g = guard;
                std::future::pending::<anyhow::Result<()>>().await
            });
            let rx = self.rx.clone();
            Ok(Tunnel {
                handle,
                rx: Box::new(move || rx.load(SeqCst)),
            })
        }
    }

    struct Rig {
        joiner: Joiner,
        starter: Arc<FakeStarter>,
        offset_ms: Arc<AtomicU64>,
        /// The base of the fake clock: the ghost polls on the same time as the
        /// joiner under test.
        base: Instant,
        disc: Discovery,
        /// The ghost joiner of the code track (PAKE: 0x05 Hello, 0x06 Confirm).
        ghost: RefCell<Option<PakeLink>>,
        /// While muted the ghost does not poll: it has no socket yet, so the
        /// sharer's first Hello is lost.
        muted: Cell<bool>,
        /// How many Hellos of the sharer the ghost has already counted.
        last_hellos: Cell<u64>,
        key: KeyPair,
        dir: tempfile::TempDir,
        /// The joiner's no-network seam (see `Env::send_fault`).
        send_fault: Arc<Mutex<Option<std::io::ErrorKind>>>,
    }

    impl Rig {
        fn new() -> Rig {
            let starter = Arc::new(FakeStarter {
                rx: Arc::new(AtomicU64::new(0)),
                alive: Arc::new(AtomicUsize::new(0)),
                started: Mutex::new(Vec::new()),
                privs: Mutex::new(Vec::new()),
            });
            let offset_ms = Arc::new(AtomicU64::new(0));
            let base = Instant::now();
            let off = offset_ms.clone();
            let clock: Clock = Arc::new(move || base + Duration::from_millis(off.load(SeqCst)));
            let send_fault: Arc<Mutex<Option<std::io::ErrorKind>>> = Arc::default();
            Rig {
                send_fault: send_fault.clone(),
                joiner: Joiner::with_env(Env {
                    starter: starter.clone(),
                    clock,
                    // Port 0 always binds, so a rig never collides with a real 9797.
                    listen_port: 0,
                    send_fault,
                }),
                starter,
                offset_ms,
                base,
                disc: Discovery::bind(&loopback_cfg()).unwrap(),
                ghost: RefCell::new(None),
                muted: Cell::new(false),
                last_hellos: Cell::new(0),
                key: KeyPair::generate(),
                dir: tempfile::tempdir().unwrap(),
            }
        }

        /// A rig whose Joiner has a state file (the Android store).
        fn with_state() -> Rig {
            let rig = Rig::new();
            rig.joiner.set_state_path(rig.state_path());
            rig
        }

        fn state_path(&self) -> std::path::PathBuf {
            self.dir.path().join("state.toml")
        }

        fn stored(&self) -> Vec<Peer> {
            TomlStateStore::new(self.state_path()).load().unwrap()
        }

        /// Pre-fill the slot: a pair with the ghost (`self.key`) as the peer.
        fn store_pair(&self, own: &KeyPair) {
            let rec = pair_record(own, &self.key.public_key_b64(), Role::Sharer, None, 1).unwrap();
            TomlStateStore::new(self.state_path()).save(&[rec]).unwrap();
        }

        fn start_known(&self) {
            self.joiner.sharer_start_known(loopback_cfg()).unwrap();
        }

        /// The ghost joiner of a known pair, as a real track (own = `self.key`).
        fn known_ghost(&self, sharer: &KeyPair) -> KnownTrack {
            KnownTrack::open(
                &loopback_cfg(),
                &self.key,
                &sharer.public_key_b64(),
                Role::Joiner,
                Instant::now(),
            )
            .unwrap()
        }

        /// Move the sharer's clock one second per step (a Hello each time)
        /// while the ghost polls in real time, until `cond`.
        fn drive(&self, ghost: &mut KnownTrack, what: &str, cond: impl Fn(&Rig) -> bool) {
            let end = Instant::now() + Duration::from_secs(5);
            while !cond(self) {
                assert!(
                    Instant::now() < end,
                    "timeout: {what}; status {:?}",
                    self.status()
                );
                self.advance(Duration::from_secs(1));
                ghost.poll(Instant::now()).unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn start(&self) {
            self.joiner
                .sharer_start_with("1234".into(), loopback_cfg())
                .unwrap();
        }

        fn status(&self) -> String {
            self.joiner
                .status()
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        }

        fn advance(&self, d: Duration) {
            self.offset_ms.fetch_add(d.as_millis() as u64, SeqCst);
        }

        /// Wait for a condition. The fake clock moves one second per step,
        /// like `drive` does for the known track, so the timers of the joiner
        /// under test (auto-accept, connect timeout) and the Hello cadence of
        /// the ghost both progress.
        fn wait(&self, what: &str, cond: impl Fn(&Rig) -> bool) {
            let end = Instant::now() + Duration::from_secs(5);
            while !cond(self) {
                assert!(
                    Instant::now() < end,
                    "timeout: {what}; status {:?}",
                    self.status()
                );
                self.advance(Duration::from_secs(1));
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn starts(&self) -> usize {
            self.starter
                .started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
        }

        /// The phone took the new session of the paired device over.
        fn took_over(&self) -> bool {
            self.joiner
                .state
                .lock()
                .unwrap()
                .events
                .iter()
                .any(|e| e.contains("event=session_takeover"))
        }

        fn ghost_token(&self) {
            self.disc
                .announce(&DiscoveryToken::derive(&GHOST_NONCE))
                .unwrap();
        }

        /// The ghost joiner's PAKE link: the same code, the joiner side. Its
        /// WireGuard port is the standard one, so the stored peer endpoint
        /// keeps its old shape.
        fn ghost_pake(&self) -> RefMut<'_, PakeLink> {
            let mut slot = self.ghost.borrow_mut();
            if slot.is_none() {
                *slot = Some(
                    PakeLink::open(
                        &loopback_cfg(),
                        Role::Joiner,
                        &code(),
                        &self.key,
                        LISTEN_PORT,
                    )
                    .unwrap(),
                );
            }
            RefMut::map(slot, |p| p.as_mut().unwrap())
        }

        /// The fake clock the joiner under test uses.
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.offset_ms.load(SeqCst))
        }

        /// The ghost sends its Hello and answers the sharer's one with a
        /// Confirm. It polls on the fake clock, like the joiner under test.
        fn poll_ghost(&self) {
            if self.muted.get() {
                return;
            }
            let mut ghost = self.ghost_pake();
            let _ = ghost.poll(self.now());
        }

        /// Hellos of the sharer seen by the ghost since the last call.
        fn frames(&self) -> usize {
            let seen = self.ghost_pake().hellos_seen();
            let n = seen.saturating_sub(self.last_hellos.get());
            self.last_hellos.set(seen);
            n as usize
        }

        /// Waits for the sharer to send a NEW Hello.
        ///
        /// The fake clock moves one second per call, not per iteration: the
        /// joiner under test sends one Hello per second of its own clock, so
        /// a clock that runs a hundred times faster than the real one would
        /// end the attempt before the sharer ever sent a second Hello. The
        /// wait itself is in real time and it looks at the absolute count of
        /// Hellos the ghost has seen, so a Hello that arrived before the call
        /// cannot be counted twice.
        fn wait_frame(&self, what: &str) {
            let before = self.ghost_pake().hellos_seen();
            let end = Instant::now() + Duration::from_secs(3);
            self.advance(ANNOUNCE_INTERVAL);
            loop {
                self.poll_ghost();
                let seen = self.ghost_pake().hellos_seen();
                if seen > before {
                    self.last_hellos.set(seen);
                    return;
                }
                assert!(
                    Instant::now() < end,
                    "no Hello from the sharer: {what}; status {:?}",
                    self.status()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Nonces of the sharer's 0x01 discovery tokens seen so far.
        fn sharer_nonces(&self) -> Vec<[u8; 16]> {
            let end = Instant::now() + Duration::from_millis(150);
            let mut out = Vec::new();
            while Instant::now() < end {
                if let Some((_, t)) = self.disc.try_receive(Duration::from_millis(10)).unwrap() {
                    if t.verify() && t.nonce != GHOST_NONCE {
                        out.push(t.nonce);
                    }
                }
            }
            out
        }

        /// Ghost announces itself until the sharer notices, then the 3 s
        /// auto-accept elapses on the fake clock. Every attempt is a new
        /// PAKE session, so the ghost starts over too.
        fn to_connecting(&self) {
            self.ghost.replace(None);
            self.wait("peer found", |r| {
                r.ghost_token();
                r.poll_ghost();
                r.status() == "Pairing"
            });
            self.advance(AUTO_ACCEPT_DELAY);
            self.wait("connecting", |r| {
                r.poll_ghost();
                r.status() == "Connecting"
            });
        }

        /// The ghost answers the sharer's Hello with a Confirm; the sharer
        /// starts its tunnel.
        fn to_tunnel(&self, nth: usize) {
            self.wait("tunnel start", |r| {
                r.poll_ghost();
                r.starts() >= nth
            });
            // Let the loop run once more (fresh connect window opens).
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            self.joiner.stop();
        }
    }

    #[test]
    fn sharer_reports_connected_only_after_first_rx() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        assert_eq!(
            rig.status(),
            "Connecting",
            "Connected on receipt of the 0x02"
        );
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
    }

    #[test]
    fn sharer_keeps_sending_hellos_until_rx_then_stops() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        rig.to_connecting();
        rig.wait_frame("first announcement");
        rig.to_tunnel(1);
        rig.frames();
        for i in 0..3 {
            rig.wait_frame(&format!("second {i} after the tunnel came up"));
        }
        rig.starter.rx.store(1, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        // Drain what the sharer sent before it stopped, then watch for new
        // Hellos.
        rig.poll_ghost();
        rig.frames();
        for _ in 0..3 {
            rig.advance(ANNOUNCE_INTERVAL);
            rig.poll_ghost();
            std::thread::sleep(Duration::from_millis(150));
        }
        assert_eq!(rig.frames(), 0, "announces forever after Connected");
    }

    // The old 0x02 case ("the first frame of each side is lost") has no
    // counterpart here: the PAKE Hello is retransmitted every second and the
    // Confirm is answered from the Hello, so the pairing survives a lost
    // frame without a special case. The retransmission is covered by
    // `sharer_keeps_sending_hellos_until_rx_then_stops`.

    #[test]
    fn sharer_gives_up_after_connect_timeout_and_drops_the_tunnel() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.advance(CONNECT_TIMEOUT - Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(rig.status(), "Connecting");
        assert_eq!(rig.starter.alive.load(SeqCst), 1);

        rig.advance(Duration::from_secs(2));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.wait("tunnel dropped", |r| r.starter.alive.load(SeqCst) == 0);
    }

    // The lab deadlock: the sharer sat in Connected forever with a peer that
    // never sent WG data. It must now search afresh -- new nonce, new keypair --
    // and connect to the peer when that appears again.
    #[test]
    fn sharer_researches_afresh_after_giving_up_and_reconnects() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        let first_nonce = {
            let end = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(n) = rig.sharer_nonces().first() {
                    break *n;
                }
                assert!(Instant::now() < end, "sharer announced no 0x01");
            }
        };
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.advance(CONNECT_TIMEOUT + Duration::from_secs(1));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.advance(RETRY_INTERVAL);
        rig.wait("searching", |r| {
            r.status() == "Searching for the other device"
        });
        rig.sharer_nonces();
        rig.advance(ANNOUNCE_INTERVAL);
        let end = Instant::now() + Duration::from_secs(3);
        let second_nonce = loop {
            if let Some(n) = rig.sharer_nonces().first() {
                break *n;
            }
            assert!(Instant::now() < end, "no 0x01 after retry");
        };
        assert_ne!(first_nonce, second_nonce, "nonce reused across retry");

        rig.to_connecting();
        rig.to_tunnel(2);
        let started = rig
            .starter
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_ne!(started[0].0, started[1].0, "keypair reused across retry");
        rig.starter.rx.store(7, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
    }

    // ---- Known peer ----
    // The sharer persists a code pairing after the first rx > 0 and, with a
    // stored peer, runs the known track: gateway with the stored keys at
    // start, 0x03 Hello/Proof, no 0x01/0x02. Keys are generated per test; the
    // key assertions never print key material.

    #[test]
    fn code_pairing_is_persisted_only_after_the_first_rx() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        std::thread::sleep(Duration::from_millis(200));
        assert!(rig.stored().is_empty(), "persisted before any rx");

        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        let peers = rig.stored();
        assert_eq!(peers.len(), 1);
        let p = &peers[0];
        assert!(p.public_key == rig.key.public_key_b64(), "wrong peer key");
        let used = rig.starter.privs.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
        assert!(
            p.private_key.as_deref() == Some(used.as_str()),
            "stored key is not the tunnel's"
        );
        assert_eq!(p.role, Role::Joiner, "role is the peer's");
        assert_eq!(p.last_endpoint.as_deref(), Some("127.0.0.1:9797"));
        let mode =
            |p: std::path::PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(rig.state_path()), 0o600);
    }

    #[test]
    fn a_connect_that_never_gets_data_persists_nothing() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.advance(CONNECT_TIMEOUT + Duration::from_secs(1));
        rig.wait("retry", |r| is_retry(&r.status()));
        assert!(rig.stored().is_empty());
    }

    #[test]
    fn a_new_pairing_replaces_the_stored_peer() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let old = Peer {
            id: "old".into(),
            name: "old".into(),
            public_key: KeyPair::generate().public_key_b64(),
            role: Role::Joiner,
            last_connected: None,
            private_key: Some(KeyPair::generate().private_key_b64()),
            last_endpoint: None,
        };
        TomlStateStore::new(rig.state_path()).save(&[old]).unwrap();
        rig.start(); // --code: a deliberate new pairing
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(1, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        let peers = rig.stored();
        assert_eq!(peers.len(), 1, "v1 keeps one known peer");
        assert!(peers[0].public_key == rig.key.public_key_b64());
    }

    #[test]
    fn known_start_needs_a_paired_device() {
        let rig = Rig::new(); // no state file at all
        assert_eq!(
            rig.joiner.sharer_start_known(loopback_cfg()),
            Err("no paired device".to_string())
        );
        let rig = Rig::with_state(); // empty store
        assert!(!rig.joiner.is_paired());
        assert_eq!(
            rig.joiner.sharer_start_known(loopback_cfg()),
            Err("no paired device".to_string())
        );
        rig.store_pair(&KeyPair::generate());
        assert!(rig.joiner.is_paired());
    }

    // Step 2c: what the Java side asks about the stored pair.
    #[test]
    fn paired_name_is_the_pair_display_name_and_none_without_a_pair() {
        use jafnify_core::known::PairSecret;
        use jafnify_core::names::display_name;
        let rig = Rig::new(); // no state path
        assert_eq!(rig.joiner.paired_name(), None);
        let rig = Rig::with_state(); // empty store
        assert_eq!(rig.joiner.paired_name(), None);
        let own = KeyPair::generate();
        rig.store_pair(&own);
        // The rig stores a pair as the sharer, so the peer is the joiner.
        let id = PairSecret::derive(&own, &rig.key.public_key_b64())
            .unwrap()
            .pair_id();
        assert_eq!(
            rig.joiner.paired_name(),
            Some(display_name(Role::Joiner, &id))
        );
    }

    // Step 2: the live name (status line 3) is the same display name the
    // store keeps, not a key prefix.
    #[test]
    fn live_peer_name_equals_the_name_pair_record_stores() {
        let (own, peer) = (KeyPair::generate(), KeyPair::generate());
        let peer_b64 = peer.public_key_b64();
        // Sharer side: the peer is the joiner.
        let stored = pair_record(&own, &peer_b64, Role::Sharer, None, 1).unwrap();
        assert_eq!(live_peer_name(&own, &peer_b64, Role::Joiner), stored.name);
        assert!(stored.name.starts_with("Laptop - "));
    }

    #[test]
    fn live_peer_name_falls_back_to_the_key_prefix_for_an_unusable_key() {
        let own = KeyPair::generate();
        let zero = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert_eq!(live_peer_name(&own, &zero, Role::Joiner), "AAAAAAAAAAAA");
    }

    #[test]
    fn a_taken_listen_port_gives_the_sharer_another_port() {
        let (held, port) = held_port();
        let chosen = choose_listen_port(port);
        assert_ne!(chosen, port, "the held port must not be chosen");
        assert_ne!(chosen, 0);
        drop(held);
        assert_eq!(choose_listen_port(port), port, "a free port is kept");
    }

    /// A joiner whose gateway port is `port` (the rig's Env otherwise), on the
    /// rig's state file, so a test can hold that port first.
    fn joiner_on_port(rig: &Rig, port: u16) -> Joiner {
        let j = Joiner::with_env(Env {
            starter: rig.starter.clone(),
            clock: Arc::new(Instant::now),
            listen_port: port,
            send_fault: Arc::default(),
        });
        j.set_state_path(rig.state_path());
        j
    }

    fn held_port() -> (std::net::UdpSocket, u16) {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = s.local_addr().unwrap().port();
        (s, port)
    }

    // A taken 9797 is not an error: the sharer uses another port and announces
    // it, and the joiner connects to the announced port.
    #[test]
    fn a_code_start_succeeds_on_another_port_when_the_listen_port_is_taken() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        let r = j.sharer_start_with("4321".into(), loopback_cfg());
        j.stop();
        r.expect("start must not fail while the preferred port is held");
    }

    #[test]
    fn a_known_start_succeeds_on_another_port_when_the_listen_port_is_taken() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.store_pair(&KeyPair::generate());
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        let r = j.sharer_start_known(loopback_cfg());
        j.stop();
        r.expect("start must not fail while the preferred port is held");
    }

    /// A version-1 0x02 frame (key + MAC over the key) under `code`, as the
    /// old app sends it.
    fn legacy_handshake_frame(code: &PairingCode, pk: [u8; 32]) -> Vec<u8> {
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(code.as_str().as_bytes()).unwrap();
        mac.update(&pk);
        let mut f = vec![0x02];
        f.extend_from_slice(&pk);
        f.extend_from_slice(&mac.finalize().into_bytes());
        f
    }

    // An older peer must be named to the user, not left in a silent search.
    #[test]
    fn an_old_peer_is_reported_as_an_update_to_install() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.joiner
            .sharer_start_with("1234".into(), loopback_cfg())
            .expect("start");
        let old = legacy_handshake_frame(&code(), KeyPair::generate().public_key_bytes());
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !rig.status().contains(OLD_PEER_MESSAGE) {
            sender.send_to(&old, "127.0.0.1:49801").unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        let status = rig.status();
        rig.joiner.stop();
        assert!(status.contains(OLD_PEER_MESSAGE), "{status}");
    }

    // A LAN guesser sends well-formed PAKE Hellos under a wrong code. They
    // never stop the pairing: the attempt runs on and ends at the connect
    // timeout.
    #[test]
    fn a_flood_of_wrong_code_hellos_does_not_stop_the_pairing() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        // The real peer stays quiet: only the guesser talks.
        rig.muted.set(true);
        rig.start();
        rig.to_connecting();
        let guesser = KeyPair::generate();
        let wrong = PairingCode::parse("9999").unwrap();
        let mut stranger =
            PakeLink::open(&loopback_cfg(), Role::Joiner, &wrong, &guesser, 0).unwrap();
        let now = Instant::now();
        for _ in 0..10 {
            let _ = stranger.poll(now);
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = rig.status();
        assert!(
            !status.contains("Too many wrong codes"),
            "a wrong code stopped the pairing: {status}"
        );
        assert_eq!(status, "Connecting", "a wrong code stopped the pairing");
    }

    // Stop followed at once by Start (Pair new device) must not trip over the
    // old gateway's socket, which is released a moment after Stop.
    #[test]
    fn a_port_released_just_after_the_start_call_is_tolerated() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let (held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });
        let r = j.sharer_start_with("4321".into(), loopback_cfg());
        j.stop();
        releaser.join().unwrap();
        r.expect("a port freed within the wait must not fail the start");
    }

    // A restart before pairing must keep the code track (and its code), even
    // when a stored pair from an older run exists on disk.
    #[test]
    fn a_restart_before_pairing_keeps_the_code_track() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.store_pair(&KeyPair::generate());
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        j.sharer_start_with("4321".into(), loopback_cfg())
            .expect("start");
        assert!(!j.state.lock().unwrap_or_else(|e| e.into_inner()).paired);
        j.sharer_restart(loopback_cfg()).expect("code restart");
        assert_eq!(
            j.active_code
                .lock()
                .unwrap()
                .as_ref()
                .map(|session| session.code().as_str()),
            Some("4321"),
            "a code run must not switch to the stored pair before it pairs"
        );
        j.stop();
    }

    // The real persist path moves the run to the known track, and a restart
    // follows it there.
    #[test]
    fn a_restart_after_pairing_uses_the_known_track() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        assert_eq!(rig.stored().len(), 1);
        assert!(
            rig.joiner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .paired
        );
        rig.joiner
            .sharer_restart(loopback_cfg())
            .expect("known restart");
        assert!(rig
            .joiner
            .active_code
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none());
    }

    // A stop waits for the old worker, so it cannot rewrite the next run's
    // track or status.
    #[test]
    fn a_stop_joins_the_worker() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        j.sharer_start_with("4321".into(), loopback_cfg())
            .expect("start");
        j.stop();
        assert!(j.worker.lock().unwrap_or_else(|e| e.into_inner()).is_none());
    }

    // A run that started on the known track stays there.
    #[test]
    fn a_known_restart_stays_on_the_known_track() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.store_pair(&KeyPair::generate());
        let (_held, port) = held_port();
        let j = joiner_on_port(&rig, port);
        j.sharer_start_known(loopback_cfg()).expect("start");
        j.sharer_restart(loopback_cfg()).expect("known restart");
        assert!(j
            .active_code
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none());
        j.stop();
    }

    #[test]
    fn forget_stops_the_known_sharer_and_deletes_the_pair() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.store_pair(&KeyPair::generate());
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);

        rig.joiner.forget().unwrap();
        rig.wait("gateway dropped", |r| r.starter.alive.load(SeqCst) == 0);
        assert!(
            rig.stored().is_empty(),
            "record (and its key) survived forget"
        );
        assert!(!rig.joiner.is_paired());
        assert_eq!(rig.joiner.paired_name(), None);
        assert_eq!(
            rig.joiner.sharer_start_known(loopback_cfg()),
            Err("no paired device".to_string())
        );
    }

    #[test]
    fn forget_without_a_pair_or_a_state_path_is_a_no_op() {
        assert_eq!(Rig::new().joiner.forget(), Ok(()));
        assert_eq!(Rig::with_state().joiner.forget(), Ok(()));
    }

    // With a slot filled the pairing (code) track is disarmed.
    #[test]
    fn known_sharer_serves_at_start_and_announces_no_code_frames() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        rig.store_pair(&own);
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);
        let (own_pk, peer_pk) = rig
            .starter
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())[0];
        assert!(own_pk == own.public_key_bytes(), "not the stored own key");
        assert!(
            peer_pk == rig.key.public_key_bytes(),
            "not the stored peer key"
        );
        let used = rig.starter.privs.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
        assert!(used == own.private_key_b64(), "not the stored private key");
        assert_eq!(rig.status(), "Searching for the other device");

        rig.sharer_nonces();
        rig.frames();
        rig.advance(ANNOUNCE_INTERVAL);
        std::thread::sleep(Duration::from_millis(200));
        assert!(rig.sharer_nonces().is_empty(), "0x01 on the known track");
        assert_eq!(rig.frames(), 0, "0x02 on the known track");
    }

    #[test]
    fn known_sharer_connects_on_proof_then_rx_and_does_not_rewrite_the_pair() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        rig.store_pair(&own);
        let before = rig.stored();
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);

        let mut ghost = rig.known_ghost(&own);
        rig.drive(&mut ghost, "proof", |r| r.status() == "Connecting");
        assert_eq!(rig.starts(), 1, "gateway restarted for a Proof");
        rig.starter.rx.store(1, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        assert!(rig.stored() == before, "reconnect rewrote the pair");
    }

    fn epoch(rig: &Rig) -> u64 {
        rig.joiner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session_epoch
    }

    /// A known sharer Connected to a ghost joiner whose track is connected.
    fn connected_known(rig: &Rig, own: &KeyPair) -> KnownTrack {
        rig.store_pair(own);
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);
        let mut ghost = rig.known_ghost(own);
        rig.drive(&mut ghost, "proof", |r| r.status() == "Connecting");
        rig.starter.rx.store(1, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        ghost.set_connected(true);
        ghost
    }

    // Live: the laptop stopped and started again within seconds and
    // the phone kept counting the old session. Its Bye ends the session now.
    #[test]
    fn known_sharer_ends_the_session_on_the_peers_bye() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        let mut ghost = connected_known(&rig, &own);
        for _ in 0..3 {
            rig.advance(Duration::from_millis(500));
            ghost.poll(Instant::now()).unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(rig.status(), "Connected");
        ghost.send_bye().unwrap();
        rig.wait("retry after bye", |r| is_retry(&r.status()));
        rig.wait("gateway dropped", |r| r.starter.alive.load(SeqCst) == 0);
        rig.starter.rx.store(0, SeqCst);
        rig.advance(RETRY_AFTER_BYE);
        rig.wait("searching again", |r| {
            r.status() == "Searching for the other device"
        });
    }

    // Live: the hotspot went away for 50 minutes. With no network
    // the search must wait quietly: no Retry, no socket rebind and no gateway
    // restart every few seconds, one event for the outage and one for the
    // return.
    #[test]
    fn known_sharer_with_no_network_waits_quietly_while_searching() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        rig.store_pair(&own);
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);
        rig.joiner.diag();
        *rig.send_fault.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::io::ErrorKind::NetworkUnreachable);
        for _ in 0..60 {
            rig.advance(Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(rig.status(), "Searching for the other device");
        assert_eq!(rig.starts(), 1, "no gateway restart while waiting");
        let diag = rig.joiner.diag();
        assert_eq!(diag.matches("no_network").count(), 1, "{diag}");
        assert!(!diag.contains("socket_rebind"), "{diag}");
        assert!(!diag.contains("socket_error"), "{diag}");

        *rig.send_fault.lock().unwrap_or_else(|e| e.into_inner()) = None;
        rig.wait("network back", |r| {
            r.joiner
                .state
                .lock()
                .unwrap()
                .events
                .iter()
                .any(|e| e.contains("network_back"))
        });
        assert_eq!(rig.status(), "Searching for the other device");
    }

    // A connected session whose network goes is still lost at once.
    #[test]
    fn known_sharer_loses_a_connected_session_when_the_network_goes() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        let _ghost = connected_known(&rig, &own);
        *rig.send_fault.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::io::ErrorKind::NetworkUnreachable);
        rig.wait("lost", |r| is_retry(&r.status()));
    }

    // The laptop restarted (kill, crash, quick stop/start) and data resumed at
    // once: the phone stays Connected but starts a new session.
    #[test]
    fn known_sharer_starts_a_new_session_when_the_peer_restarts() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        let mut ghost = connected_known(&rig, &own);
        // The first session's own Fresh copies are not a restart.
        for _ in 0..12 {
            rig.advance(Duration::from_millis(500));
            ghost.poll(Instant::now()).unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(epoch(&rig), 0);
        drop(ghost);
        let mut restarted = rig.known_ghost(&own);
        rig.drive(&mut restarted, "new session", |r| epoch(r) == 1);
        assert_eq!(rig.status(), "Connected");
        for _ in 0..12 {
            rig.advance(Duration::from_millis(500));
            restarted.poll(Instant::now()).unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(epoch(&rig), 1, "repeated Fresh copies count once");
    }

    #[test]
    fn known_sharer_ignores_a_stranger() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        rig.store_pair(&own);
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);

        let stranger = KeyPair::generate();
        let mut fake = KnownTrack::open(
            &loopback_cfg(),
            &stranger,
            &own.public_key_b64(),
            Role::Joiner,
            Instant::now(),
        )
        .unwrap();
        let listener = KnownSocket::bind(&loopback_cfg()).unwrap();
        let end = Instant::now() + Duration::from_millis(1500);
        let mut answered = 0;
        while Instant::now() < end {
            rig.advance(Duration::from_secs(1));
            fake.poll(Instant::now()).unwrap();
            while let Some((_, f)) = listener.receive(Duration::from_millis(10)).unwrap() {
                if matches!(f, Frame::Proof(p) if p.role == Role::Sharer) {
                    answered += 1;
                }
            }
        }
        assert_eq!(
            answered, 0,
            "sharer answered a Hello without the pair secret"
        );
        assert_eq!(rig.status(), "Searching for the other device");
        assert_eq!(rig.starts(), 1);
    }

    // Loss rules (5983f18) apply to the known track too: the dead gateway is
    // dropped, 10 s later a fresh search restarts it with the SAME stored key.
    #[test]
    fn known_sharer_restarts_with_the_stored_key_after_a_stall_and_reconnects() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        let own = KeyPair::generate();
        rig.store_pair(&own);
        rig.start_known();
        rig.wait("gateway at start", |r| r.starts() == 1);
        let mut ghost = rig.known_ghost(&own);
        rig.drive(&mut ghost, "proof", |r| r.status() == "Connecting");
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        std::thread::sleep(Duration::from_millis(200));

        rig.advance(RX_STALL_TIMEOUT + Duration::from_secs(1));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.wait("gateway dropped", |r| r.starter.alive.load(SeqCst) == 0);
        rig.starter.rx.store(0, SeqCst);
        rig.advance(RETRY_INTERVAL);
        rig.wait("searching again", |r| {
            r.status() == "Searching for the other device"
        });
        rig.wait("gateway restarted", |r| r.starts() == 2);
        let started = rig
            .starter
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert!(started[0] == started[1], "keys changed across the retry");

        rig.drive(&mut ghost, "proof again", |r| r.status() == "Connecting");
        rig.starter.rx.store(1, SeqCst);
        rig.wait("reconnected", |r| r.status() == "Connected");
    }

    // After a code pairing is persisted the retry no longer repeats the code
    // search with a new keypair: it reconnects on the known track.
    #[test]
    fn after_a_code_pairing_is_persisted_retries_use_the_known_track() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        assert_eq!(rig.stored().len(), 1);
        std::thread::sleep(Duration::from_millis(200));

        rig.advance(RX_STALL_TIMEOUT + Duration::from_secs(1));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.starter.rx.store(0, SeqCst);
        rig.advance(RETRY_INTERVAL);
        rig.wait("gateway restarted", |r| r.starts() == 2);
        let started = rig
            .starter
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert!(
            started[0].0 == started[1].0,
            "new keypair: the code search was repeated"
        );
        assert!(started[0].1 == started[1].1);
        rig.sharer_nonces();
        rig.advance(ANNOUNCE_INTERVAL);
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            rig.sharer_nonces().is_empty(),
            "0x01 after the pair was stored"
        );
    }

    // The laptop app is killed and started again: it comes back on the known
    // track (0x03). The phone, still on the code track, must answer it at
    // once and take its new session over, without restarting the tunnel.
    #[test]
    fn the_code_track_answers_the_paired_device_that_came_back() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        let peers = rig.stored();
        assert_eq!(peers.len(), 1, "the pairing was not stored");
        // The phone's own key of the stored pair.
        let sharer = KeyPair::from_private_b64(peers[0].private_key.as_ref().unwrap()).unwrap();

        // The laptop came back: a known track with the laptop's own key.
        let mut ghost = rig.known_ghost(&sharer);
        let end = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while !found {
            rig.advance(Duration::from_secs(1));
            rig.starter.rx.fetch_add(1, SeqCst);
            // The endpoint is per poll: the poll that takes the Proof in is
            // the one that reports it.
            found = ghost.poll(Instant::now()).unwrap().is_some();
            assert!(
                Instant::now() < end,
                "the phone did not answer the known-peer Hello; status {:?}",
                rig.status()
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // The phone took the new session over; the tunnel kept running.
        let end = Instant::now() + Duration::from_secs(5);
        while !rig.took_over() {
            rig.advance(Duration::from_secs(1));
            rig.starter.rx.fetch_add(1, SeqCst);
            ghost.poll(Instant::now()).unwrap();
            assert!(
                Instant::now() < end,
                "the phone did not take the new session over; status {:?}",
                rig.status()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(rig.starts(), 1, "the tunnel was restarted at the takeover");
        assert_eq!(rig.status(), "Connected");
    }

    // A Hello alone proves nothing: the paired device's Hello must not take
    // the session over.
    #[test]
    fn a_bare_hello_from_the_paired_device_takes_nothing_over() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::with_state();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        let peers = rig.stored();
        assert_eq!(peers.len(), 1, "the pairing was not stored");
        let sharer = KeyPair::from_private_b64(peers[0].private_key.as_ref().unwrap()).unwrap();

        // Only a Hello from the paired device: no Proof.
        let socket = KnownSocket::bind(&loopback_cfg()).unwrap();
        let secret = PairSecret::derive(&rig.key, &sharer.public_key_b64()).unwrap();
        for _ in 0..10 {
            rig.advance(Duration::from_secs(1));
            rig.starter.rx.fetch_add(1, SeqCst);
            socket
                .send(&secret.hello(Role::Joiner, [7u8; 16]).encode())
                .unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(!rig.took_over(), "a bare Hello took the session over");
        assert_eq!(rig.starts(), 1, "the tunnel was restarted");
        assert_eq!(rig.status(), "Connected");
    }

    #[test]
    fn sharer_goes_to_retry_when_rx_stalls_after_connected() {
        let _g = MULTICAST.lock().unwrap_or_else(|e| e.into_inner());
        let rig = Rig::new();
        rig.start();
        rig.to_connecting();
        rig.to_tunnel(1);
        rig.starter.rx.store(5, SeqCst);
        rig.wait("connected", |r| r.status() == "Connected");
        std::thread::sleep(Duration::from_millis(200));
        rig.advance(RX_STALL_TIMEOUT - Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(rig.status(), "Connected");
        rig.advance(Duration::from_secs(2));
        rig.wait("retry", |r| is_retry(&r.status()));
        rig.wait("tunnel dropped", |r| r.starter.alive.load(SeqCst) == 0);
    }

    #[test]
    fn the_stats_provider_is_silent_until_a_valid_summary_is_set() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        assert!(provider.respond(b"JFS1 1 summary").is_none());

        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3 junk=1".to_string()));
        let body = provider.respond(b"JFS1 1 summary").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("net=5g"), "{text}");
        assert!(text.contains("sig=3"), "{text}");
        assert!(
            !text.contains("junk"),
            "unknown keys are not echoed: {text}"
        );
    }

    #[test]
    fn an_exit_word_is_served_on_several_answers_so_the_laptop_cannot_miss_it() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        provider.set_exit(ExitReason::Updated);
        // The laptop's poll lands at most every five seconds; two answers
        // survive one lost datagram or one poll that raced the sampler.
        for _ in 0..EXIT_ANSWERS {
            let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
            assert!(text.contains(" exit=updated"), "{text}");
        }
        // After the word was seen it is not repeated: a later outage must not
        // be explained by an old update.
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(!text.contains("exit="), "{text}");
    }

    #[test]
    fn an_exit_word_does_not_clobber_the_sampler_line() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        provider.set_exit(ExitReason::Crash);
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(text.contains("exit=crash"), "{text}");
        // The next sampler tick rewrites the line; the word lives beside it.
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=4".to_string()));
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(text.contains("sig=4"), "{text}");
        assert!(text.contains("exit=crash"), "{text}");
    }

    #[test]
    fn the_stats_provider_answers_history_unsupported_without_data() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        assert_eq!(
            provider.respond(b"JFS1 1 history").unwrap(),
            jafnify_core::phone_stats::UNSUPPORTED_LINE.as_bytes()
        );
    }

    #[test]
    fn the_stats_provider_ignores_a_bad_line_or_a_bad_request() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary sig=9".to_string()));
        assert!(provider.respond(b"JFS1 1 summary").is_none());
        assert!(provider.respond(b"JFS9 1 summary").is_none());
        provider.set_line(Some("JFS1 1 summary v=1 net=wifi".to_string()));
        assert!(provider.respond(b"JFS1 1 summary").is_some());
        assert!(provider.respond(b"JFS1 1 history").is_some());
    }

    #[test]
    fn the_answer_carries_the_wireguard_loss_and_round_trip_of_a_busy_tunnel() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        let counters = Arc::new(Mutex::new(WgStats {
            udp_rx_packets: LOSS_MIN_PACKETS - 1,
            est_loss_permille: 34,
            handshake_rtt_ms: 41,
            ..WgStats::default()
        }));
        provider.set_wg(Some(counters.clone()));
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        // Too few datagrams: the loss says nothing yet, the round trip does.
        assert!(!text.contains("loss="), "{text}");
        assert!(text.contains(" wgrtt=41 caps=link"), "{text}");

        counters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .udp_rx_packets = LOSS_MIN_PACKETS;
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(text.contains(" loss=3 wgrtt=41 caps=link"), "{text}");

        // No handshake yet, no estimate yet: nothing is made up.
        *counters.lock().unwrap_or_else(|e| e.into_inner()) = WgStats {
            udp_rx_packets: 10_000,
            ..WgStats::default()
        };
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(
            !text.contains("loss=") && !text.contains("wgrtt="),
            "{text}"
        );
    }

    #[test]
    fn the_phone_says_it_takes_the_link_report_and_keeps_it_while_fresh() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(text.ends_with(" caps=link"), "{text}");
        assert!(provider.range(now_epoch_ms()).is_none());

        let body = provider
            .respond(b"JFS1 1 summary lvl=-71 rate=1300 mb=1")
            .unwrap();
        assert!(String::from_utf8(body).unwrap().contains("net=5g"));
        let now = now_epoch_ms();
        let (report, trend) = provider.range(now).expect("the report is kept");
        assert_eq!(report.level_dbm, -71);
        assert_eq!(report.rate_100k, Some(1300));
        assert!(report.missed_beacons);
        assert_eq!(trend.len(), 1);
        // A report older than the freshness limit is no report.
        assert!(provider.range(now + RANGE_FRESH_MS + 1).is_none());
        // A request with words that are not key=value is no request.
        assert!(provider.respond(b"JFS1 1 summary lvl").is_none());
    }

    #[test]
    fn the_sharer_clock_starts_at_connected_and_resets_on_a_neutral_gap() {
        let start = Instant::now();
        assert_eq!(next_connected_since(None, true, start), Some(start));
        assert_eq!(
            next_connected_since(Some(start), true, start),
            Some(start)
        );
        assert_eq!(next_connected_since(Some(start), false, start), None);
    }

    #[test]
    fn a_summary_answer_carries_the_sharer_connected_ms() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        provider.set_connected_since(
            Instant::now().checked_sub(Duration::from_millis(12_000)),
        );
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        let sconn: u64 = text
            .split_whitespace()
            .find_map(|token| token.strip_prefix("sconn="))
            .expect("sconn is present")
            .parse()
            .expect("sconn is a number");
        assert!((12_000..12_500).contains(&sconn), "{text}");
    }

    #[test]
    fn an_unconnected_sharer_sends_no_connected_ms() {
        use libwgslirpy::router::StatsProvider;

        let provider = PhoneStatsProvider::new();
        provider.set_line(Some("JFS1 1 summary v=1 net=5g sig=3".to_string()));
        let text = String::from_utf8(provider.respond(b"JFS1 1 summary").unwrap()).unwrap();
        assert!(!text.contains("sconn="), "{text}");
    }

    #[test]
    fn the_status_line_adds_the_clock_and_the_neutral_flag() {
        let joiner = Joiner::new();
        {
            let mut s = joiner.state.lock().unwrap_or_else(|e| e.into_inner());
            s.status = "Connected".to_string();
            s.peer_name = "Phone".to_string();
            s.connected_since = Instant::now().checked_sub(Duration::from_millis(4_000));
            s.reconnecting = true;
        }
        let status = joiner.status_with_endpoint();
        let lines: Vec<&str> = status.lines().collect();
        assert_eq!(lines.len(), 9, "{status}");
        // The neutral flag is additive: line 0 stays the orchestrator word the
        // phone's session tracker feeds on.
        assert_eq!(lines[0], "Connected");
        let connected: u64 = lines[7].parse().expect("the 8th line is the clock");
        assert!((4_000..4_500).contains(&connected), "{status}");
        assert_eq!(lines[8], "1");
        assert!(joiner.connected_ms().is_some());
    }
}
